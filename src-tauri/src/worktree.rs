//! Worktree sessions: a session that works in a git worktree of its own, on a branch of its own,
//! so it can't trip over the user's checkout or another agent's. The worktree goes beside the
//! repository (`<repo>.worktrees\<name>`, on the branch `graysense/<name>`); when the work is
//! done its branch is merged back, pushed for a pull request, or thrown away.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::settings::short_path;
use crate::sync::git::{Ran, git_exe, hide_window, run};

/// Branches Graysense makes are `graysense/<name>`.
pub const BRANCH_PREFIX: &str = "graysense/";
/// What Graysense notes about a worktree it made, kept in the worktree's own git folder (so it
/// goes when the worktree does).
const META: &str = "graysense.json";
/// Uncommitted changes bigger than this don't come along to a fork's worktree.
const CARRY_LIMIT: u64 = 50 * 1024 * 1024;
/// How much of a branch's diff a review shows.
const DIFF_LIMIT: u64 = 2 * 1024 * 1024;
/// How diffs are made, whatever the user's git config says about colors, prefixes and helpers.
const DIFF_ARGS: &[&str] = &["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--no-renames", "--ignore-submodules=all", "--src-prefix=a/", "--dst-prefix=b/"];

/// A worktree Graysense made for a session.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// The worktree's folder.
    pub path: PathBuf,
    /// The branch Graysense made for it ("graysense/fix-login").
    pub branch: String,
    /// The branch it started from, to merge back into (none if the repository wasn't on a
    /// branch). Made from another of Graysense's worktrees, the branch that one merges into.
    pub base_branch: Option<String>,
    /// The commit it started from.
    pub base_commit: String,
    /// The repository's main folder.
    pub repo: PathBuf,
    /// For a forked conversation: the folder it worked in before.
    pub forked_from: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Meta {
    branch: String,
    base_branch: Option<String>,
    base_commit: String,
    forked_from: Option<PathBuf>,
}

/// The Graysense worktree `dir` is in. `None` for any other folder, worktrees the user made
/// themselves included.
pub fn detect(dir: &Path) -> Option<Info> {
    let top = dir.ancestors().find(|d| d.join(".git").exists())?;
    let gitdir = read_gitdir(&top.join(".git"))?;
    let meta: Meta = serde_json::from_slice(&std::fs::read(gitdir.join(META)).ok()?).ok()?;
    if meta.branch.is_empty() {
        return None;
    }
    let repo = repo_of(&common_dir(&gitdir)?);
    Some(Info { path: top.to_path_buf(), branch: meta.branch, base_branch: meta.base_branch, base_commit: meta.base_commit, repo, forked_from: meta.forked_from })
}

/// Where a `.git` file (a linked worktree's, or a submodule's) points.
fn read_gitdir(dot_git: &Path) -> Option<PathBuf> {
    if !dot_git.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(dot_git).ok()?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    if target.is_empty() {
        return None;
    }
    let target = native(target);
    let target = if target.is_absolute() { target } else { dot_git.parent()?.join(target) };
    Some(crate::util::tidy_path(&target))
}

/// The git folder a linked worktree's git folder shares (the repository's own).
fn common_dir(gitdir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = native(text.trim());
    let common = if common.is_absolute() { common } else { gitdir.join(common) };
    Some(crate::util::tidy_path(&common))
}

/// The folder a repository's git folder belongs to (a bare repository is its own).
fn repo_of(common: &Path) -> PathBuf {
    match (common.file_name(), common.parent()) {
        (Some(name), Some(parent)) if name.eq_ignore_ascii_case(".git") => parent.to_path_buf(),
        _ => common.to_path_buf(),
    }
}

/// The main folder of the repository the work tree at `top` belongs to: `top` itself, unless
/// it's a linked worktree.
fn main_root(top: &Path) -> PathBuf {
    read_gitdir(&top.join(".git")).and_then(|g| common_dir(&g)).map(|c| repo_of(&c)).unwrap_or_else(|| top.to_path_buf())
}

/// A path as git prints it, as Windows writes it.
fn native(path: &str) -> PathBuf {
    if cfg!(windows) { PathBuf::from(path.replace('/', "\\")) } else { PathBuf::from(path) }
}

// ------------------------------------------------------------------ running git

/// Git, to run in `dir`.
fn git(dir: &Path) -> Result<Command, String> {
    let exe = git_exe().ok_or_else(|| "Worktree sessions need Git. Install Git for Windows (git-scm.com), then try again.".to_owned())?;
    let mut cmd = Command::new(exe);
    // Paths come out as they are (not quoted), and git never waits for a password typed into a
    // console no one can see.
    cmd.current_dir(dir).args(["-c", "core.quotepath=false"]).env("GIT_TERMINAL_PROMPT", "0");
    hide_window(&mut cmd);
    Ok(cmd)
}

fn finish(cmd: Command, secs: u64) -> Result<Ran, String> {
    run(cmd, Duration::from_secs(secs)).map_err(|e| format!("Couldn't run Git: {e}"))
}

/// Run git in `dir`, giving it `secs` to finish.
fn exec(dir: &Path, args: &[&str], secs: u64) -> Result<Ran, String> {
    let mut cmd = git(dir)?;
    cmd.args(args);
    finish(cmd, secs)
}

/// What git printed, or what went wrong.
fn out(dir: &Path, args: &[&str], secs: u64) -> Result<String, String> {
    let ran = exec(dir, args, secs)?;
    if ran.ok { Ok(ran.stdout) } else { Err(failure(&ran)) }
}

/// What went wrong, from what git said: its error, or a plainer account of a common one.
fn failure(ran: &Ran) -> String {
    let said = if ran.stderr.trim().is_empty() { &ran.stdout } else { &ran.stderr };
    let lower = said.to_lowercase();
    if lower.starts_with("timed out after") {
        return "Git took too long and was stopped.".into();
    }
    if lower.contains("please tell me who you are") || lower.contains("unable to auto-detect email") || lower.contains("empty ident name") {
        return "Git doesn't know who you are yet. Run git config --global user.name \"Your Name\" and git config --global user.email \"you@example.com\", then try again.".into();
    }
    let lines: Vec<&str> = said.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let line = lines.iter().find(|l| l.starts_with("fatal:") || l.starts_with("error:")).or_else(|| lines.iter().rev().find(|l| !l.starts_with("hint:"))).copied().unwrap_or("");
    let line = line.strip_prefix("fatal:").or_else(|| line.strip_prefix("error:")).unwrap_or(line).trim();
    let mut chars = line.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => "Git failed.".into(),
    }
}

