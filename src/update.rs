//! Updates for installed copies of Graysense, through Velopack. A background check finds a newer
//! release on GitHub and downloads it; it installs when Graysense restarts or closes.
//!
//! The releases are private, so checks sign in with the user's GitHub CLI login, or a token in
//! GRAYSENSE_GITHUB_TOKEN, GH_TOKEN or GITHUB_TOKEN. GRAYSENSE_UPDATE_FEED points the updater at
//! another feed instead (a folder or URL of `vpk pack` output), e.g. to try an update locally.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText, Sense, Ui, vec2};
use parking_lot::Mutex;
use serde::Deserialize;
use velopack::bundle::Manifest;
use velopack::download::{self, HttpHeader, HttpOptions};
use velopack::sources::{self, UpdateSource};
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, VelopackAsset, VelopackAssetFeed};

use crate::agent::markdown::{self, MdOut};
use crate::kit::{self, icon};
use crate::theme::{Theme, icon_font, semibold, ui as ui_font};

/// The GitHub repository that releases are published to.
pub const REPO: &str = "spatney/graysense-terminal";
/// How often automatic updates look for a new version.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// The first automatic check, once startup has settled.
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// When a failed automatic check or download is retried.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
const API_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Not an installed copy (a build from source), so there's nothing to update.
    Unsupported,
    Idle,
    Checking,
    UpToDate,
    /// A newer version is out. With automatic updates off, it waits for the user.
    Available,
    /// Downloading the newer version (0 to 1).
    Downloading(f32),
    /// Downloaded: it installs when Graysense restarts or closes.
    Ready,
    Failed(String),
}

/// What the user asked for in the Updates section.
pub enum UiAction {
    Check,
    Download,
    Restart,
    SetAuto(bool),
    OpenUrl(String),
}

struct Checked {
    manager: UpdateManager,
    via: String,
    update: Option<UpdateInfo>,
}

enum Msg {
    Checked(Result<Checked, String>),
    Progress(i16),
    Downloaded(Result<(), String>),
}

pub struct Updater {
    /// The running version.
    pub version: String,
    pub phase: Phase,
    /// The newer version and its release notes (Markdown), once a check finds one.
    pub next: Option<(String, String)>,
    checked: Option<SystemTime>,
    /// How the last successful check signed in ("GitHub CLI (octocat)").
    via: Option<String>,
    manager: Option<UpdateManager>,
    info: Option<UpdateInfo>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    next_auto: Option<Instant>,
    /// The update is being installed (a restart was requested, or Graysense is closing).
    applying: bool,
    ctx: egui::Context,
}

impl Updater {
    pub fn new(ctx: egui::Context, auto: bool) -> Self {
        let (tx, rx) = channel();
        let mut u = Updater {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            phase: Phase::Unsupported,
            next: None,
            checked: None,
            via: None,
            manager: None,
            info: None,
            tx,
            rx,
            next_auto: None,
            applying: false,
            ctx,
        };
        // Only an installed copy (with Update.exe beside its program folder) can update itself.
        let Ok(um) = UpdateManager::new(sources::NoneSource {}, None, None) else { return u };
        u.version = um.get_current_version_as_string();
        u.phase = Phase::Idle;
        if let Some(asset) = um.get_update_pending_restart() {
            u.next = Some((asset.Version.clone(), asset.NotesMarkdown.clone()));
            u.info = Some(UpdateInfo { TargetFullRelease: asset, ..Default::default() });
            u.phase = Phase::Ready;
        }
        u.manager = Some(um);
        if auto {
            u.next_auto = Some(Instant::now() + FIRST_CHECK);
        }
        u
    }

    pub fn installed(&self) -> bool {
        self.phase != Phase::Unsupported
    }

