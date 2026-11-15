//! The repository: creating it on GitHub, cloning and fetching it, reading what it holds, and
//! pushing a commit on top of what was fetched.
//!
//! Git is only a transport here. A sync always fetches, resets the work tree to the repository's
//! branch, writes the merged result over it and pushes a fast-forward commit; if someone pushed in
//! between, the push is refused ([`Error::Race`]) and the caller starts again from a fresh fetch.
//! Graysense never asks git to merge, so the repository can't end up in a conflicted state.
//!
//! Sign-in uses the same GitHub CLI login the updater does (or `GRAYSENSE_GITHUB_TOKEN`,
//! `GH_TOKEN`, `GITHUB_TOKEN`); the token goes to git through its environment, never a command line,
//! and credential helpers are switched off so nothing can open a prompt.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

use super::items::{Item, MissionItem, Snapshot};
use super::{FORMAT, MANIFEST, MAX_FILE, MAX_UPLOAD, MEMORIES_DIR, MISSIONS_DIR, NOTES_DIR, Options, SETTINGS_FILE, repo_dir, safe_file_name};
use crate::memory;
use crate::settings::SYNCED_KEYS;

/// Why a step of syncing failed, in terms the user can act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Git isn't installed.
    NoGit,
    /// Nobody to sign in as, or GitHub won't let them in.
    Auth(String),
    NotFound(String),
    /// The repository is public.
    Public,
    /// The repository holds something that isn't Graysense's.
    NotSyncRepo,
    /// The repository was written by a newer Graysense.
    TooNew(u32),
    /// Someone pushed first: fetch again.
    Race,
    Offline,
    Other(String),
}

impl Error {
    /// What to tell the user.
    pub fn message(&self) -> String {
        match self {
            Error::NoGit => "Sync needs Git. Install Git for Windows (git-scm.com), then try again.".into(),
            Error::Auth(m) | Error::Other(m) => m.clone(),
            Error::NotFound(repo) => format!("GitHub can't find {repo}, or the account Graysense signs in as can't see it. Check the name, or sign in with an account that can (gh auth login)."),
            Error::Public => "That repository is public: anyone could read your notes and memories.".into(),
            Error::NotSyncRepo => "That repository already holds other things. Choose an empty repository, or one Graysense made.".into(),
            Error::TooNew(_) => "That repository was written by a newer version of Graysense. Update Graysense to sync with it.".into(),
            Error::Race => "The repository changed while syncing. Try again.".into(),
            Error::Offline => "Couldn't reach GitHub. Check your connection and try again.".into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

// ------------------------------------------------------------------ git

/// Git, on PATH or where its installers put it.
pub fn git_exe() -> Option<PathBuf> {
    which::which("git").ok().or_else(|| {
        let exe = if cfg!(windows) { "git.exe" } else { "git" };
        let known = [
            std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Git").join("cmd").join(exe)),
            std::env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join("Git").join("cmd").join(exe)),
            dirs::data_local_dir().map(|p| p.join("Programs").join("Git").join("cmd").join(exe)),
        ];
        known.into_iter().flatten().find(|p| p.is_file())
    })
}

/// `(major, minor)` from `git --version` output ("git version 2.49.0.windows.1").
fn parse_git_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split_whitespace().nth(2)?.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The sign-in reaches git through `GIT_CONFIG_*` variables, which older versions ignore (they'd
/// only say "authentication failed"). Say what's wrong instead.
fn check_version(git: &Path) -> Result<(), Error> {
    let mut cmd = Command::new(git);
    cmd.arg("--version");
    hide_window(&mut cmd);
    let version = run(cmd, Duration::from_secs(15)).ok().and_then(|o| parse_git_version(&o.stdout));
    match version {
        Some(v) if v < (2, 31) => Err(Error::Other(format!("Sync needs Git 2.31 or newer, and this is Git {}.{}. Update Git for Windows (git-scm.com) and try again.", v.0, v.1))),
        _ => Ok(()),
    }
}

/// A way to sign in to GitHub.
#[derive(Clone, Debug)]
pub struct Signin {
    /// "GitHub CLI (octocat)": shown to the user, never the token.
    pub label: String,
    pub token: String,
}

/// The sign-ins to try, best first (the updater's list, without "no sign-in").
pub fn signins() -> Vec<Signin> {
    if remote_base().is_some() {
        return vec![Signin { label: "a local test repository".into(), token: "local".into() }];
    }
    crate::update::credentials().into_iter().filter_map(|c| c.token.map(|token| Signin { label: c.label, token })).collect()
}

/// Where repositories live instead of github.com, for tests: a folder holding `owner/name.git`.
fn remote_base() -> Option<PathBuf> {
    std::env::var_os("GRAYSENSE_SYNC_REMOTE").filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The repository, and where its clone lives.
#[derive(Clone, Debug)]
pub struct Remote {
    pub git: PathBuf,
    /// The clone's work tree.
    pub dir: PathBuf,
    pub url: String,
    pub branch: String,
    pub token: Option<String>,
    pub device: String,
}

impl Remote {
    /// `repo` (`owner/name`) on github.com, cloned in Graysense's sync folder.
    pub fn new(git: PathBuf, repo: &str, branch: &str, token: &str, device: &str) -> Remote {
        let (url, token) = match remote_base() {
            Some(base) => (base.join(format!("{repo}.git")).display().to_string(), None),
            // The user name keeps a `url.<base>.insteadOf` rewrite in the user's git config (https to
            // ssh, say) from sending this somewhere the token isn't sent.
            None => (format!("https://x-access-token@github.com/{repo}.git"), Some(token.to_owned())),
        };
        Remote { git, dir: repo_dir(), url, branch: branch.to_owned(), token, device: device.to_owned() }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.git);
        let hooks = self.dir.with_file_name("no-hooks");
        let _ = std::fs::create_dir_all(&hooks);
        c.current_dir(&self.dir)
            .args(["-c", "credential.helper=", "-c", "core.autocrlf=false", "-c", "core.safecrlf=false", "-c", "core.longpaths=true", "-c", "core.quotepath=false", "-c", "core.symlinks=false", "-c", "core.fsmonitor=false", "-c", "commit.gpgsign=false", "-c", "gc.auto=0"])
            .arg("-c")
            .arg(format!("core.hooksPath={}", hooks.display()))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("GIT_AUTHOR_NAME", format!("Graysense ({})", self.device))
            .env("GIT_AUTHOR_EMAIL", "graysense@users.noreply.github.com")
            .env("GIT_COMMITTER_NAME", format!("Graysense ({})", self.device))
            .env("GIT_COMMITTER_EMAIL", "graysense@users.noreply.github.com");
        if let Some(token) = &self.token {
            let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
            c.env("GIT_CONFIG_COUNT", "1").env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader").env("GIT_CONFIG_VALUE_0", format!("AUTHORIZATION: basic {basic}"));
        }
        hide_window(&mut c);
        c
    }

    /// Run git, returning what it printed. A step that talks to the network gets longer.
    fn git(&self, args: &[&str], network: bool) -> Result<String, Error> {
        let mut cmd = self.command();
        cmd.args(args);
        let out = run(cmd, Duration::from_secs(if network { 180 } else { 60 })).map_err(|e| Error::Other(format!("Couldn't run git: {e}")))?;
        if out.ok {
            return Ok(out.stdout);
        }
        Err(explain(&out.stderr, &self.url))
    }

    /// The clone, ready for a fetch: created if it isn't there, pointing at the right repository.
    fn ensure_clone(&self) -> Result<(), Error> {
        std::fs::create_dir_all(&self.dir).map_err(|e| Error::Other(format!("Couldn't create {}: {e}", self.dir.display())))?;
        if !self.dir.join(".git").exists() {
            self.git(&["init", "-q"], false)?;
        }
        // Syncs run one at a time, so a lock still there is from a git we stopped (it timed out).
        let _ = std::fs::remove_file(self.dir.join(".git").join("index.lock"));
        let have = self.git(&["remote"], false)?;
        if have.split_whitespace().any(|r| r == "origin") {
            self.git(&["remote", "set-url", "origin", &self.url], false)?;
        } else {
            self.git(&["remote", "add", "origin", &self.url], false)?;
        }
        Ok(())
    }
}

/// What a finished process left.
pub(crate) struct Ran {
    pub(crate) ok: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// What a child process printed so far, read on a thread as it comes so a chatty process can't
/// fill its pipe and stall.
struct Drain {
    text: std::sync::Arc<parking_lot::Mutex<Vec<u8>>>,
    closed: std::sync::mpsc::Receiver<()>,
}

impl Drain {
    fn start(pipe: Option<impl Read + Send + 'static>) -> Drain {
        let text = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let (tx, closed) = std::sync::mpsc::channel();
        let shared = text.clone();
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    shared.lock().extend_from_slice(&chunk[..n]);
                }
            }
            let _ = tx.send(());
        });
        Drain { text, closed }
    }