/// `what` as a commit id, if it is one.
fn rev(dir: &Path, what: &str) -> Option<String> {
    let ran = exec(dir, &["rev-parse", "--verify", "-q", &format!("{what}^{{commit}}")], 30).ok()?;
    Some(ran.stdout.trim().to_owned()).filter(|id| ran.ok && !id.is_empty())
}

fn branch_exists(dir: &Path, branch: &str) -> bool {
    exec(dir, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")], 30).is_ok_and(|r| r.ok)
}

/// How many commits `range` ("a..b") holds.
fn count(dir: &Path, range: &str) -> Result<u64, String> {
    Ok(out(dir, &["rev-list", "--count", range], 60)?.trim().parse().unwrap_or(0))
}

fn is_ancestor(dir: &Path, older: &str, newer: &str) -> bool {
    exec(dir, &["merge-base", "--is-ancestor", older, newer], 60).is_ok_and(|r| r.ok)
}

/// A file in the temp folder, deleted when this goes.
struct TempFile(PathBuf);

impl TempFile {
    fn new(what: &str) -> TempFile {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        TempFile(std::env::temp_dir().join(format!("graysense-{what}-{}-{n}", std::process::id())))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn output_arg(path: &Path) -> OsString {
    let mut arg = OsString::from("--output=");
    arg.push(path);
    arg
}

/// A copy of the index of the work tree at `top` with every new file marked "to be added", so a
/// diff against a commit shows new files too. The real index isn't touched.
fn scratch_index(top: &Path) -> Result<TempFile, String> {
    let gitdir = native(out(top, &["rev-parse", "--absolute-git-dir"], 30)?.trim());
    let index = TempFile::new("index");
    match std::fs::copy(gitdir.join("index"), &index.0) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Couldn't copy Git's index: {e}")),
    }
    let mut cmd = git(top)?;
    cmd.env("GIT_INDEX_FILE", &index.0).args(["add", "-A", "-N"]);
    let ran = finish(cmd, 300)?;
    if !ran.ok {
        return Err(failure(&ran));
    }
    Ok(index)
}

// ------------------------------------------------------------------ making one

/// A folder and branch name from a session's title: "Fix the login page" is
/// "fix-the-login-page".
pub fn slug_base(hint: &str) -> String {
    let mut slug = String::new();
    for c in hint.chars() {
        if slug.len() >= 40 {
            break;
        }
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return "session".into();
    }
    // Names Windows keeps for devices can't be folders.
    let reserved = matches!(slug, "con" | "prn" | "aux" | "nul") || (slug.len() == 4 && (slug.starts_with("com") || slug.starts_with("lpt")) && slug.as_bytes()[3].is_ascii_digit());
    if reserved { format!("{slug}-session") } else { slug.to_owned() }
}

/// `base`, or `base-2`, `base-3`… whichever has neither a folder nor a branch yet.
fn unique_name(top: &Path, holder: &Path, base: &str) -> Result<String, String> {
    for n in 1..1000 {
        let name = if n == 1 { base.to_owned() } else { format!("{base}-{n}") };
        if !holder.join(&name).exists() && !branch_exists(top, &format!("{BRANCH_PREFIX}{name}")) {
            return Ok(name);
        }
    }
    Err("Couldn't find a free name for the worktree.".into())
}

/// A worktree Graysense just made.
#[derive(Debug)]
pub struct Created {
    pub info: Info,
    /// The folder to work in: the one the session was in, in the worktree.
    pub cwd: PathBuf,
    /// What didn't go to plan without stopping it (changes that couldn't come along).
    pub warning: Option<String>,
}

/// Make a worktree, on a new branch named after `hint` (the session's title), from the commit
/// checked out where `from` is. With `carry`, the changes not committed there (new files too)
/// come along.
pub fn create(from: &Path, hint: &str, carry: bool, forked_from: Option<&Path>) -> Result<Created, String> {
    let ran = exec(from, &["rev-parse", "--show-toplevel", "--show-prefix"], 30)?;
    if !ran.ok {
        return Err(if ran.stderr.to_lowercase().contains("not a git repository") { format!("{} isn't in a Git repository.", short_path(from)) } else { failure(&ran) });
    }
    let mut lines = ran.stdout.lines();
    let top = native(lines.next().unwrap_or_default());
    let prefix = lines.next().unwrap_or_default().trim_end_matches('/').to_owned();
    if top.as_os_str().is_empty() {
        return Err(format!("{} isn't in a Git work tree.", short_path(from)));
    }
    let head = exec(&top, &["rev-parse", "--verify", "-q", "HEAD^{commit}"], 30)?;
    let base_commit = head.stdout.trim().to_owned();
    if !head.ok || base_commit.is_empty() {
        return Err("This repository has no commits yet. Make a first commit, then try again.".into());
    }
    let on = exec(&top, &["symbolic-ref", "-q", "--short", "HEAD"], 30)?;
    let mut base_branch = Some(on.stdout.trim().to_owned()).filter(|b| on.ok && !b.is_empty());
    // Made from another session's worktree, on its branch: it merges where that one would, as
    // that branch goes once it's merged.
    if let Some(parent) = detect(&top)
        && base_branch.as_deref() == Some(parent.branch.as_str())
        && let Some(b) = parent.base_branch.filter(|b| branch_exists(&top, b))
    {
        base_branch = Some(b);
    }

    let repo = main_root(&top);
    let (Some(parent), Some(repo_name)) = (repo.parent(), repo.file_name()) else {
        return Err(format!("Graysense can't make a worktree for the repository at {}.", repo.display()));
    };
    let holder = parent.join(format!("{}.worktrees", repo_name.to_string_lossy()));
    let name = unique_name(&top, &holder, &slug_base(hint))?;
    let branch = format!("{BRANCH_PREFIX}{name}");
    let path = holder.join(&name);
    std::fs::create_dir_all(&holder).map_err(|e| format!("Couldn't create {}: {e}", short_path(&holder)))?;
    let mut cmd = git(&top)?;
    cmd.args(["worktree", "add", "-q", "-b", &branch]).arg(&path).arg(&base_commit);
    let ran = finish(cmd, 600)?;
    if !ran.ok {
        // A branch or folder that's there already is someone else's (it wasn't a moment ago).
        if ran.stderr.contains("already exists") {
            let _ = std::fs::remove_dir(&holder);
        } else {
            undo_add(&top, &path, &branch, &holder);
        }
        return Err(failure(&ran));
    }

    let made = (|| -> Result<Info, String> {
        let gitdir = read_gitdir(&path.join(".git")).ok_or("Git made the worktree, but its .git file can't be read.")?;
        let meta = Meta { branch: branch.clone(), base_branch, base_commit, forked_from: forked_from.map(Path::to_path_buf) };
        let json = serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?;
        std::fs::write(gitdir.join(META), json).map_err(|e| format!("Couldn't write to the worktree's git folder: {e}"))?;
        detect(&path).ok_or_else(|| "Git made the worktree, but Graysense can't find it again.".to_owned())
    })();
    let info = match made {
        Ok(info) => info,
        Err(e) => {
            undo_add(&top, &path, &branch, &holder);
            return Err(e);
        }
    };
    let cwd = Some(path.join(native(&prefix))).filter(|p| !prefix.is_empty() && p.is_dir()).unwrap_or_else(|| path.clone());
    let warning = if carry { carry_changes(&top, &path).err() } else { None };
    Ok(Created { info, cwd, warning })
}

/// Take back a worktree that didn't get made properly, and its branch.
fn undo_add(top: &Path, path: &Path, branch: &str, holder: &Path) {
    if let Ok(mut cmd) = git(top) {
        cmd.args(["worktree", "remove", "--force"]).arg(path);
        let _ = finish(cmd, 120);
    }
    if path.exists() && path.starts_with(holder) {
        let _ = std::fs::remove_dir_all(path);
    }
    let _ = exec(top, &["worktree", "prune"], 60);
    let _ = exec(top, &["branch", "-D", branch], 60);
    let _ = std::fs::remove_dir(holder);
}

/// Bring what isn't committed in the work tree at `from` (new and deleted files too) into `to`,
/// a worktree of the same commit.
fn carry_changes(from: &Path, to: &Path) -> Result<(), String> {
    let index = scratch_index(from)?;
    let patch = TempFile::new("carry");
    let mut cmd = git(from)?;
    cmd.env("GIT_INDEX_FILE", &index.0).args(DIFF_ARGS).args(["--binary", "HEAD"]).arg(output_arg(&patch.0));
    let ran = finish(cmd, 300)?;
    if !ran.ok {
        return Err(format!("The changes not committed in {} couldn't come along ({}); the worktree has the last commit only.", short_path(from), failure(&ran)));
    }
    let size = std::fs::metadata(&patch.0).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Ok(());
    }
    if size > CARRY_LIMIT {
        return Err(format!("The changes not committed in {} are too big to bring along ({} MB); the worktree has the last commit only.", short_path(from), size / (1024 * 1024)));
    }
    let mut cmd = git(to)?;
    cmd.args(["apply", "--binary", "--whitespace=nowarn"]).arg(&patch.0);
    let ran = finish(cmd, 300)?;
    if !ran.ok {
        return Err(format!("The changes not committed in {} couldn't come along ({}); the worktree has the last commit only.", short_path(from), failure(&ran)));
    }
    Ok(())
}

// ------------------------------------------------------------------ reviewing

/// What a worktree's branch changes since it left its base, uncommitted changes included.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub branch: String,
    pub base: Option<String>,
    pub files: Vec<FileStat>,
    pub added: u64,
    pub removed: u64,
    /// Commits on the branch the base doesn't have.
    pub ahead: u64,
    /// Commits on the base since the branch left it.
    pub behind: u64,
    /// Whether some changes aren't committed yet.
    pub dirty: bool,
    /// The changes as a unified diff, when asked for.
    pub diff: Option<String>,
    /// Whether the diff was cut short.
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    /// Lines added and removed (`None` for a binary file).
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

pub fn review(info: &Info, with_diff: bool) -> Result<Review, String> {
    let dir = &info.path;
    if !dir.is_dir() {
        return Err(format!("{} is gone.", short_path(dir)));
    }
    let base = info.base_branch.clone().filter(|b| branch_exists(dir, b));
    // Where the branch left its base (which may have moved on since).
    let from = base
        .as_ref()
        .and_then(|b| exec(dir, &["merge-base", &format!("refs/heads/{b}"), "HEAD"], 60).ok().filter(|r| r.ok))
        .map(|r| r.stdout.trim().to_owned())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| info.base_commit.clone());
    let (ahead, behind) = match &base {
        Some(b) => {
            let counts = out(dir, &["rev-list", "--left-right", "--count", &format!("refs/heads/{b}...HEAD")], 60)?;
            let mut n = counts.split_whitespace().map(|n| n.parse::<u64>().unwrap_or(0));
            let behind = n.next().unwrap_or(0);
            (n.next().unwrap_or(0), behind)
        }
        None => (count(dir, &format!("{}..HEAD", info.base_commit))?, 0),
    };
    // A look that doesn't lock the index the agent may be using.
    let mut cmd = git(dir)?;
    cmd.env("GIT_OPTIONAL_LOCKS", "0").args(["status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=all"]);
    let status = finish(cmd, 120)?;
    if !status.ok {
        return Err(failure(&status));
    }
    let index = scratch_index(dir)?;
    let mut cmd = git(dir)?;
    cmd.env("GIT_INDEX_FILE", &index.0).args(DIFF_ARGS).args(["--numstat", "-z", &from]);
    let ran = finish(cmd, 120)?;
    if !ran.ok {
        return Err(failure(&ran));
    }
    let files = parse_numstat(&ran.stdout);
    let (diff, truncated) = if with_diff {
        let file = TempFile::new("diff");
        let mut cmd = git(dir)?;
        cmd.env("GIT_INDEX_FILE", &index.0).args(DIFF_ARGS).arg(&from).arg(output_arg(&file.0));
        let ran = finish(cmd, 120)?;
        if !ran.ok {
            return Err(failure(&ran));
        }
        let (text, cut) = read_head(&file.0, DIFF_LIMIT)?;
        (Some(text), cut)
    } else {
        (None, false)
    };
    Ok(Review {
        branch: info.branch.clone(),
        base,
        added: files.iter().filter_map(|f| f.added).sum(),
        removed: files.iter().filter_map(|f| f.removed).sum(),
        files,
        ahead,
        behind,
        dirty: !status.stdout.trim().is_empty(),
        diff,
        truncated,
    })
}