    /// A downloaded update is waiting to be installed.
    pub fn ready(&self) -> bool {
        self.phase == Phase::Ready && !self.applying
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Downloading(_)) || self.applying
    }

    /// The user turned automatic updates on or off.
    pub fn set_auto(&mut self, on: bool) {
        if !self.installed() {
            return;
        }
        self.next_auto = on.then(|| Instant::now() + Duration::from_secs(2));
        if on && self.phase == Phase::Available {
            self.download();
        }
    }

    /// Look for a newer version in the background.
    pub fn check(&mut self) {
        if !self.installed() || self.busy() || self.phase == Phase::Ready {
            return;
        }
        self.phase = Phase::Checking;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let spawned = std::thread::Builder::new().name("graysense-update".into()).spawn(move || {
            let _ = tx.send(Msg::Checked(check_now()));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.phase = Phase::Failed("Couldn't start the update check.".into());
        }
    }

    /// Download the version the last check found.
    pub fn download(&mut self) {
        if self.busy() || self.phase == Phase::Ready {
            return;
        }
        let (Some(um), Some(info)) = (self.manager.clone(), self.info.clone()) else { return };
        self.phase = Phase::Downloading(0.0);
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let spawned = std::thread::Builder::new().name("graysense-update".into()).spawn(move || {
            let (progress, updates) = channel::<i16>();
            let forward = {
                let (tx, ctx) = (tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    for p in updates {
                        let _ = tx.send(Msg::Progress(p));
                        ctx.request_repaint();
                    }
                })
            };
            let result = um.download_updates(&info, Some(progress)).map_err(|e| e.to_string());
            let _ = forward.join();
            let _ = tx.send(Msg::Downloaded(result));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.phase = Phase::Failed("Couldn't start the download.".into());
        }
    }

    /// Handle finished background work and run automatic checks. Returns a version that just
    /// finished downloading.
    pub fn tick(&mut self, auto: bool) -> Option<String> {
        let mut ready = None;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Checked(Ok(c)) => {
                    self.checked = Some(SystemTime::now());
                    self.via = Some(c.via);
                    self.manager = Some(c.manager);
                    self.next_auto = auto.then(|| Instant::now() + CHECK_EVERY);
                    match c.update {
                        Some(info) => {
                            let target = &info.TargetFullRelease;
                            self.next = Some((target.Version.clone(), target.NotesMarkdown.clone()));
                            self.info = Some(info);
                            self.phase = Phase::Available;
                            if auto {
                                self.download();
                            }
                        }
                        None => {
                            self.next = None;
                            self.info = None;
                            self.phase = Phase::UpToDate;
                        }
                    }
                }
                Msg::Checked(Err(e)) => {
                    self.phase = Phase::Failed(e);
                    self.next_auto = auto.then(|| Instant::now() + RETRY_AFTER);
                }
                Msg::Progress(p) => {
                    if matches!(self.phase, Phase::Downloading(_)) {
                        self.phase = Phase::Downloading(f32::from(p.clamp(0, 100)) / 100.0);
                    }
                }
                Msg::Downloaded(Ok(())) => {
                    self.phase = Phase::Ready;
                    ready = self.next.as_ref().map(|(v, _)| v.clone());
                }
                Msg::Downloaded(Err(e)) => {
                    self.phase = Phase::Failed(format!("Couldn't download the update. {e}"));
                    self.next_auto = auto.then(|| Instant::now() + RETRY_AFTER);
                }
            }
        }
        if auto && !self.busy() && self.next_auto.is_some_and(|t| Instant::now() >= t) {
            self.next_auto = None;
            self.check();
        }
        ready
    }

    /// Install the downloaded update and reopen Graysense. The caller then closes the window
    /// (which saves the workspace); the installer waits for this process to exit.
    pub fn restart(&mut self) -> Result<(), String> {
        let (Some(um), Some(info)) = (&self.manager, &self.info) else { return Err("No update is ready to install.".into()) };
        if !self.ready() {
            return Err("No update is ready to install.".into());
        }
        um.wait_exit_then_apply_updates(&info.TargetFullRelease, false, true, Vec::<String>::new()).map_err(|e| e.to_string())?;
        self.applying = true;
        Ok(())
    }

    /// Graysense is closing: install a downloaded update once it has exited.
    pub fn on_exit(&mut self) {
        if !self.ready() {
            return;
        }
        let (Some(um), Some(info)) = (&self.manager, &self.info) else { return };
        match um.wait_exit_then_apply_updates(&info.TargetFullRelease, true, false, Vec::<String>::new()) {
            Ok(()) => {
                tracing::info!("installing Graysense {} after exit", info.TargetFullRelease.Version);
                self.applying = true;
            }
            Err(e) => tracing::warn!("couldn't install the update on exit: {e}"),
        }
    }

    /// The GitHub release page for the running version.
    pub fn notes_url(&self) -> String {
        format!("https://github.com/{REPO}/releases/tag/v{}", self.version)
    }
}