    /// What was printed. The pipe closes when the process and everything it started have ended; a
    /// helper that outlives its parent doesn't get to hold this up for more than `wait`.
    fn finish(self, wait: Duration) -> String {
        let _ = self.closed.recv_timeout(wait);
        String::from_utf8_lossy(&self.text.lock()).into_owned()
    }
}

/// Stop `child` and everything it started. On Windows git is a launcher that starts the real git,
/// which starts its helpers (git-remote-https…): ending the launcher alone leaves the rest
/// running, still holding the pipes.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let taskkill = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("taskkill.exe")).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("taskkill"));
        let mut cmd = Command::new(taskkill);
        cmd.args(["/T", "/F", "/PID", &child.id().to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        hide_window(&mut cmd);
        let _ = cmd.status();
    }
    let _ = child.kill();
}

/// Run a command to its end, or stop it (and what it started) when `limit` passes.
pub(crate) fn run(mut cmd: Command, limit: Duration) -> std::io::Result<Ran> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let out = Drain::start(child.stdout.take());
    let err = Drain::start(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if start.elapsed() > limit {
            kill_tree(&mut child);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let wait = Duration::from_secs(if status.is_some() { 5 } else { 2 });
    let stdout = out.finish(wait);
    let stderr = err.finish(wait);
    match status {
        Some(s) => Ok(Ran { ok: s.success(), stdout, stderr }),
        None => Ok(Ran { ok: false, stdout, stderr: format!("timed out after {}s: {stderr}", limit.as_secs()) }),
    }
}

pub(crate) fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// What a failed git command means, from what it said.
fn explain(stderr: &str, url: &str) -> Error {
    let s = stderr.to_lowercase();
    let last = || stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("git failed").trim_start_matches("fatal: ").to_owned();
    // Someone pushed first: either git saw it coming (a stale push is "rejected") or two pushes
    // reached the repository at once and the second couldn't take the branch's lock.
    if s.contains("(fetch first)") || s.contains("non-fast-forward") || s.contains("! [rejected]") || s.contains("updates were rejected") || s.contains("cannot lock ref") || s.contains("(failed to update ref)") {
        Error::Race
    } else if s.contains("gh013") || s.contains("push protection") || s.contains("secret scanning") {
        Error::Other("GitHub's secret scanning refused the push because something in your notes looks like a secret. Remove it (see the repository's push error on github.com) and sync again.".into())
    } else if s.contains("could not resolve host") || s.contains("failed to connect") || s.contains("could not connect") || s.contains("network is unreachable") || s.contains("connection timed out") || s.contains("timed out after") || s.contains("connection reset") || s.contains("unable to access") && s.contains("ssl") {
        Error::Offline
    } else if s.contains("repository not found") || s.contains("does not appear to be a git repository") || s.contains("returned error: 404") {
        Error::NotFound(url.trim_end_matches(".git").rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/"))
    } else if s.contains("authentication failed") || s.contains("could not read username") || s.contains("returned error: 401") || s.contains("returned error: 403") || s.contains("permission to") && s.contains("denied") || s.contains("invalid credentials") {
        Error::Auth("GitHub refused the sign-in Graysense used. Run `gh auth login` again (or check the token you set), and make sure that account can write to the repository.".into())
    } else {
        Error::Other(last())
    }
}

// ------------------------------------------------------------------ GitHub

/// What GitHub says about a repository.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoInfo {
    pub full_name: String,
    pub private: bool,
    pub default_branch: String,
    pub can_push: bool,
}

impl RepoInfo {
    fn from_json(v: &Value) -> Option<RepoInfo> {
        Some(RepoInfo {
            full_name: v.get("full_name")?.as_str()?.to_owned(),
            private: v.get("private").and_then(Value::as_bool).unwrap_or(true),
            default_branch: v.get("default_branch").and_then(Value::as_str).unwrap_or("main").to_owned(),
            can_push: v.pointer("/permissions/push").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// An authenticated GET of the GitHub API.
fn api_get(path: &str, token: &str) -> Result<Value, Error> {
    use velopack::download::{HttpHeader, HttpOptions};
    let header = |name: &str, value: String| HttpHeader { Name: name.into(), Value: value };
    let options = HttpOptions {
        Headers: vec![header("Accept", "application/vnd.github+json".into()), header("X-GitHub-Api-Version", "2022-11-28".into()), header("Authorization", format!("Bearer {token}"))],
        TimeoutMilliseconds: 20_000,
    };
    let text = velopack::download::download_url_as_string(&format!("https://api.github.com/{path}"), Some(&options)).map_err(|e| http_error(&e.to_string(), path))?;
    serde_json::from_str(&text).map_err(|e| Error::Other(format!("GitHub sent something unexpected: {e}")))
}

fn http_error(error: &str, what: &str) -> Error {
    let status = error.split("http status: ").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|s| s.parse::<u16>().ok());
    match status {
        Some(401) => Error::Auth("GitHub refused the sign-in Graysense used. Run `gh auth login` again.".into()),
        Some(403) => Error::Auth("GitHub refused the request (the sign-in may lack the `repo` permission, or the repository needs single sign-on).".into()),
        Some(404) => Error::NotFound(what.trim_start_matches("repos/").to_owned()),
        Some(code) => Error::Other(format!("GitHub returned an error ({code}). Try again later.")),
        None => Error::Offline,
    }
}

/// `(owner, name)` from what the user typed: `owner/name`, a github.com URL (https, ssh or scp
/// style), or just a `name` (no owner).
pub fn parse_repo(input: &str) -> Option<(Option<String>, String)> {
    let input = input.trim();
    let path = if input.contains("github.com") { super::projects::normalize_remote(input)?.strip_prefix("github.com/")?.to_owned() } else { input.trim_matches('/').to_owned() };
    let parts: Vec<&str> = path.split('/').collect();
    let (owner, name) = match parts.as_slice() {
        [name] => (None, *name),
        [owner, name] => (Some(*owner), *name),
        _ => return None,
    };
    let name = name.strip_suffix(".git").unwrap_or(name);
    let word = |s: &str, max: usize, extra: &[char]| !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(&c));
    if !word(name, 100, &['-', '_', '.']) || name == "." || name == ".." || owner.is_some_and(|o| !word(o, 39, &['-'])) {
        return None;
    }
    Some((owner.map(str::to_owned), name.to_owned()))
}

/// What's needed to sync with a repository, once it's been checked.
#[derive(Clone, Debug)]
pub struct Connected {
    pub repo: String,
    pub branch: String,
    pub signin: Signin,
}

/// Check `input` (an existing repository, or with `create` the name of a new private one) with
/// each sign-in until one works. A public repository is refused unless `allow_public`.
pub fn connect(input: &str, create: bool, allow_public: bool) -> Result<Connected, Error> {
    let Some((owner, name)) = parse_repo(input) else { return Err(Error::Other(format!("“{}” isn't a repository name. Use owner/name or the repository's address on github.com.", input.trim()))) };
    let signins = signins();
    if signins.is_empty() {
        return Err(Error::Auth("Sign in to GitHub first: install the GitHub CLI (cli.github.com) and run `gh auth login`, or set GH_TOKEN.".into()));
    }
    if let Some(base) = remote_base() {
        let repo = format!("{}/{name}", owner.as_deref().unwrap_or("local"));
        let dir = base.join(format!("{repo}.git"));
        if create {
            if dir.exists() {
                return Err(Error::Other(format!("{repo} already exists.")));
            }
            std::fs::create_dir_all(&dir).map_err(|e| Error::Other(e.to_string()))?;
            let git = git_exe().ok_or(Error::NoGit)?;
            let mut cmd = Command::new(git);
            cmd.args(["init", "--bare", "-q"]).arg(&dir);
            run(cmd, Duration::from_secs(30)).map_err(|e| Error::Other(e.to_string()))?;
        } else if !dir.exists() {
            return Err(Error::NotFound(repo));
        }
        return Ok(Connected { repo, branch: "main".into(), signin: signins[0].clone() });
    }

    let mut failure: Option<Error> = None;
    for signin in signins {
        let info = if create { create_repo(owner.as_deref(), &name, &signin.token) } else { existing_repo(owner.as_deref(), &name, &signin.token) };
        match info {
            Ok(info) => {
                if !info.can_push {
                    return Err(Error::Auth(format!("{} can read {} but not write to it. Choose a repository you can push to.", signin.label, info.full_name)));
                }
                if !info.private && !allow_public {
                    return Err(Error::Public);
                }
                return Ok(Connected { repo: info.full_name, branch: info.default_branch, signin });
            }
            // A sign-in that can't see it isn't the end: another account might.
            Err(e @ (Error::NotFound(_) | Error::Auth(_))) => {
                failure.get_or_insert(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(failure.unwrap_or(Error::Offline))
}

/// Whether `repo` is private. Checked on every sync: a repository can be made public later.
pub fn is_private(repo: &str, token: &str) -> Result<bool, Error> {
    if remote_base().is_some() {
        return Ok(true);
    }
    Ok(api_get(&format!("repos/{repo}"), token)?.get("private").and_then(Value::as_bool).unwrap_or(true))
}

fn existing_repo(owner: Option<&str>, name: &str, token: &str) -> Result<RepoInfo, Error> {
    let owner = match owner {
        Some(o) => o.to_owned(),
        None => api_get("user", token)?.get("login").and_then(Value::as_str).ok_or_else(|| Error::Other("GitHub didn't say who you are.".into()))?.to_owned(),
    };
    let json = api_get(&format!("repos/{owner}/{name}"), token)?;
    RepoInfo::from_json(&json).ok_or_else(|| Error::Other("GitHub sent something unexpected about that repository.".into()))
}

/// A new private repository, with `gh` (the way the updater signs in): under the user's own
/// account, or `owner`'s when that's an organisation they belong to.
fn create_repo(owner: Option<&str>, name: &str, token: &str) -> Result<RepoInfo, Error> {
    let Some(gh) = crate::update::gh_exe() else {
        return Err(Error::Other("Creating a repository needs the GitHub CLI (cli.github.com). Install it and run `gh auth login`, or create an empty private repository on github.com and connect to that.".into()));
    };
    let login = api_get("user", token)?.get("login").and_then(Value::as_str).unwrap_or_default().to_owned();
    let endpoint = match owner {
        Some(o) if !o.eq_ignore_ascii_case(&login) => format!("orgs/{o}/repos"),
        _ => "user/repos".to_owned(),
    };
    let mut cmd = Command::new(gh);
    cmd.args(["api", "-X", "POST", &endpoint, "-f", &format!("name={name}"), "-f", "description=Memories, notes and preferences synced by Graysense", "-F", "private=true", "-F", "auto_init=false"])
        .env("GH_TOKEN", token)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    hide_window(&mut cmd);
    let out = run(cmd, Duration::from_secs(60)).map_err(|e| Error::Other(format!("Couldn't run the GitHub CLI: {e}")))?;
    let body: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    if out.ok {
        return RepoInfo::from_json(&body).ok_or_else(|| Error::Other("GitHub made the repository but sent something unexpected back.".into()));
    }
    let said = body.get("message").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| out.stderr.trim().to_owned());
    let exists = body.get("errors").and_then(Value::as_array).is_some_and(|e| e.iter().any(|x| x.get("message").and_then(Value::as_str).is_some_and(|m| m.contains("already exists"))));
    Err(if exists { Error::Other(format!("You already have a repository called {name}. Connect to it, or pick another name.")) } else { Error::Other(format!("GitHub couldn't create the repository: {said}")) })
}

// ------------------------------------------------------------------ fetch

/// What a fetch found.
#[derive(Debug)]
pub struct Fetched {
    /// The repository's commit, or `None` while it's empty.
    pub head: Option<String>,
    /// The repository's own default branch (where a push goes, which may not be the one expected).
    pub branch: String,
    pub snapshot: Snapshot,
    /// Items whose files couldn't be read: they're left alone.
    pub unreadable: HashSet<String>,
    /// What was skipped and why, for the report.
    pub skipped: Vec<String>,
    /// Who made the latest commit (a device name) and when.
    pub last: Option<(String, i64)>,
}

/// Bring the clone up to date with the repository and read what it holds.
pub fn fetch(remote: &Remote) -> Result<Fetched, Error> {
    if remote.token.is_some() {
        check_version(&remote.git)?;
    }
    remote.ensure_clone()?;
    remote.git(&["fetch", "--no-tags", "--prune", "origin", "+refs/heads/*:refs/remotes/origin/*"], true)?;
    // The repository's own default branch wins over the one we expected. A repository with no
    // commits has no HEAD to report. A failure here is a failure, not an empty repository: reading
    // the wrong branch as missing would look like everything was deleted.
    let head_ref = remote.git(&["ls-remote", "--symref", "origin", "HEAD"], true)?;
    let branch = head_ref.lines().find_map(|l| l.strip_prefix("ref: refs/heads/")).and_then(|r| r.split_whitespace().next()).map(str::to_owned).unwrap_or_else(|| remote.branch.clone());
    let remote_ref = format!("refs/remotes/origin/{branch}");
    let exists = remote.git(&["for-each-ref", "--format=%(refname)", &remote_ref], false)?.lines().any(|l| l.trim() == remote_ref);
    let (head, last) = if exists {
        remote.git(&["checkout", "-q", "-f", "-B", &branch, &remote_ref], false)?;
        remote.git(&["clean", "-fdxq"], false)?;
        let head = remote.git(&["rev-parse", "HEAD"], false)?.trim().to_owned();
        let last = remote.git(&["log", "-1", "--format=%an%x1f%ct"], false).ok().and_then(|l| {
            let (who, when) = l.trim().split_once('\u{1f}')?;
            Some((who.strip_prefix("Graysense (").and_then(|w| w.strip_suffix(')')).unwrap_or(who).to_owned(), when.parse().ok()?))
        });
        (Some(head), last)
    } else {
        // Nothing there yet: an empty tree on an unborn branch.
        remote.git(&["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")], false)?;
        let _ = remote.git(&["update-ref", "-d", &format!("refs/heads/{branch}")], false);
        let _ = remote.git(&["read-tree", "--empty"], false);
        remote.git(&["clean", "-fdxq"], false)?;
        (None, None)
    };
    let (snapshot, unreadable, skipped) = read_tree(&remote.dir)?;
    Ok(Fetched { head, branch, snapshot, unreadable, skipped, last })
}

/// What a work tree holds. Fails when it isn't a Graysense repository (or not one this version understands).
pub fn read_tree(dir: &Path) -> Result<(Snapshot, HashSet<String>, Vec<String>), Error> {
    let manifest = dir.join(MANIFEST);
    if manifest.is_file() {
        let format = std::fs::read_to_string(&manifest).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.get("format").and_then(Value::as_u64)).unwrap_or(1) as u32;
        if format > FORMAT {
            return Err(Error::TooNew(format));
        }
    } else if std::fs::read_dir(dir).map_err(|e| Error::Other(e.to_string()))?.flatten().any(|e| !is_boilerplate(&e.file_name().to_string_lossy())) {
        return Err(Error::NotSyncRepo);
    }
    let mut snapshot = Snapshot::default();
    let mut unreadable = HashSet::new();
    let mut skipped = Vec::new();
    let unlistable = |folder: &str, e: std::io::Error| Error::Other(format!("Couldn't read the {folder} folder of the sync folder: {e}"));
    for (uid, file, parsed) in read_dir_items::<Item>(&dir.join(MEMORIES_DIR)).map_err(|e| unlistable(MEMORIES_DIR, e))? {
        match parsed.and_then(|i| (i.uid == uid).then_some(i).ok_or_else(|| "its name doesn't match its id".to_owned())).and_then(Item::sanitized) {
            Ok(item) => {
                snapshot.memories.insert(item.uid.clone(), item);
            }
            Err(why) => {
                unreadable.insert(uid);
                skipped.push(format!("{MEMORIES_DIR}/{file} was left alone: {why}."));
            }
        }
    }
    for (uid, file, parsed) in read_dir_items::<MissionItem>(&dir.join(MISSIONS_DIR)).map_err(|e| unlistable(MISSIONS_DIR, e))? {
        match parsed.and_then(|m| (m.uid == uid).then_some(m).ok_or_else(|| "its name doesn't match its id".to_owned())).and_then(MissionItem::sanitized) {
            Ok(m) => {
                snapshot.missions.insert(m.uid.clone(), m);
            }
            Err(why) => {
                unreadable.insert(uid);
                skipped.push(format!("{MISSIONS_DIR}/{file} was left alone: {why}."));
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join(SETTINGS_FILE)) {
        match serde_json::from_str::<BTreeMap<String, Value>>(&text) {
            Ok(map) => snapshot.settings = map.into_iter().filter(|(k, _)| SYNCED_KEYS.contains(&k.as_str())).collect(),
            Err(_) => skipped.push(format!("{SETTINGS_FILE} was left alone: it isn't valid.")),
        }
    }
    Ok((snapshot, unreadable, skipped))
}

/// What GitHub puts in a repository made with "Add a README" (and a license or .gitignore):
/// such a repository is still empty as far as sync is concerned.
fn is_boilerplate(name: &str) -> bool {
    let n = name.to_lowercase();
    let stem = n.split('.').next().unwrap_or("");
    n == ".git" || n == ".gitignore" || n == ".gitattributes" || ["readme", "license", "licence", "copying"].contains(&stem)
}

/// A `<uid>.json` file of the repository: its uid, its name, and what it parsed to.
type Entry<T> = (String, String, Result<T, String>);

/// `(uid from the file name, file name, parsed)` for each `<uid>.json` in `dir`. A folder that
/// isn't there holds nothing; one that can't be listed is an error, not an empty folder (which
/// would read as everything deleted).
fn read_dir_items<T: for<'de> serde::Deserialize<'de>>(dir: &Path) -> std::io::Result<Vec<Entry<T>>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in entries {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(uid) = name.strip_suffix(".json") else { continue };
        if !memory::valid_uid(uid) || !e.path().is_file() {
            continue;
        }
        let parsed = std::fs::read_to_string(e.path()).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<T>(&t).map_err(|e| format!("it isn't valid ({e})")));
        out.push((uid.to_owned(), name, parsed));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// ------------------------------------------------------------------ push

/// What a push did.
#[derive(Debug, PartialEq)]
pub struct Pushed {
    /// The repository's commit now.
    pub head: Option<String>,
    /// Anything was committed (otherwise the repository already had all of it).
    pub changed: bool,
    /// Files that stayed behind, and why.
    pub notes: Vec<String>,
}

/// Write `snapshot` over the work tree (which `fetch` just brought to the repository's commit,
/// `previous` being what it read), commit it and push. Only what `options` includes is touched.
/// `kept` is what the repository holds that this push leaves alone (what this PC can't share): it
/// isn't written, but the readable copy shows it, so every PC writes that copy the same way.
pub fn push(remote: &Remote, previous: &Snapshot, snapshot: &Snapshot, kept: &Snapshot, options: Options, summary: &str) -> Result<Pushed, Error> {
    let mut notes = Vec::new();
    write_tree(&remote.dir, previous, snapshot, kept, options, &mut notes).map_err(|e| Error::Other(format!("Couldn't write the sync folder: {e}")))?;
    // Forced, so a .gitignore the repository came with can't keep a file out.
    remote.git(&["add", "-A", "-f"], false)?;
    if remote.git(&["status", "--porcelain"], false)?.trim().is_empty() {
        let head = remote.git(&["rev-parse", "-q", "--verify", "HEAD"], false).ok().map(|h| h.trim().to_owned());
        return Ok(Pushed { head, changed: false, notes });
    }
    remote.git(&["commit", "-q", "-m", &format!("Sync from {}: {summary}", remote.device)], false)?;
    remote.git(&["push", "-q", "origin", &format!("HEAD:refs/heads/{}", remote.branch)], true)?;
    let head = remote.git(&["rev-parse", "HEAD"], false)?.trim().to_owned();
    Ok(Pushed { head: Some(head), changed: true, notes })
}

/// Remove `path` if it is a link: nothing may be written through one, since it could lead anywhere.
/// (Git is told not to make links, but the work tree is only as trustworthy as the repository.)
fn clear_link(path: &Path) {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        let _ = std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path));
    }
}

fn write_if_changed(path: &Path, text: &str) -> std::io::Result<()> {
    clear_link(path);
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        clear_link(parent);
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// A name for a note's file that no other file of the note has.
fn unique_name(wanted: &str, taken: &HashSet<String>) -> String {
    let mut name = wanted.to_owned();
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        let path = Path::new(wanted);
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        name = format!("{stem} ({n}){ext}");
        n += 1;
    }
    name
}

/// Put the repository's files in the state `snapshot` describes. Files this version doesn't
/// understand, and everything in a part `options` leaves out, are not touched.
fn write_tree(dir: &Path, previous: &Snapshot, snapshot: &Snapshot, kept: &Snapshot, options: Options, notes: &mut Vec<String>) -> std::io::Result<()> {
    let mut snapshot = snapshot.clone();
    let mut uploaded = 0u64;
    for item in snapshot.memories.values_mut() {
        let note_dir = dir.join(NOTES_DIR).join(&item.uid);
        let mut taken: HashSet<String> = HashSet::new();
        for a in item.attachments.iter_mut().filter(|a| a.kind == "file") {
            let wanted = a.file.clone().or_else(|| safe_file_name(&a.name)).unwrap_or_else(|| "file".into());
            let name = unique_name(&wanted, &taken);
            taken.insert(name.to_lowercase());
            let dest = note_dir.join(&name);
            let source = a.local.clone().filter(|p| p.is_file());
            a.file = None;
            if options.files && let Some(src) = source {
                let size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
                let there = std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == size);
                if there {
                    a.file = Some(name);
                } else if size > MAX_FILE {
                    notes.push(format!("{} is too big to keep on GitHub ({} MB; the limit is {} MB), so it only stays on this PC.", a.name, size / 1_048_576, MAX_FILE / 1_048_576));
                } else if uploaded + size > MAX_UPLOAD {
                    notes.push(format!("{} will be added to GitHub next time (a sync adds at most {} MB of files).", a.name, MAX_UPLOAD / 1_048_576));
                } else if looks_like_credentials(&a.name) {
                    notes.push(format!("{} was left out of the sync because its name looks like a credential file.", a.name));
                } else {
                    clear_link(&note_dir);
                    std::fs::create_dir_all(&note_dir)?;
                    clear_link(&dest);
                    std::fs::copy(&src, &dest)?;
                    uploaded += size;
                    a.file = Some(name);
                }
            } else if dest.is_file() {
                // Already in the repository (from another PC): the reference stays.
                a.file = Some(name);
            }
        }
        // Files the note no longer has.
        if options.files && note_dir.is_dir() {
            let keep: HashSet<String> = item.attachments.iter().filter_map(|a| a.file.as_ref().map(|f| f.to_lowercase())).collect();
            for e in std::fs::read_dir(&note_dir)?.flatten() {
                if !keep.contains(&e.file_name().to_string_lossy().to_lowercase()) && e.path().is_file() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    let memories = dir.join(MEMORIES_DIR);
    // Keep the repository's own copy of anything whose meaning hasn't changed. PCs keep their own
    // bookkeeping (when a memory was last touched, say), and rewriting a file for that would make a
    // commit on every sync, flipping back and forth between PCs.
    let refs = |i: &Item| {
        let mut v: Vec<(String, Option<String>)> = i.attachments.iter().map(|a| (a.ident(), a.file.clone())).collect();
        v.sort();
        v
    };
    let chosen: BTreeMap<String, Item> = snapshot
        .memories
        .iter()
        .map(|(uid, item)| {
            let same = previous.memories.get(uid).filter(|p| p.hash() == item.hash() && refs(p) == refs(item));
            (uid.clone(), same.unwrap_or(item).clone())
        })
        .collect();
    for item in chosen.values() {
        write_if_changed(&memories.join(item.file_name()), &item.to_json())?;
    }
    for uid in previous.memories.keys().filter(|u| !snapshot.memories.contains_key(*u)) {
        let _ = std::fs::remove_file(memories.join(format!("{uid}.json")));
        if options.files {
            let _ = std::fs::remove_dir_all(dir.join(NOTES_DIR).join(uid));
        }
    }
    if options.missions {
        let missions = dir.join(MISSIONS_DIR);
        for m in snapshot.missions.values() {
            // As with memories, the repository's own copy stays when nothing about the mission changed.
            let same = previous.missions.get(&m.uid).filter(|p| p.hash() == m.hash());
            write_if_changed(&missions.join(m.file_name()), &same.unwrap_or(m).to_json())?;
        }
        for uid in previous.missions.keys().filter(|u| !snapshot.missions.contains_key(*u)) {
            let _ = std::fs::remove_file(missions.join(format!("{uid}.json")));
        }
    }
    if options.settings && !snapshot.settings.is_empty() {
        // Over what the file holds, so a key a newer Graysense added isn't lost.
        let path = dir.join(SETTINGS_FILE);
        let mut all: BTreeMap<String, Value> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        all.extend(snapshot.settings.clone());
        let mut text = serde_json::to_string_pretty(&all).unwrap_or_default();
        text.push('\n');
        write_if_changed(&path, &text)?;
    }
    write_if_changed(&dir.join(MANIFEST), &format!("{}\n", serde_json::to_string_pretty(&json!({ "app": "graysense", "format": FORMAT })).unwrap_or_default()))?;
    write_if_changed(&dir.join(".gitattributes"), "# Notes can hold any file: never rewrite line endings.\n* -text\n")?;
    if !dir.join("README.md").exists() {
        write_if_changed(&dir.join("README.md"), README)?;
    }
    // Everything the repository holds after this push: what's written, and what it keeps of what this PC can't share.
    let mut shown = kept.clone();
    shown.memories.extend(chosen);
    shown.missions.extend(snapshot.missions.clone());
    shown.settings = snapshot.settings.clone();
    write_if_changed(&dir.join("MEMORY.md"), &render_markdown(&shown))?;
    Ok(())
}

/// Names that are nearly always credentials: such a file never goes to the repository, however
/// private it is.
fn looks_like_credentials(name: &str) -> bool {
    let n = name.to_lowercase();
    n == ".env" || n.starts_with(".env.") || n.starts_with("id_rsa") || n.starts_with("id_ed25519") || ["pem", "key", "pfx", "p12", "kdbx", "ppk"].iter().any(|ext| n.ends_with(&format!(".{ext}")))
}

const README: &str = "# Graysense memory\n\nThis repository is kept by [Graysense](https://github.com/spatney/graysense-terminal): it holds the\nmemories, notes, missions and a few preferences Graysense syncs between your PCs.\n\n- `MEMORY.md` is a readable copy of everything, rewritten on every sync.\n- `memories/`, `missions/` and `settings.json` are what Graysense reads back. Edit them in\n  Graysense rather than here.\n- `notes/` holds the files attached to your notes.\n\nKeep this repository **private**: it has your notes in it.\n";

/// A readable copy of everything, for people browsing the repository. It says nothing about which
/// PC wrote it: a sync from another PC that changes nothing must not make a commit.
pub fn render_markdown(snapshot: &Snapshot) -> String {
    use super::items::ItemScope;
    let mut groups: Vec<(u8, String, Vec<&Item>)> = Vec::new();
    for i in snapshot.memories.values() {
        let (rank, title) = if i.is_note() {
            (3, "Notes".to_owned())
        } else if i.kind == "episode" {
            (4, "Past sessions".to_owned())
        } else {
            match &i.scope {
                ItemScope::Global => (0, "About me".to_owned()),
                ItemScope::Sessions => (1, "Sessions folder".to_owned()),
                ItemScope::Project { name, .. } => (2, format!("Project: {name}")),
            }
        };
        match groups.iter_mut().find(|(r, t, _)| *r == rank && *t == title) {
            Some((_, _, v)) => v.push(i),
            None => groups.push((rank, title, vec![i])),
        }
    }
    groups.sort_by_key(|g| (g.0, g.1.to_lowercase()));
    let mut s = String::from("# Graysense memory\n\nSynced by Graysense. This page is rewritten on every sync; edit memories in Graysense.\n");
    for (_, title, mut items) in groups {
        items.sort_by_key(|i| (i.created, i.uid.clone()));
        s.push_str(&format!("\n## {title}\n\n"));
        for i in items {
            let pending = if i.state == "pending" { " _(suggested)_" } else { "" };
            if i.is_note() {
                s.push_str(&format!("- {}{pending}\n", i.text.replace('\n', "\n  ")));
                for a in &i.attachments {
                    s.push_str(&match (a.kind.as_str(), &a.file) {
                        ("link", _) => format!("  - Link: {}\n", a.target),
                        (_, Some(f)) => format!("  - File: [{}]({NOTES_DIR}/{}/{})\n", a.name, i.uid, f.replace(' ', "%20")),
                        _ => format!("  - File: {}\n", a.name),
                    });
                }
            } else {
                s.push_str(&format!("- {}{pending}\n", i.text));
            }
        }
    }
    if !snapshot.missions.is_empty() {
        s.push_str("\n## Missions\n\n");
        let mut ms: Vec<&MissionItem> = snapshot.missions.values().collect();
        ms.sort_by_key(|m| (m.created, m.uid.clone()));
        for m in ms {
            s.push_str(&format!("- [{}] {}\n", if m.status == "done" { "x" } else { " " }, m.title));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::items::ItemAttachment;
    use crate::sync::items::tests::item;

    const OPTS: Options = Options { files: true, settings: true, missions: true };

    /// `push` for the common case: nothing is held back.
    fn push(remote: &Remote, previous: &Snapshot, snapshot: &Snapshot, options: Options, summary: &str) -> Result<Pushed, Error> {
        super::push(remote, previous, snapshot, &Snapshot::default(), options, summary)
    }

    /// A bare repository standing in for GitHub, and the folders two PCs clone it into.
    struct Lab {
        root: PathBuf,
        git: PathBuf,
        bare: PathBuf,
    }

    impl Lab {
        fn new() -> Option<Lab> {
            let git = git_exe()?;
            let root = std::env::temp_dir().join(format!("gs-sync-{}", memory::new_uid()));
            let bare = root.join("remote.git");
            std::fs::create_dir_all(&bare).unwrap();
            let mut cmd = Command::new(&git);
            cmd.args(["init", "--bare", "-q"]).arg(&bare);
            assert!(run(cmd, Duration::from_secs(30)).unwrap().ok);
            Some(Lab { root, git, bare })
        }

        fn pc(&self, name: &str) -> Remote {
            Remote { git: self.git.clone(), dir: self.root.join(name).join("repo"), url: self.bare.display().to_string(), branch: "main".into(), token: None, device: name.to_owned() }
        }
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn snapshot_of(items: Vec<Item>) -> Snapshot {
        Snapshot { memories: items.into_iter().map(|i| (i.uid.clone(), i)).collect(), ..Default::default() }
    }

    #[test]
    fn a_new_repository_is_filled_by_one_pc_and_read_by_another() {
        let Some(lab) = Lab::new() else { return };
        let (a, b) = (lab.pc("alpha"), lab.pc("beta"));

        let first = fetch(&a).unwrap();
        assert!(first.head.is_none() && first.snapshot.is_empty());

        // A note with a file, a pending suggestion, a mission and a preference.
        let file = lab.root.join("logo.png");
        std::fs::write(&file, b"png bytes").unwrap();
        let mut note = item("aaaaaaaa00000001", "note", "Sent Charles the dark logo");
        note.attachments.push(ItemAttachment { kind: "file".into(), name: "logo.png".into(), target: "C:\\old\\logo.png".into(), size: Some(9), file: None, local: Some(file) });
        note.attachments.push(ItemAttachment { kind: "link".into(), name: "site".into(), target: "https://rayfin.ai/brand".into(), size: None, file: None, local: None });
        let mut pending = item("bbbbbbbb00000002", "fact", "Deploys go out on Fridays");
        pending.state = "pending".into();
        let mut snap = snapshot_of(vec![note.clone(), pending, item("cccccccc00000003", "preference", "Prefers pnpm")]);
        snap.missions.insert("dddddddd00000004".into(), MissionItem { uid: "dddddddd00000004".into(), title: "Draft release notes".into(), notes: "this week".into(), status: "idea".into(), created: 1_700_000_000, from_agent: false });
        snap.settings.insert("theme".into(), json!("slate"));
        let pushed = push(&a, &first.snapshot, &snap, OPTS, "3 added").unwrap();
        assert!(pushed.changed && pushed.head.is_some());

        let got = fetch(&b).unwrap();
        assert_eq!(got.head, pushed.head);
        assert_eq!(got.snapshot.memories.len(), 3);
        assert_eq!(got.snapshot.memories["bbbbbbbb00000002"].state, "pending");
        assert_eq!(got.snapshot.missions["dddddddd00000004"].notes, "this week");
        assert_eq!(got.snapshot.settings["theme"], json!("slate"));
        let note_back = &got.snapshot.memories[&note.uid];
        assert_eq!(note_back.attachments[0].file.as_deref(), Some("logo.png"));
        assert_eq!(std::fs::read(b.dir.join("notes").join(&note.uid).join("logo.png")).unwrap(), b"png bytes");
        assert_eq!(got.last.as_ref().map(|l| l.0.as_str()), Some("alpha"));
        // The repository is readable on github.com without Graysense.
        assert!(std::fs::read_to_string(b.dir.join("MEMORY.md")).unwrap().contains("- Prefers pnpm"));
        assert!(b.dir.join("README.md").is_file() && b.dir.join(MANIFEST).is_file());
        // Hashes (what a sync compares) are the same on both ends.
        assert_eq!(got.snapshot.base().memories, snap.base().memories);

        // Pushing what's already there changes nothing.
        let again = push(&b, &got.snapshot, &got.snapshot, OPTS, "nothing").unwrap();
        assert!(!again.changed);
        assert_eq!(again.head, got.head);
    }

    #[test]
    fn changes_and_deletions_travel_and_a_stale_push_is_refused() {
        let Some(lab) = Lab::new() else { return };
        let (a, b) = (lab.pc("alpha"), lab.pc("beta"));
        let one = item("aaaaaaaa00000001", "preference", "Prefers pnpm");
        let two = item("bbbbbbbb00000002", "fact", "Staging is on 9000");
        let start = fetch(&a).unwrap();
        push(&a, &start.snapshot, &snapshot_of(vec![one.clone(), two.clone()]), OPTS, "2 added").unwrap();

        // Both PCs have fetched the same commit.
        let at_a = fetch(&a).unwrap();
        let at_b = fetch(&b).unwrap();
        let edited = Item { text: "Prefers yarn".into(), ..one.clone() };
        push(&a, &at_a.snapshot, &snapshot_of(vec![edited.clone()]), OPTS, "1 changed, 1 removed").unwrap();
        // B pushes from a commit that's no longer the latest.
        let stale = push(&b, &at_b.snapshot, &snapshot_of(vec![one.clone(), two.clone(), item("cccccccc00000003", "fact", "New on B")]), OPTS, "1 added");
        assert_eq!(stale, Err(Error::Race));

        // After fetching again, B sees both changes (and its work tree is clean).
        let again = fetch(&b).unwrap();
        assert_eq!(again.snapshot.memories.len(), 1);
        assert_eq!(again.snapshot.memories[&one.uid].text, "Prefers yarn");
        assert!(!b.dir.join("memories").join(two.file_name()).exists());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_alone_and_reported() {
        let Some(lab) = Lab::new() else { return };
        let a = lab.pc("alpha");
        let start = fetch(&a).unwrap();
        let good = item("aaaaaaaa00000001", "fact", "Staging is on 9000");
        push(&a, &start.snapshot, &snapshot_of(vec![good.clone()]), OPTS, "1 added").unwrap();
        // Someone edits a file on github.com and breaks it; another is renamed so it no longer matches its id.
        std::fs::write(a.dir.join("memories").join("bbbbbbbb00000002.json"), "{ not json").unwrap();
        std::fs::write(a.dir.join("memories").join("cccccccc00000003.json"), item("dddddddd00000004", "fact", "Moved around").to_json()).unwrap();
        a.git(&["add", "-A"], false).unwrap();
        a.git(&["commit", "-q", "-m", "hand edit"], false).unwrap();
        a.git(&["push", "-q", "origin", "HEAD:refs/heads/main"], true).unwrap();

        let got = fetch(&lab.pc("beta")).unwrap();
        assert_eq!(got.snapshot.memories.len(), 1);
        assert_eq!(got.unreadable, HashSet::from(["bbbbbbbb00000002".to_owned(), "cccccccc00000003".to_owned()]));
        assert_eq!(got.skipped.len(), 2);
        // Pushing doesn't delete what it couldn't read.
        let b = lab.pc("beta");
        push(&b, &got.snapshot, &got.snapshot, OPTS, "nothing").unwrap();
        assert!(b.dir.join("memories").join("bbbbbbbb00000002.json").exists());
    }

    #[test]
    fn bookkeeping_that_differs_between_pcs_does_not_make_a_commit() {
        let Some(lab) = Lab::new() else { return };
        let (a, b) = (lab.pc("alpha"), lab.pc("beta"));
        let start = fetch(&a).unwrap();
        let mut project = item("aaaaaaaa00000001", "fact", "The app builds with just");
        project.scope = crate::sync::items::ItemScope::Project { name: "app".into(), remote: Some("github.com/me/app".into()), key: None };
        push(&a, &start.snapshot, &snapshot_of(vec![project.clone()]), OPTS, "1 added").unwrap();

        // Another PC's copy says the same thing but was touched later and kept other bookkeeping.
        let got = fetch(&b).unwrap();
        let mut mine = project.clone();
        mine.updated += 9_999;
        mine.confidence = 0.5;
        mine.sources.push(memory::Source { session: None, title: None, quote: Some("remember".into()), at: 5 });
        let pushed = push(&b, &got.snapshot, &snapshot_of(vec![mine.clone()]), OPTS, "nothing").unwrap();
        assert!(!pushed.changed, "a commit was made for bookkeeping alone");

        // What it says still wins when it changes.
        mine.text = "The app builds with just and cargo".into();
        assert!(push(&b, &got.snapshot, &snapshot_of(vec![mine]), OPTS, "1 changed").unwrap().changed);
    }

    #[test]
    fn a_repository_that_is_not_graysenses_is_refused() {
        let Some(lab) = Lab::new() else { return };
        let a = lab.pc("alpha");
        fetch(&a).unwrap();
        std::fs::write(a.dir.join("notes.txt"), "my dotfiles").unwrap();
        a.git(&["add", "-A"], false).unwrap();
        a.git(&["commit", "-q", "-m", "first"], false).unwrap();
        a.git(&["push", "-q", "origin", "HEAD:refs/heads/main"], true).unwrap();
        assert_eq!(fetch(&lab.pc("beta")).unwrap_err(), Error::NotSyncRepo);

        // A newer layout is also refused.
        std::fs::write(a.dir.join(MANIFEST), "{\"app\":\"graysense\",\"format\":99}").unwrap();
        a.git(&["add", "-A"], false).unwrap();
        a.git(&["commit", "-q", "-m", "newer"], false).unwrap();
        a.git(&["push", "-q", "origin", "HEAD:refs/heads/main"], true).unwrap();
        assert_eq!(fetch(&lab.pc("gamma")).unwrap_err(), Error::TooNew(99));
    }

    #[test]
    fn what_a_sync_leaves_out_stays_untouched_in_the_repository() {
        let Some(lab) = Lab::new() else { return };
        let (a, b) = (lab.pc("alpha"), lab.pc("beta"));
        let start = fetch(&a).unwrap();
        let mut snap = snapshot_of(vec![item("aaaaaaaa00000001", "fact", "Staging is on 9000")]);
        snap.missions.insert("dddddddd00000004".into(), MissionItem { uid: "dddddddd00000004".into(), title: "Keep me".into(), notes: String::new(), status: "idea".into(), created: 1, from_agent: false });
        snap.settings.insert("theme".into(), json!("slate"));
        push(&a, &start.snapshot, &snap, OPTS, "added").unwrap();

        // A PC that doesn't sync missions or settings pushes an empty set of both.
        let got = fetch(&b).unwrap();
        let mut mine = snapshot_of(vec![item("aaaaaaaa00000001", "fact", "Staging is on 9000"), item("eeeeeeee00000005", "fact", "Also this")]);
        mine.settings.insert("theme".into(), json!("paper"));
        push(&b, &got.snapshot, &mine, Options { files: true, settings: false, missions: false }, "1 added").unwrap();
        let after = fetch(&a).unwrap();
        assert_eq!(after.snapshot.memories.len(), 2);
        assert_eq!(after.snapshot.missions.len(), 1);
        assert_eq!(after.snapshot.settings["theme"], json!("slate"));
    }

    #[test]
    fn files_that_look_like_credentials_or_are_too_big_stay_home() {
        let Some(lab) = Lab::new() else { return };
        let a = lab.pc("alpha");
        let start = fetch(&a).unwrap();
        let dir = lab.root.join("files");
        std::fs::create_dir_all(&dir).unwrap();
        let (env, ok) = (dir.join(".env"), dir.join("plan.txt"));
        std::fs::write(&env, "API_KEY=1").unwrap();
        std::fs::write(&ok, "the plan").unwrap();
        let mut note = item("aaaaaaaa00000001", "note", "Files");
        for (path, name) in [(&env, ".env"), (&ok, "plan.txt")] {
            note.attachments.push(ItemAttachment { kind: "file".into(), name: name.into(), target: path.display().to_string(), size: Some(8), file: None, local: Some(path.clone()) });
        }
        let pushed = push(&a, &start.snapshot, &snapshot_of(vec![note]), OPTS, "1 added").unwrap();
        assert_eq!(pushed.notes.len(), 1);
        assert!(pushed.notes[0].contains(".env"));
        let got = fetch(&lab.pc("beta")).unwrap();
        let back = &got.snapshot.memories["aaaaaaaa00000001"];
        assert_eq!(back.attachments[0].file, None);
        assert_eq!(back.attachments[1].file.as_deref(), Some("plan.txt"));
    }

    /// GitHub itself, read-only: the API's answers, https sign-in through the environment, finding
    /// a repository's default branch, and what "not found" looks like. Nothing is created or pushed.
    /// Run with `cargo test github -- --ignored` while signed in (`gh auth login`).
    #[test]
    #[ignore = "needs the network and a GitHub sign-in"]
    fn github_answers_the_way_the_code_expects() {
        let git = git_exe().expect("Git isn't installed");
        let token = signins().into_iter().next().expect("sign in with `gh auth login` first").token;

        // The API: a public repository this account can only read.
        let info = RepoInfo::from_json(&api_get("repos/octocat/Hello-World", &token).unwrap()).unwrap();
        assert_eq!((info.full_name.as_str(), info.private, info.default_branch.as_str(), info.can_push), ("octocat/Hello-World", false, "master", false));
        assert_eq!(is_private("octocat/Hello-World", &token), Ok(false));
        assert!(matches!(api_get("repos/octocat/no-such-repo-for-graysense", &token), Err(Error::NotFound(r)) if r == "octocat/no-such-repo-for-graysense"));
        // A new repository can't be made in an organisation that doesn't exist: GitHub says no, and nothing is created.
        assert!(matches!(create_repo(Some("graysense-no-such-org-0"), "x", &token), Err(Error::Other(m)) if m.contains("couldn't create")));

        // git over https, signed in through the environment: the default branch is found, and a
        // repository holding only a README counts as empty.
        let root = std::env::temp_dir().join(format!("gs-github-{}", memory::new_uid()));
        let remote = |name: &str| Remote { git: git.clone(), dir: root.join(name).join("repo"), url: format!("https://x-access-token@github.com/octocat/{name}.git"), branch: "main".into(), token: Some(token.clone()), device: "test".into() };
        let got = fetch(&remote("Hello-World")).unwrap();
        assert_eq!(got.branch, "master");
        assert!(got.head.is_some() && got.snapshot.is_empty());
        assert!(matches!(fetch(&remote("no-such-repo-for-graysense")), Err(Error::NotFound(_))));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// git on Windows is a launcher that starts the real git, which starts helpers: stopping a
    /// stalled one must stop them all, or they keep running and hold the sync up.
    #[cfg(windows)]
    #[test]
    fn a_stalled_command_is_stopped_with_everything_it_started() {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        // `cmd` starts `ping`, which would run for a minute; the size marks this test's ping.
        let marker = (3000 + std::process::id() % 5000).to_string();
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "ping", "-n", "60", "-l", &marker, "127.0.0.1"]);
        hide_window(&mut cmd);
        let started = Instant::now();
        let ran = run(cmd, Duration::from_millis(1500)).unwrap();
        assert!(!ran.ok && ran.stderr.contains("timed out"), "{}", ran.stderr);
        assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always));
        let alive = sys.processes().values().filter(|p| p.name().to_string_lossy().eq_ignore_ascii_case("ping.exe") && p.cmd().iter().any(|a| a.to_string_lossy() == marker)).count();
        assert_eq!(alive, 0, "the ping that cmd started is still running");
    }

    #[test]
    fn output_is_read_in_full_from_a_command_that_finishes() {
        let Some(git) = git_exe() else { return };
        let mut cmd = Command::new(git);
        cmd.arg("--version");
        let ran = run(cmd, Duration::from_secs(30)).unwrap();
        assert!(ran.ok && ran.stdout.starts_with("git version"), "{}", ran.stdout);
    }

    #[test]
    fn a_folder_that_cannot_be_listed_is_an_error_not_an_empty_repository() {
        let root = std::env::temp_dir().join(format!("gs-list-{}", memory::new_uid()));
        std::fs::create_dir_all(&root).unwrap();
        // Not there: nothing in it.
        assert_eq!(read_tree(&root).map(|t| t.0.memories.len()), Ok(0));
        // There, but not a folder (it can't be listed): that must not read as "everything was deleted".
        std::fs::write(root.join(MANIFEST), "{\"app\":\"graysense\",\"format\":1}").unwrap();
        std::fs::write(root.join(MEMORIES_DIR), "oops").unwrap();
        let err = read_tree(&root).unwrap_err();
        assert!(matches!(&err, Error::Other(m) if m.contains(MEMORIES_DIR)), "{err:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_one_pc_cannot_share_is_left_in_the_repository_and_does_not_make_the_pcs_trade_commits() {
        let Some(lab) = Lab::new() else { return };
        let (a, b) = (lab.pc("alpha"), lab.pc("beta"));
        let shared = item("aaaaaaaa00000001", "fact", "Builds with just");
        let held = item("bbbbbbbb00000002", "fact", "Deploys on Fridays");
        let mut snap = snapshot_of(vec![shared.clone(), held.clone()]);
        snap.missions.insert("dddddddd00000004".into(), MissionItem { uid: "dddddddd00000004".into(), title: "Ship it".into(), notes: String::new(), status: "idea".into(), created: 1, from_agent: false });
        let start = fetch(&a).unwrap();
        push(&a, &start.snapshot, &snap, OPTS, "3 added").unwrap();

        // B has everything. A has changed `held` into something it can't share (and the mission too): its copy is
        // left out of what A pushes, but the readable copy still lists what the repository holds.
        let at_b = fetch(&b).unwrap();
        assert!(!push(&b, &at_b.snapshot, &snap, OPTS, "nothing").unwrap().changed);
        let at_a = fetch(&a).unwrap();
        let withheld: HashSet<String> = [held.uid.clone(), "dddddddd00000004".to_owned()].into();
        let (previous, kept) = (at_a.snapshot.without(&withheld), at_a.snapshot.only(&withheld));
        let mine = Snapshot { memories: snapshot_of(vec![shared]).memories, settings: snap.settings.clone(), ..Default::default() };
        let pushed = super::push(&a, &previous, &mine, &kept, OPTS, "nothing").unwrap();
        assert!(!pushed.changed, "a commit was made for what A can't share");
        assert!(a.dir.join("memories").join(held.file_name()).exists() && a.dir.join("missions").join("dddddddd00000004.json").exists());
        let readable = std::fs::read_to_string(a.dir.join("MEMORY.md")).unwrap();
        assert!(readable.contains("Deploys on Fridays") && readable.contains("Ship it"), "{readable}");
    }

    #[test]
    fn git_versions_are_read_from_what_git_prints() {
        assert_eq!(parse_git_version("git version 2.49.0.windows.1\n"), Some((2, 49)));
        assert_eq!(parse_git_version("git version 2.30.2"), Some((2, 30)));
        assert_eq!(parse_git_version("git version 10.0"), Some((10, 0)));
        assert_eq!(parse_git_version("not git"), None);
        assert!((2, 30) < (2, 31) && (2, 49) >= (2, 31) && (3, 0) >= (2, 31));
    }

    #[test]
    fn two_files_with_one_name_get_their_own_names() {
        let mut taken = HashSet::new();
        taken.insert("logo.png".to_owned());
        assert_eq!(unique_name("logo.png", &taken), "logo (2).png");
        taken.insert("logo (2).png".to_owned());
        assert_eq!(unique_name("Logo.PNG", &taken), "Logo (3).PNG");
        assert_eq!(unique_name("other.txt", &taken), "other.txt");
    }

    #[test]
    fn repository_names_are_read_from_whatever_the_user_pastes() {
        let some = |o: Option<&str>, n: &str| Some((o.map(str::to_owned), n.to_owned()));
        assert_eq!(parse_repo("octocat/graysense-memory"), some(Some("octocat"), "graysense-memory"));
        assert_eq!(parse_repo("https://github.com/octocat/Hello-World.git"), some(Some("octocat"), "hello-world"));
        assert_eq!(parse_repo("git@github.com:octocat/hello.world.git"), some(Some("octocat"), "hello.world"));
        assert_eq!(parse_repo("  my-memory  "), some(None, "my-memory"));
        for bad in ["", "a/b/c", "owner/..", "own er/repo", "https://gitlab.com/a/b", "https://github.com/onlyowner", "a/b c", "-/x?"] {
            assert_eq!(parse_repo(bad), None, "{bad}");
        }
    }

    #[test]
    fn git_failures_are_explained() {
        assert_eq!(explain("To x\n ! [rejected]        HEAD -> main (fetch first)\nerror: failed to push some refs", "u"), Error::Race);
        assert_eq!(explain(" ! [rejected] main -> main (non-fast-forward)", "u"), Error::Race);
        // Two pushes at the very same moment: the second can't take the branch's lock (real git output).
        assert_eq!(
            explain("remote: error: cannot lock ref 'refs/heads/main': is at bfcc49ad but expected bb8e32b2\nTo C:\\r\\remote.git\n ! [remote rejected] HEAD -> main (failed to update ref)\nerror: failed to push some refs to 'C:\\r\\remote.git'", "u"),
            Error::Race
        );
        // Other server-side refusals are not a race.
        assert!(matches!(explain(" ! [remote rejected] HEAD -> main (protected branch hook declined)\nerror: failed to push some refs to 'x'", "u"), Error::Other(_)));
        assert_eq!(explain("fatal: unable to access 'https://github.com/a/b.git/': Could not resolve host: github.com", "u"), Error::Offline);
        assert!(matches!(explain("remote: Repository not found.\nfatal: repository 'https://x-access-token@github.com/a/b.git/' not found", "https://x-access-token@github.com/a/b.git"), Error::NotFound(r) if r == "a/b"));
        assert!(matches!(explain("fatal: Authentication failed for 'https://github.com/a/b.git/'", "u"), Error::Auth(_)));
        assert!(matches!(explain("remote: error: GH013: Repository rule violations found for refs/heads/main.\n ! [remote rejected] HEAD -> main (push declined due to repository rule violations)", "u"), Error::Other(m) if m.contains("secret")));
        assert_eq!(explain("fatal: something odd happened", "u"), Error::Other("something odd happened".into()));
    }

    #[test]
    fn the_readable_copy_groups_by_where_a_memory_applies() {
        use crate::sync::items::ItemScope;
        let mut snap = snapshot_of(vec![
            item("aaaaaaaa00000001", "preference", "Prefers pnpm"),
            Item { scope: ItemScope::Project { name: "app".into(), remote: Some("github.com/me/app".into()), key: None }, ..item("bbbbbbbb00000002", "procedure", "Run `just test`") },
            item("cccccccc00000003", "note", "Sent the logo"),
        ]);
        snap.missions.insert("dddddddd00000004".into(), MissionItem { uid: "dddddddd00000004".into(), title: "Ship it".into(), notes: String::new(), status: "done".into(), created: 1, from_agent: false });
        let md = render_markdown(&snap);
        let order: Vec<usize> = ["## About me", "## Project: app", "## Notes", "## Missions"].iter().map(|h| md.find(h).unwrap_or_else(|| panic!("missing {h}"))).collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{md}");
        assert!(md.contains("- [x] Ship it") && md.contains("- Run `just test`"));
    }
}