/// `git diff --numstat -z`: "added<TAB>removed<TAB>path<NUL>" ("-" for a binary file's counts).
fn parse_numstat(text: &str) -> Vec<FileStat> {
    text.split('\0')
        .map(|r| r.trim_start_matches('\n'))
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let mut parts = r.splitn(3, '\t');
            let (added, removed, path) = (parts.next()?, parts.next()?, parts.next()?);
            Some(FileStat { path: path.to_owned(), added: added.parse().ok(), removed: removed.parse().ok() })
        })
        .collect()
}

/// The start of a text file (at most `limit` bytes, ending at a line's end), and whether there
/// was more.
fn read_head(path: &Path, limit: u64) -> Result<(String, bool), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Couldn't read the diff: {e}"))?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|e| format!("Couldn't read the diff: {e}"))?;
    let cut = bytes.len() as u64 > limit;
    if cut {
        bytes.truncate(limit as usize);
        if let Some(end) = bytes.iter().rposition(|&b| b == b'\n') {
            bytes.truncate(end + 1);
        }
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), cut))
}

// ------------------------------------------------------------------ finishing

/// Commit what isn't committed in the worktree yet, new files too (`false`: there was nothing).
pub fn commit_all(info: &Info, message: &str) -> Result<bool, String> {
    let dir = &info.path;
    if !dir.is_dir() {
        return Err(format!("{} is gone.", short_path(dir)));
    }
    let head = exec(dir, &["symbolic-ref", "-q", "--short", "HEAD"], 30)?;
    let on = head.stdout.trim();
    if !head.ok || on != info.branch {
        return Err(if on.is_empty() {
            format!("The worktree isn't on its branch ({}) any more. Check it out there, then try again.", info.branch)
        } else {
            format!("The worktree is on {on}, not its own branch ({}). Switch back, then try again.", info.branch)
        });
    }
    out(dir, &["add", "-A"], 300)?;
    if exec(dir, &["diff", "--cached", "--quiet"], 120)?.ok {
        return Ok(false);
    }
    let message = Some(message.trim()).filter(|m| !m.is_empty()).unwrap_or("Work from a Graysense session");
    out(dir, &["commit", "-q", "-m", message], 300)?;
    Ok(true)
}