fn check_now() -> Result<Checked, String> {
    if let Some(feed) = std::env::var("GRAYSENSE_UPDATE_FEED").ok().map(|f| f.trim().to_owned()).filter(|f| !f.is_empty()) {
        let um = UpdateManager::new(sources::AutoSource::new(&feed), None, None).map_err(|e| e.to_string())?;
        let update = available(um.check_for_updates().map_err(|e| e.to_string())?);
        return Ok(Checked { manager: um, via: feed, update });
    }
    let mut signed_in = Vec::new();
    let mut last = String::new();
    for cred in credentials() {
        let source = GithubReleases { repo: REPO.to_owned(), token: cred.token.clone(), urls: Mutex::default() };
        let um = UpdateManager::new(source, None, None).map_err(|e| e.to_string())?;
        match um.check_for_updates() {
            Ok(check) => return Ok(Checked { manager: um, via: cred.label, update: available(check) }),
            Err(e) => {
                tracing::info!("update check as {} failed: {e}", cred.label);
                if cred.token.is_some() {
                    signed_in.push(cred.label);
                }
                last = e.to_string();
            }
        }
    }
    Err(explain(&last, &signed_in))
}

fn available(check: UpdateCheck) -> Option<UpdateInfo> {
    match check {
        UpdateCheck::UpdateAvailable(info) => Some(*info),
        UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => None,
    }
}

/// A short, actionable reason why a check failed. `signed_in` names the sign-ins that were tried.
fn explain(error: &str, signed_in: &[String]) -> String {
    let status = error.split("http status: ").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|s| s.parse::<u16>().ok());
    match status {
        Some(401 | 403 | 404) if signed_in.is_empty() => "Graysense's releases are private. Sign in with the GitHub CLI (gh auth login) to get updates.".into(),
        Some(401 | 403 | 404) => format!("{} can't see {REPO}. Sign in to the GitHub CLI with an account that can (gh auth login).", signed_in.join(", ")),
        Some(code) => format!("GitHub returned an error ({code}). Try again later."),
        None if error.starts_with("Network error") || error.starts_with("IO error") => "Couldn't reach GitHub. Check your connection and try again.".into(),
        None => error.to_owned(),
    }
}

/// A way to sign in to GitHub for update checks.
struct Credential {
    /// Shown in Settings ("GitHub CLI (octocat)"), never the token itself.
    label: String,
    token: Option<String>,
}

/// Sign-ins to try, best first: explicit tokens, each GitHub CLI account (the active one first),
/// then none, which is enough for a public repository.
fn credentials() -> Vec<Credential> {
    let mut out: Vec<Credential> = Vec::new();
    let mut add = |label: String, token: Option<String>| {
        let token = token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        if token.is_some() && !out.iter().any(|c| c.token == token) {
            out.push(Credential { label, token });
        }
    };
    for var in ["GRAYSENSE_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        add(var.to_owned(), std::env::var(var).ok());
    }
    if let Some(gh) = gh_exe() {
        let status = run(&gh, &["auth", "status", "--hostname", "github.com"]).map(|o| String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)).unwrap_or_default();
        let token = |args: &[&str]| run(&gh, args).filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        for login in gh_accounts(&status) {
            add(format!("GitHub CLI ({login})"), token(&["auth", "token", "--hostname", "github.com", "--user", &login]));
        }
        // GitHub CLI before 2.40 has no --user; this gets its one account.
        add("GitHub CLI".into(), token(&["auth", "token", "--hostname", "github.com"]));
    }
    out.push(Credential { label: "no sign-in".into(), token: None });
    out
}