/// A branch merged back.
#[derive(Debug)]
pub struct Merged {
    /// The branch it went into.
    pub into: String,
    /// How many commits it brought (0: there was nothing to merge).
    pub commits: u64,
}

#[derive(Debug)]
pub enum MergeError {
    /// The branches change the same lines; nothing was changed.
    Conflict(Vec<String>),
    Failed(String),
}

impl From<String> for MergeError {
    fn from(e: String) -> Self {
        MergeError::Failed(e)
    }
}

/// Merge the worktree's branch into the branch it started from, committing what isn't committed
/// first. When they conflict, nothing changes.
pub fn merge(info: &Info, message: &str) -> Result<Merged, MergeError> {
    let Some(base) = info.base_branch.clone() else {
        return Err(MergeError::Failed("This worktree didn't start from a branch, so there's nothing to merge it into. Open a pull request instead.".into()));
    };
    commit_all(info, message)?;
    let dir = &info.path;
    let base_ref = format!("refs/heads/{base}");
    let Some(base_id) = rev(dir, &base_ref) else {
        return Err(MergeError::Failed(format!("The branch {base} isn't there any more.")));
    };
    let head_id = rev(dir, "HEAD").ok_or_else(|| "The worktree's branch has no commits.".to_owned())?;
    let commits = count(dir, &format!("{base_id}..{head_id}"))?;
    if commits == 0 {
        return Ok(Merged { into: base, commits: 0 });
    }
    // A trial merge first, which touches no files: a conflict leaves everything as it was.
    let tree = match merge_tree(dir, &base_id, &head_id)? {
        Trial::Conflict(files) => return Err(MergeError::Conflict(files)),
        Trial::Clean(tree) => Ok(tree),
        Trial::Failed(why) => Err(why),
    };
    match checked_out(dir, &base_ref)? {
        // Merged where the base is checked out, so its files change with it.
        Some(at) => merge_in(&at, &info.branch)?,
        None => {
            let merged = if is_ancestor(dir, &base_id, &head_id) {
                head_id
            } else {
                let tree = tree.map_err(MergeError::Failed)?;
                out(dir, &["commit-tree", &tree, "-p", &base_id, "-p", &head_id, "-m", &format!("Merge branch '{}'", info.branch)], 120)?.trim().to_owned()
            };
            out(dir, &["update-ref", "-m", &format!("merge {}", info.branch), &base_ref, &merged, &base_id], 60)?;
        }
    }
    Ok(Merged { into: base, commits })
}

enum Trial {
    Clean(String),
    Conflict(Vec<String>),
    Failed(String),
}

/// Merge two commits without touching any work tree (Git 2.38 or newer).
fn merge_tree(dir: &Path, base: &str, head: &str) -> Result<Trial, String> {
    let ran = exec(dir, &["merge-tree", "--write-tree", "--name-only", "--no-messages", base, head], 300)?;
    let mut lines = ran.stdout.lines();
    let tree = lines.next().unwrap_or_default().trim();
    let is_id = tree.len() >= 40 && tree.bytes().all(|b| b.is_ascii_hexdigit());
    Ok(match (ran.ok, is_id) {
        (true, true) => Trial::Clean(tree.to_owned()),
        (false, true) => {
            let mut files: Vec<String> = lines.map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
            files.dedup();
            Trial::Conflict(files)
        }
        _ if ran.stderr.contains("write-tree") || ran.stderr.contains("usage:") => {
            Trial::Failed("Merging into a branch that isn't checked out needs Git 2.38 or newer. Update Git for Windows (git-scm.com), or check the branch out somewhere, then try again.".into())
        }
        _ => Trial::Failed(failure(&ran)),
    })
}

/// The work tree that has `branch_ref` checked out, if one does.
fn checked_out(dir: &Path, branch_ref: &str) -> Result<Option<PathBuf>, String> {
    let list = out(dir, &["worktree", "list", "--porcelain"], 60)?;
    let (mut path, mut on, mut prunable) = (None, false, false);
    for line in list.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if on && !prunable && let Some(p) = path.take().filter(|p: &PathBuf| p.is_dir()) {
                return Ok(Some(p));
            }
            (path, on, prunable) = (None, false, false);
        } else if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(native(p));
        } else if line.strip_prefix("branch ") == Some(branch_ref) {
            on = true;
        } else if line.starts_with("prunable") {
            prunable = true;
        }
    }
    Ok(None)
}

/// Merge `branch` in the work tree at `at` (which has the branch to merge into checked out).
fn merge_in(at: &Path, branch: &str) -> Result<(), MergeError> {
    if rev(at, "MERGE_HEAD").is_some() {
        return Err(MergeError::Failed(format!("{} is in the middle of a merge. Finish it there, then try again.", short_path(at))));
    }
    let ran = exec(at, &["merge", "--no-edit", branch], 300)?;
    if ran.ok {
        return Ok(());
    }
    if rev(at, "MERGE_HEAD").is_some() {
        let files = out(at, &["diff", "--name-only", "--diff-filter=U"], 60).unwrap_or_default();
        let _ = exec(at, &["merge", "--abort"], 120);
        return Err(MergeError::Conflict(files.lines().map(str::to_owned).collect()));
    }
    let said = format!("{}\n{}", ran.stderr, ran.stdout).to_lowercase();
    if said.contains("would be overwritten") || said.contains("commit your changes or stash them") {
        return Err(MergeError::Failed(format!("Changes not committed in {} would be overwritten. Commit or stash them there, then try again.", short_path(at))));
    }
    Err(MergeError::Failed(failure(&ran)))
}

/// A branch pushed for a pull request.
#[derive(Debug)]
pub struct Published {
    /// Where to open the pull request, when Graysense knows.
    pub url: Option<String>,
    pub remote: String,
}

/// Commit what isn't committed, then push the worktree's branch.
pub fn publish(info: &Info, message: &str) -> Result<Published, String> {
    commit_all(info, message)?;
    let dir = &info.path;
    let base = info.base_branch.clone().filter(|b| branch_exists(dir, b));
    let ahead = match &base {
        Some(b) => count(dir, &format!("refs/heads/{b}..HEAD"))?,
        None => count(dir, &format!("{}..HEAD", info.base_commit))?,
    };
    if ahead == 0 {
        return Err("There's nothing on this branch to publish yet.".into());
    }
    let remotes = out(dir, &["remote"], 30)?;
    let remotes: Vec<&str> = remotes.lines().map(str::trim).filter(|r| !r.is_empty()).collect();
    let Some(remote) = remotes.iter().find(|r| **r == "origin").or(remotes.first()).map(|r| r.to_string()) else {
        return Err("This repository has no remote to push to. Add one (git remote add origin <url>), then try again.".into());
    };
    let refspec = format!("refs/heads/{0}:refs/heads/{0}", info.branch);
    let ran = exec(dir, &["push", "-u", &remote, &refspec], 180)?;
    if !ran.ok {
        let said = ran.stderr.to_lowercase();
        if said.contains("authentication failed") || said.contains("could not read username") || said.contains("permission denied") || said.contains("returned error: 403") {
            return Err(format!("Git couldn't sign in to push to {remote}. Check that you can push from a terminal (for GitHub, gh auth login and then gh auth setup-git), then try again."));
        }
        return Err(failure(&ran));
    }
    let url = out(dir, &["remote", "get-url", &remote], 30).unwrap_or_default();
    let url = url.trim();
    Ok(Published { url: github_compare(url, base.as_deref(), &info.branch).or_else(|| pr_link(&ran.stderr, url)), remote })
}

/// The host and path of a remote's URL (https, ssh, or scp-like `git@host:path`).
fn host_and_path(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            if !["https", "http", "ssh", "git", "git+ssh", "ssh+git"].contains(&scheme.to_ascii_lowercase().as_str()) {
                return None;
            }
            rest.split_once('/')?
        }
        None => {
            let (left, path) = url.split_once(':')?;
            // Not a drive ("C:\repos\app") or a local path.
            if left.len() < 2 || left.contains(['/', '\\']) {
                return None;
            }
            (left, path)
        }
    };
    let host = authority.rsplit('@').next()?.split(':').next()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((host, path.to_owned()))
}