/// The GitHub CLI: on PATH, or where its installers put it (an app started before gh was
/// installed can have an older PATH).
fn gh_exe() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "gh.exe" } else { "gh" };
    let on_path: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join(exe)).collect()).unwrap_or_default();
    let known = [
        std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("GitHub CLI").join(exe)),
        dirs::data_local_dir().map(|p| p.join("Programs").join("GitHub CLI").join(exe)),
    ];
    on_path.into_iter().chain(known.into_iter().flatten()).find(|p| p.is_file())
}

/// Runs a console program without flashing a window.
fn run(exe: &Path, args: &[&str]) -> Option<Output> {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output().ok()
}

/// GitHub CLI logins in `gh auth status` output, the active account first.
fn gh_accounts(status: &str) -> Vec<String> {
    let mut accounts: Vec<(String, bool)> = Vec::new();
    for line in status.lines().map(str::trim) {
        let login = line
            .split_once("Logged in to github.com account ")
            .or_else(|| line.split_once("Logged in to github.com as "))
            .and_then(|(_, rest)| rest.split_whitespace().next());
        if let Some(login) = login {
            accounts.push((login.to_owned(), false));
        } else if line.starts_with("- Active account: true")
            && let Some(last) = accounts.last_mut()
        {
            last.1 = true;
        }
    }
    accounts.sort_by_key(|(_, active)| !active);
    accounts.into_iter().map(|(login, _)| login).collect()
}

/// GitHub Releases as an update feed. Unlike Velopack's own GitHub source, this skips drafts,
/// which the API also lists for accounts that can push: the owner's copy would otherwise install
/// a release before it's published.
struct GithubReleases {
    repo: String,
    token: Option<String>,
    /// Package file name (lowercase) → its asset's API URL, from the last feed read.
    urls: Mutex<HashMap<String, String>>,
}

#[derive(Deserialize)]
struct GhRelease {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    /// API URL: with `Accept: application/octet-stream` it serves the file, also for private repos.
    url: String,
}

/// Releases users should get: published, and not marked as pre-releases.
fn published(releases: &[GhRelease]) -> impl Iterator<Item = &GhRelease> {
    releases.iter().filter(|r| !r.draft && !r.prerelease)
}

impl GithubReleases {
    fn options(&self, accept: &str, timeout: Duration) -> HttpOptions {
        let header = |name: &str, value: String| HttpHeader { Name: name.into(), Value: value };
        let mut headers = vec![header("Accept", accept.into()), header("X-GitHub-Api-Version", "2022-11-28".into())];
        if let Some(token) = &self.token {
            headers.push(header("Authorization", format!("Bearer {token}")));
        }
        HttpOptions { Headers: headers, TimeoutMilliseconds: timeout.as_millis() as u64 }
    }
}

impl UpdateSource for GithubReleases {
    fn get_release_feed(&self, channel: &str, _app: &Manifest, _staged_user_id: &str) -> Result<VelopackAssetFeed, velopack::Error> {
        let list = format!("https://api.github.com/repos/{}/releases?per_page=20", self.repo);
        let releases: Vec<GhRelease> = serde_json::from_str(&download::download_url_as_string(&list, Some(&self.options("application/vnd.github+json", API_TIMEOUT)))?)?;
        let feed_name = format!("releases.{channel}.json");
        let (mut assets, mut urls) = (Vec::new(), HashMap::new());
        for release in published(&releases) {
            let Some(feed) = release.assets.iter().find(|a| a.name.eq_ignore_ascii_case(&feed_name)) else { continue };
            let json = download::download_url_as_string(&feed.url, Some(&self.options("application/octet-stream", API_TIMEOUT)))?;
            let feed: VelopackAssetFeed = serde_json::from_str(&json)?;
            // Keep the packages this release actually carries.
            for asset in feed.Assets {
                let key = asset.FileName.to_ascii_lowercase();
                if urls.contains_key(&key) {
                    continue;
                }
                if let Some(file) = release.assets.iter().find(|a| a.name.eq_ignore_ascii_case(&asset.FileName)) {
                    urls.insert(key, file.url.clone());
                    assets.push(asset);
                }
            }
        }
        *self.urls.lock() = urls;
        Ok(VelopackAssetFeed { Assets: assets })
    }