/// Where to open a pull request for `branch` on GitHub (github.com, or a GHE.com host), if the
/// remote is there.
fn github_compare(remote_url: &str, base: Option<&str>, branch: &str) -> Option<String> {
    let (host, path) = host_and_path(remote_url)?;
    let host = if host == "ssh.github.com" || host == "www.github.com" { "github.com".to_owned() } else { host };
    if host != "github.com" && !host.ends_with(".ghe.com") {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    let name_ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if parts.next().is_some() || !name_ok(owner) || !name_ok(repo) {
        return None;
    }
    let range = match base {
        Some(base) => format!("{}...{}", url_encode(base), url_encode(branch)),
        None => url_encode(branch),
    };
    Some(format!("https://{host}/{owner}/{repo}/compare/{range}?expand=1"))
}

fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The link for opening a pull request that a remote printed when a branch was pushed to it
/// (GitLab and Azure DevOps do), if it's on the remote's own site.
fn pr_link(said: &str, remote_url: &str) -> Option<String> {
    let site = |host: &str| host.rsplit('.').take(2).collect::<Vec<_>>().join(".");
    let ours = site(&host_and_path(remote_url)?.0);
    said.lines()
        .filter_map(|l| l.trim().strip_prefix("remote:"))
        .flat_map(str::split_whitespace)
        .find(|word| {
            let lower = word.to_lowercase();
            lower.starts_with("https://") && (lower.contains("pull") || lower.contains("merge_request")) && host_and_path(word).is_some_and(|(h, _)| site(&h) == ours)
        })
        .map(str::to_owned)
}

/// Remove the worktree, whatever is in it, and with `delete_branch` its branch.
pub fn remove(info: &Info, delete_branch: bool) -> Result<(), String> {
    // Only ever a folder Graysense made: in `<repo>.worktrees`, and not the repository.
    let holder = info.path.parent().filter(|h| h.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".worktrees")));
    let Some(holder) = holder.filter(|_| !crate::sessions::is_under(&info.repo, &info.path)) else {
        return Err(format!("{} isn't a worktree Graysense made, so it was left alone.", short_path(&info.path)));
    };
    if !info.repo.is_dir() {
        let _ = std::fs::remove_dir_all(&info.path);
        let _ = std::fs::remove_dir(holder);
        return if info.path.exists() { Err(format!("Couldn't remove {}.", short_path(&info.path))) } else { Ok(()) };
    }
    // A session that just closed may still be letting go of files in there.
    std::thread::sleep(Duration::from_millis(500));
    for attempt in 0..3 {
        if !info.path.exists() {
            break;
        }
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let mut cmd = git(&info.repo)?;
        cmd.args(["worktree", "remove", "--force"]).arg(&info.path);
        let _ = finish(cmd, 120)?;
    }
    if info.path.exists() {
        let _ = std::fs::remove_dir_all(&info.path);
    }
    let _ = exec(&info.repo, &["worktree", "prune"], 60);
    if info.path.exists() {
        return Err(format!("Couldn't remove {}: something may be using a file in it. Close it and delete the folder (the branch {} was kept).", short_path(&info.path), info.branch));
    }
    if delete_branch && info.branch.starts_with(BRANCH_PREFIX) {
        let ran = exec(&info.repo, &["branch", "-D", &info.branch], 60)?;
        if !ran.ok && !ran.stderr.contains("not found") {
            return Err(failure(&ran));
        }
    }
    let _ = std::fs::remove_dir(holder);
    Ok(())
}

/// Remove the worktree and its branch if nothing was done in it: no changes, no commits the base
/// doesn't have (`false`: it was kept).
pub fn remove_if_unused(info: &Info) -> Result<bool, String> {
    let r = review(info, false)?;
    if !r.files.is_empty() || r.ahead > 0 || r.dirty {
        return Ok(false);
    }
    remove(info, true).map(|_| true)
}

/// The repository a worktree Graysense made belonged to, from where it was (for a session whose
/// worktree has been removed since).
pub fn repo_of_removed(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find_map(|p| {
            let holder = p.parent()?;
            let repo = holder.file_name()?.to_str()?.strip_suffix(".worktrees")?;
            Some(holder.parent()?.join(repo))
        })
        .filter(|repo| repo.join(".git").exists())
}