    fn download_release_entry(&self, asset: &VelopackAsset, local_file: &Path, progress: Option<Sender<i16>>) -> Result<(), velopack::Error> {
        let url = self.urls.lock().get(&asset.FileName.to_ascii_lowercase()).cloned();
        let url = url.ok_or_else(|| velopack::Error::Other(format!("{} isn't in a published release.", asset.FileName)))?;
        download::download_url_to_file(&url, local_file, Some(&self.options("application/octet-stream", DOWNLOAD_TIMEOUT)), |p| {
            if let Some(tx) = &progress {
                let _ = tx.send(p);
            }
        })
    }
}

// ------------------------------------------------------------------ UI

fn ago(at: SystemTime) -> String {
    match at.elapsed().map(|d| d.as_secs()).unwrap_or(0) {
        0..=59 => "just now".into(),
        s @ 60..=3599 => format!("{} min ago", s / 60),
        s @ 3600..=86_399 => format!("{} h ago", s / 3600),
        86_400..=172_799 => "yesterday".into(),
        s => format!("{} days ago", s / 86_400),
    }
}

/// A status line: icon (or spinner) and text, with an optional faint detail line.
fn status(ui: &mut Ui, glyph: Option<(&str, Color32)>, text: &str, detail: Option<&str>, theme: &Theme, t: f32) {
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
        match glyph {
            Some((g, color)) => {
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, g, icon_font(13.0), color);
            }
            None => {
                kit::spinner(ui.painter(), r.center(), 5.5, t, theme.accent);
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(text).font(ui_font(12.5)).color(theme.text)).wrap());
            if let Some(d) = detail {
                ui.add(egui::Label::new(RichText::new(d).font(ui_font(11.5)).color(theme.text_faint)).wrap());
            }
        });
    });
}