/// Whether `dir` is in a git work tree (one Graysense can make a worktree from).
pub fn in_repo(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// The top folder of the git work tree `dir` is in (a worktree is one of its own), or `dir`
/// itself outside one.
pub fn top_of(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

/// What the agent is told about working in a worktree (added to its system message).
pub fn instructions(info: &Info) -> String {
    let base = info.base_branch.as_deref().map(|b| format!(" It started from `{b}`.")).unwrap_or_default();
    let mut text = format!(
        "You are working in a git worktree Graysense made for this session: {path}, on the branch `{branch}`.{base} \
         The repository's main folder is {repo}: don't change anything there, make every change in the worktree.\n\
         - Commit your work to `{branch}` when a piece of it is done, with a clear message.\n\
         - Don't switch branches, rebase, merge or push unless the user asks: they merge the branch or open a pull request from Graysense.\n\
         - Files git ignores aren't here (dependencies, build output, local settings): install dependencies or build before running anything that needs them, and copy any local config it needs (such as .env) from the main folder.",
        path = info.path.display(),
        branch = info.branch,
        repo = info.repo.display(),
    );
    if let Some(from) = &info.forked_from {
        text.push_str(&format!("\n\nEarlier in this conversation you worked in {}. Work in {} now: paths there map to the same places here.", from.display(), info.path.display()));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with one commit on `main`, in a temp folder that's removed afterwards.
    struct Lab {
        root: PathBuf,
        repo: PathBuf,
    }

    impl Lab {
        fn new() -> Option<Lab> {
            git_exe()?;
            let root = std::env::temp_dir().join(format!("gs-wt-{}", crate::memory::new_uid()));
            let repo = root.join("app");
            std::fs::create_dir_all(repo.join("src")).unwrap();
            ok(&repo, &["init", "-q", "-b", "main"]);
            for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
                ok(&repo, &["config", key, value]);
            }
            std::fs::write(repo.join("README.md"), "hello\n").unwrap();
            std::fs::write(repo.join("src").join("main.rs"), "fn main() {}\n").unwrap();
            ok(&repo, &["add", "-A"]);
            ok(&repo, &["commit", "-q", "-m", "first"]);
            Some(Lab { root, repo })
        }
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn ok(dir: &Path, args: &[&str]) -> String {
        let ran = exec(dir, args, 60).unwrap();
        assert!(ran.ok, "git {args:?} failed: {}", ran.stderr);
        ran.stdout
    }

    fn same(a: &Path, b: &Path) -> bool {
        matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
    }

    fn read(path: PathBuf) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn slugs_make_safe_folder_and_branch_names() {
        assert_eq!(slug_base("Fix the login page!"), "fix-the-login-page");
        assert_eq!(slug_base("Update README.md -- and tests"), "update-readme-md-and-tests");
        assert_eq!(slug_base("  ***  "), "session");
        assert_eq!(slug_base("Добавь кнопку"), "session");
        assert_eq!(slug_base("CON"), "con-session");
        assert_eq!(slug_base("lpt1"), "lpt1-session");
        assert_eq!(slug_base("compile"), "compile");
        let long = slug_base(&"word ".repeat(30));
        assert!(long.len() <= 40 && !long.ends_with('-'), "{long}");
    }

    #[test]
    fn a_worktree_is_made_found_reviewed_and_removed() {
        let Some(lab) = Lab::new() else { return };
        let created = create(&lab.repo.join("src"), "Fix the login page", false, None).unwrap();
        let info = &created.info;
        assert_eq!(info.branch, "graysense/fix-the-login-page");
        assert_eq!(info.base_branch.as_deref(), Some("main"));
        assert!(same(&info.repo, &lab.repo));
        assert!(same(&info.path, &lab.root.join("app.worktrees").join("fix-the-login-page")));
        assert!(same(&created.cwd, &info.path.join("src")), "{:?}", created.cwd);
        assert!(created.warning.is_none() && info.forked_from.is_none());
        // Found again from any folder in it, and memory files it under the repository.
        assert_eq!(detect(&created.cwd).as_ref(), Some(info));
        assert!(detect(&lab.repo).is_none());
        assert!(same(&crate::memory::Project::for_dir(&created.cwd).root, &lab.repo));
        assert!(instructions(info).contains("graysense/fix-the-login-page"));
        // A worktree the user made isn't Graysense's, and is a project of its own as before.
        let mine = lab.root.join("mine");
        ok(&lab.repo, &["worktree", "add", "-q", "-b", "mine", mine.to_str().unwrap()]);
        assert!(detect(&mine).is_none());
        assert!(same(&crate::memory::Project::for_dir(&mine).root, &mine));
        // The same title again gets a name of its own, and one made from a worktree still goes
        // beside the repository.
        let second = create(&mine, "Fix the login page", false, None).unwrap();
        assert_eq!(second.info.branch, "graysense/fix-the-login-page-2");
        assert_eq!(second.info.base_branch.as_deref(), Some("mine"));
        assert!(same(second.info.path.parent().unwrap(), &lab.root.join("app.worktrees")));

        let r = review(info, true).unwrap();
        assert!(r.files.is_empty() && !r.dirty && r.ahead == 0 && r.behind == 0);
        std::fs::write(info.path.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(info.path.join("new.txt"), "new\n").unwrap();
        std::fs::write(info.path.join("logo.bin"), [0u8, 1, 2, 0, 255]).unwrap();
        let r = review(info, true).unwrap();
        assert!(r.dirty);
        assert_eq!(r.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["README.md", "logo.bin", "new.txt"]);
        assert_eq!((r.added, r.removed), (2, 0));
        assert_eq!(r.files[1].added, None);
        let diff = r.diff.unwrap();
        assert!(diff.contains("+world") && diff.contains("+new") && diff.contains("Binary files"), "{diff}");
        // Committing takes everything, new files too; then there's nothing left.
        assert!(commit_all(info, "Say hello to the world").unwrap());
        assert!(!commit_all(info, "Again").unwrap());
        let r = review(info, false).unwrap();
        assert!(!r.dirty && r.ahead == 1 && r.diff.is_none() && r.files.len() == 3);
        // The main folder never saw any of it.
        assert_eq!(read(lab.repo.join("README.md")), "hello\n");
        assert!(ok(&lab.repo, &["status", "--porcelain"]).trim().is_empty());

        remove(info, true).unwrap();
        assert!(!info.path.exists() && !branch_exists(&lab.repo, &info.branch));
        assert!(lab.root.join("app.worktrees").exists());
        // A session resumed after its worktree went works in the repository instead.
        assert!(same(&repo_of_removed(&info.path.join("src")).unwrap(), &lab.repo));
        assert!(repo_of_removed(&lab.repo).is_none());
        // Closing a session that did nothing in its worktree takes it away; work keeps it.
        std::fs::write(second.info.path.join("notes.txt"), "keep me\n").unwrap();
        assert!(!remove_if_unused(&second.info).unwrap());
        assert!(second.info.path.join("notes.txt").exists());
        std::fs::remove_file(second.info.path.join("notes.txt")).unwrap();
        assert!(remove_if_unused(&second.info).unwrap());
        assert!(!lab.root.join("app.worktrees").exists() && !branch_exists(&lab.repo, &second.info.branch));
        // The user's own worktree and branch are still there.
        assert!(mine.is_dir() && branch_exists(&lab.repo, "mine"));
    }

    #[test]
    fn a_fork_brings_the_changes_not_committed_along() {
        let Some(lab) = Lab::new() else { return };
        std::fs::write(lab.repo.join("README.md"), "hello\r\nchanged\r\n").unwrap();
        std::fs::write(lab.repo.join("src").join("lib.rs"), "pub fn x() {}\n").unwrap();
        std::fs::write(lab.repo.join("logo.bin"), [0u8, 1, 2, 255, 0, 10]).unwrap();
        std::fs::remove_file(lab.repo.join("src").join("main.rs")).unwrap();
        let created = create(&lab.repo, "Try another way", true, Some(&lab.repo)).unwrap();
        assert!(created.warning.is_none(), "{:?}", created.warning);
        let wt = &created.info.path;
        assert_eq!(read(wt.join("README.md")), "hello\r\nchanged\r\n");
        assert_eq!(read(wt.join("src").join("lib.rs")), "pub fn x() {}\n");
        assert_eq!(std::fs::read(wt.join("logo.bin")).unwrap(), [0u8, 1, 2, 255, 0, 10]);
        assert!(!wt.join("src").join("main.rs").exists());
        // The original keeps its changes as they were, its index untouched.
        let status = ok(&lab.repo, &["status", "--porcelain"]);
        for line in [" M README.md", " D src/main.rs", "?? logo.bin", "?? src/lib.rs"] {
            assert!(status.lines().any(|l| l == line), "{line} in {status}");
        }
        assert!(same(created.info.forked_from.as_deref().unwrap(), &lab.repo));
        assert!(instructions(&created.info).contains("Earlier in this conversation you worked in"));

        // A fork of the fork starts where it is, and merges where it would: not into its branch.
        ok(wt, &["add", "-A"]);
        ok(wt, &["commit", "-q", "-m", "first try"]);
        let again = create(wt, "Try a third way", true, Some(wt)).unwrap().info;
        assert_eq!(again.base_branch, created.info.base_branch);
        assert_eq!(again.base_commit, ok(wt, &["rev-parse", "HEAD"]).trim());
    }

    #[test]
    fn merging_brings_the_branch_back_and_a_conflict_changes_nothing() {
        let Some(lab) = Lab::new() else { return };
        let a = create(&lab.repo, "one", false, None).unwrap().info;
        std::fs::write(a.path.join("a.txt"), "from a\n").unwrap();
        let merged = merge(&a, "Add a").unwrap();
        assert_eq!((merged.into.as_str(), merged.commits), ("main", 1));
        // `main` is checked out in the main folder, so its files moved along.
        assert_eq!(read(lab.repo.join("a.txt")), "from a\n");
        assert_eq!(merge(&a, "Again").unwrap().commits, 0);

        let b = create(&lab.repo, "two", false, None).unwrap().info;
        std::fs::write(b.path.join("README.md"), "from b\n").unwrap();
        std::fs::write(lab.repo.join("README.md"), "from main\n").unwrap();
        ok(&lab.repo, &["commit", "-qam", "main changes"]);
        match merge(&b, "B's readme") {
            Err(MergeError::Conflict(files)) => assert_eq!(files, ["README.md"]),
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert!(rev(&lab.repo, "MERGE_HEAD").is_none());
        assert_eq!(read(lab.repo.join("README.md")), "from main\n");
        assert!(ok(&lab.repo, &["status", "--porcelain"]).trim().is_empty());
        // B's work was committed on its branch, ready to be fixed.
        let r = review(&b, false).unwrap();
        assert!(!r.dirty && r.ahead == 1 && r.behind == 1);
    }

    #[test]
    fn merging_into_a_branch_not_checked_out_moves_only_the_branch() {
        let Some(lab) = Lab::new() else { return };
        let wt = create(&lab.repo, "three", false, None).unwrap().info;
        ok(&lab.repo, &["switch", "-q", "-c", "other"]);
        std::fs::write(wt.path.join("c.txt"), "c\n").unwrap();
        assert_eq!(merge(&wt, "Add c").unwrap().commits, 1);
        assert_eq!(ok(&lab.repo, &["show", "main:c.txt"]), "c\n");
        assert!(!lab.repo.join("c.txt").exists());

        // `main` moves on meanwhile, so this one takes a merge commit.
        ok(&lab.repo, &["switch", "-q", "main"]);
        std::fs::write(lab.repo.join("e.txt"), "e\n").unwrap();
        ok(&lab.repo, &["add", "e.txt"]);
        ok(&lab.repo, &["commit", "-q", "-m", "Add e"]);
        ok(&lab.repo, &["switch", "-q", "other"]);
        std::fs::write(wt.path.join("d.txt"), "d\n").unwrap();
        assert_eq!(merge(&wt, "Add d").unwrap().commits, 1);
        assert_eq!(ok(&lab.repo, &["show", "main:d.txt"]), "d\n");
        assert_eq!(ok(&lab.repo, &["show", "main:e.txt"]), "e\n");
        assert_eq!(ok(&lab.repo, &["rev-list", "--parents", "-n", "1", "main"]).split_whitespace().count(), 3);
        assert!(ok(&lab.repo, &["status", "--porcelain"]).trim().is_empty());
    }

    #[test]
    fn remotes_give_pull_request_links() {
        let compare = |url: &str, base: Option<&str>, branch: &str| github_compare(url, base, branch);
        assert_eq!(compare("https://github.com/octo/app.git", Some("main"), "graysense/fix-login").as_deref(), Some("https://github.com/octo/app/compare/main...graysense/fix-login?expand=1"));
        assert_eq!(compare("git@github.com:octo/app.git", Some("release/1.0"), "graysense/x").as_deref(), Some("https://github.com/octo/app/compare/release/1.0...graysense/x?expand=1"));
        assert_eq!(compare("ssh://git@ssh.github.com:443/octo/app.git", None, "graysense/x").as_deref(), Some("https://github.com/octo/app/compare/graysense/x?expand=1"));
        assert_eq!(compare("https://me@octo.ghe.com/team/app/", Some("main"), "graysense/a#b").as_deref(), Some("https://octo.ghe.com/team/app/compare/main...graysense/a%23b?expand=1"));
        assert_eq!(compare("https://gitlab.com/octo/app.git", Some("main"), "x"), None);
        assert_eq!(compare("C:\\repos\\app", Some("main"), "x"), None);
        assert_eq!(compare("https://github.com/octo", Some("main"), "x"), None);

        let said = "remote:\nremote: To create a merge request for graysense/x, visit:\nremote:   https://gitlab.com/octo/app/-/merge_requests/new?merge_request%5Bsource_branch%5D=graysense%2Fx\nremote:\nTo gitlab.com:octo/app.git\n";
        assert_eq!(pr_link(said, "git@gitlab.com:octo/app.git").as_deref(), Some("https://gitlab.com/octo/app/-/merge_requests/new?merge_request%5Bsource_branch%5D=graysense%2Fx"));
        assert_eq!(pr_link("remote: https://evil.example/pull/1", "https://gitlab.com/octo/app.git"), None);
    }

    #[test]
    fn git_failures_read_plainly() {
        let said = |stderr: &str| failure(&Ran { ok: false, stdout: String::new(), stderr: stderr.into() });
        assert_eq!(said("hint: try this\nfatal: not a git repository (or any of the parent directories): .git\n"), "Not a git repository (or any of the parent directories): .git");
        assert!(said("Author identity unknown\n\n*** Please tell me who you are.\n").contains("git config --global user.name"));
        assert_eq!(said("timed out after 60s: "), "Git took too long and was stopped.");
        assert_eq!(said(""), "Git failed.");
    }
}