/// The Updates part of Settings.
pub fn settings_ui(ui: &mut Ui, u: &Updater, auto: bool, theme: &Theme, t: f32) -> Option<UiAction> {
    let mut act = None;
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        kit::mark(ui.painter(), r.center(), 32.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new("Graysense").font(semibold(13.5)).color(theme.text));
            let build = if u.installed() { format!("Version {}", u.version) } else { format!("Version {} · built from source", u.version) };
            ui.label(RichText::new(build).font(ui_font(11.5)).color(theme.text_dim));
        });
    });
    ui.add_space(6.0);
    let next = u.next.as_ref().map(|(v, _)| v.as_str()).unwrap_or("The update");
    let checked = u.checked.map(|at| match &u.via {
        Some(via) if via != "no sign-in" => format!("Checked {} · {via}", ago(at)),
        _ => format!("Checked {}", ago(at)),
    });
    match &u.phase {
        Phase::Unsupported => {
            ui.add(egui::Label::new(RichText::new("This copy was built from source, so it doesn't update itself. Install Graysense from its releases to get updates.").font(ui_font(11.5)).color(theme.text_faint)).wrap());
            if kit::ghost_button(ui, "Open releases", theme).clicked() {
                act = Some(UiAction::OpenUrl(format!("https://github.com/{REPO}/releases")));
            }
        }
        Phase::Idle => {
            if kit::ghost_button(ui, "Check for updates", theme).clicked() {
                act = Some(UiAction::Check);
            }
        }
        Phase::Checking => status(ui, None, "Checking for updates…", None, theme, t),
        Phase::UpToDate => {
            status(ui, Some((icon::CHECK, theme.ok)), "You're up to date", checked.as_deref(), theme, t);
            if kit::ghost_button(ui, "Check for updates", theme).clicked() {
                act = Some(UiAction::Check);
            }
        }
        Phase::Available => {
            status(ui, Some((icon::DOWNLOAD, theme.accent)), &format!("Graysense {next} is available"), checked.as_deref(), theme, t);
            if kit::primary_button(ui, "Download update", theme).clicked() {
                act = Some(UiAction::Download);
            }
        }
        Phase::Downloading(p) => {
            status(ui, None, &format!("Downloading Graysense {next}…  {:.0}%", p * 100.0), None, theme, t);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
            kit::progress(ui.painter(), r, *p, theme.accent, theme.surface3);
        }
        Phase::Ready => {
            status(ui, Some((icon::UPDATE, theme.accent)), &format!("Graysense {next} is ready to install"), Some("It installs when you restart or close Graysense."), theme, t);
            if kit::primary_button(ui, "Restart to update", theme).clicked() {
                act = Some(UiAction::Restart);
            }
        }
        Phase::Failed(e) => {
            status(ui, Some((icon::WARNING, theme.warn)), e, None, theme, t);
            if kit::ghost_button(ui, "Try again", theme).clicked() {
                act = Some(UiAction::Check);
            }
        }
    }
    if let Some((v, notes)) = &u.next
        && matches!(u.phase, Phase::Available | Phase::Downloading(_) | Phase::Ready)
        && !notes.trim().is_empty()
    {
        ui.add_space(2.0);
        egui::CollapsingHeader::new(RichText::new(format!("What's new in {v}")).font(ui_font(12.0)).color(theme.text_dim)).id_salt("update-notes").show(ui, |ui| {
            let mut md = MdOut { open_url: None, copy: None };
            markdown::render(ui, notes, theme, &mut md);
            if let Some(url) = md.open_url {
                act = Some(UiAction::OpenUrl(url));
            }
        });
    }
    if u.installed() {
        ui.add_space(6.0);
        let mut on = auto;
        if kit::toggle_row(ui, &mut on, theme, "Update automatically").changed() {
            act = Some(UiAction::SetAuto(on));
        }
        ui.add(egui::Label::new(RichText::new("Downloads new versions in the background. They install when you restart or close Graysense.").font(ui_font(11.5)).color(theme.text_faint)).wrap());
        if ui.add(egui::Label::new(RichText::new("Release notes for this version").font(ui_font(11.5)).color(theme.accent)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            act = Some(UiAction::OpenUrl(u.notes_url()));
        }
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_cli_accounts_put_the_active_one_first() {
        let status = "github.com\n  ✓ Logged in to github.com account work-user (keyring)\n  - Active account: false\n  - Git operations protocol: https\n\n  ✓ Logged in to github.com account octocat (keyring)\n  - Active account: true\n  X Failed to log in to github.com account stale (keyring)\n";
        assert_eq!(gh_accounts(status), ["octocat", "work-user"]);
        assert_eq!(gh_accounts("github.com\n  ✓ Logged in to github.com as octocat (oauth_token)\n"), ["octocat"]);
        assert!(gh_accounts("You are not logged into any GitHub hosts. To log in, run: gh auth login").is_empty());
    }

    #[test]
    fn check_failures_say_what_to_do() {
        let not_found = "Network error: Http error: http status: 404";
        assert!(explain(not_found, &[]).contains("gh auth login"));
        assert!(explain(not_found, &["GitHub CLI (octocat)".into()]).starts_with("GitHub CLI (octocat) can't see"));
        assert_eq!(explain("Network error: Http error: http status: 502", &[]), "GitHub returned an error (502). Try again later.");
        assert!(explain("Network error: Http error: io: Connection refused", &[]).starts_with("Couldn't reach GitHub"));
    }

    #[test]
    fn drafts_and_prereleases_are_skipped() {
        let releases: Vec<GhRelease> = serde_json::from_str(
            r#"[{"draft": true, "prerelease": false, "assets": []},
                {"draft": false, "prerelease": true, "assets": []},
                {"draft": false, "prerelease": false, "assets": [{"name": "releases.win.json", "url": "https://api.github.com/x"}]}]"#,
        )
        .unwrap();
        let kept: Vec<_> = published(&releases).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].assets[0].name, "releases.win.json");
    }
}
