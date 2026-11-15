//! GitHub sync in the engine: connecting to a repository, running each sync (fetch, merge, apply,
//! ask the AI, push) and deciding when to sync by itself. `crate::sync` has the rules; this is the
//! order things happen in and the state the UI sees.
//!
//! Blocking work (git, the network) runs on a worker thread that reports through a channel `tick`
//! polls. The merge and applying it run on the core's thread: it's all in memory and quick. The AI
//! step goes through the backend, like learning does.
//!
//! A sync is: fetch → merge with what this PC has (applying what changed here) → the AI compares
//! what's new on both PCs → push. If someone pushed in between, start again from the fetch; nothing
//! is lost, because a merge applied here only makes this PC match what the repository holds.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use serde_json::json;

use super::dto::{SyncDto, SyncPendingDto};
use super::{BayKind, Core};
use crate::agent::Level;
use crate::agent::backend::{ClientState, Cmd};
use crate::memory::{self, Memory, MemoryId, Project, Scope, Source};
use crate::settings::Settings;
use crate::sync::git::{self, Connected, Error, Fetched, Pushed, Remote, Signin};
use crate::sync::items::Snapshot;
use crate::sync::merge::{self, Inputs, Strategy};
use crate::sync::projects::Projects;
use crate::sync::reconcile::{self, Side};
use crate::sync::{self, Config, Counts, Options, Report, State};

/// How long after Graysense opens the first sync waits (everything else is starting too).
const STARTUP_DELAY: Duration = Duration::from_secs(20);
/// How long local changes sit before they're synced, so a burst of edits is one commit.
const DEBOUNCE: Duration = Duration::from_secs(20);
/// How often to look at the repository when nothing here changed.
const POLL: Duration = Duration::from_secs(15 * 60);
/// How often to check whether this PC has changes the repository doesn't.
const CHECK: Duration = Duration::from_secs(4);
/// After a failed background sync, how long to leave it alone.
const BACKOFF: Duration = Duration::from_secs(5 * 60);
/// After "not now" to removing most of what was synced, how long background syncs leave it alone.
const SNOOZE: Duration = Duration::from_secs(60 * 60);
/// After a sync that left note files for later, how long until it goes on.
const MORE_FILES: Duration = Duration::from_secs(60);
/// How long the AI step may take before the sync goes on without it.
const AI_LIMIT: Duration = Duration::from_secs(4 * 60);
/// How long a git or network step may take before the sync gives up on it. Every command has a
/// limit of its own, so this only catches something that has stopped answering altogether.
const WORKER_LIMIT: Duration = Duration::from_secs(15 * 60);
/// A push that loses a race starts over this many times.
const ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Connecting,
    Fetching,
    Merging,
    Reconciling,
    Pushing,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Connecting => "connecting",
            Phase::Fetching => "fetching",
            Phase::Merging => "merging",
            Phase::Reconciling => "reconciling",
            Phase::Pushing => "pushing",
        }
    }
}

/// What a worker thread reports.
enum Done {
    Connected(Result<(Connected, Fetched), Error>),
    Fetched(Result<(Fetched, Signin), Error>),
    Pushed(Result<Pushed, Error>),
}

/// What a sync that was asked about removals goes on to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Choice {
    /// Put back what's missing: save it again, or bring it back here.
    Keep,
    /// Go ahead and remove it.
    Remove,
}

/// A question waiting for the user.
enum Pending {
    Public { input: String, create: bool },
    /// The repository and this PC both hold data. Nothing is saved until the user answers, so
    /// closing Graysense meanwhile leaves it disconnected.
    Merge { config: Config, signin: Signin, remote: Counts, local: Counts, from: Option<(String, i64)> },
    /// Most of what was synced is gone from one side. Nothing has been changed: answering starts
    /// the sync again with the answer.
    Removals { here: usize, there: usize, known: usize },
}

/// One run: a connect or a sync.
struct Run {
    phase: Phase,
    /// The user asked for it, so it speaks up when it's done.
    manual: bool,
    strategy: Strategy,
    attempt: u32,
    /// What the user said about removing most of what was synced, once asked.
    choice: Option<Choice>,
    /// What this run includes (preferences wait until Copilot's models are known).
    options: Options,
    /// The worker that's running, if any, and when it started.
    rx: Option<Receiver<Done>>,
    worker_since: Instant,
    fetched: Option<Fetched>,
    report: Report,
    /// A connect: what was typed, whether to make it, and whether a public repository was accepted.
    connect: Option<(String, bool, bool)>,
    /// The AI's requests in flight: job → the memories it was shown.
    jobs: HashMap<u64, Vec<MemoryId>>,
    answers: Vec<(Vec<MemoryId>, Result<String, String>)>,
    deadline: Option<Instant>,
    /// What's being pushed (it becomes the base once it is) and where.
    pushing: Option<(Snapshot, String)>,
    /// What this PC couldn't share when it pushed: it keeps the base entry it had.
    withheld: HashSet<String>,
    /// Note files from the repository were left for the next sync (a sync copies only so many).
    more_files: bool,
}

impl Run {
    fn new(phase: Phase, manual: bool, strategy: Strategy, attempt: u32, options: Options) -> Run {
        Run { phase, manual, strategy, attempt, choice: None, options, rx: None, worker_since: Instant::now(), fetched: None, report: Report::default(), connect: None, jobs: HashMap::new(), answers: Vec::new(), deadline: None, pushing: None, withheld: HashSet::new(), more_files: false }
    }

    /// Wait for a worker's answer.
    fn watch(&mut self, rx: Receiver<Done>) {
        self.rx = Some(rx);
        self.worker_since = Instant::now();
    }
}

pub struct Syncer {
    pub config: Config,
    pub state: State,
    device: String,
    git: Option<PathBuf>,
    git_checked: Instant,
    /// The sign-in that worked last (in memory only: tokens are never written down).
    signin: Option<Signin>,
    run: Option<Run>,
    pending: Option<Pending>,
    born: Instant,
    started: bool,
    checked: Instant,
    changes: bool,
    fingerprint: String,
    changed_at: Option<Instant>,
    next_poll: Instant,
    backoff: Option<Instant>,
    next_job: u64,
    /// The sync state changed but couldn't be saved yet (see `sync_save`).
    save_due: bool,
}

impl Syncer {
    pub fn new() -> Syncer {
        let (config, state) = sync::load();
        let now = Instant::now();
        Syncer {
            config,
            state,
            device: sync::device_name(),
            git: git::git_exe(),
            git_checked: now,
            signin: None,
            run: None,
            pending: None,
            born: now,
            started: false,
            checked: now - CHECK,
            changes: false,
            fingerprint: String::new(),
            changed_at: None,
            next_poll: now + POLL,
            backoff: None,
            next_job: 1,
            save_due: false,
        }
    }
}

/// What this PC has to sync.
struct Local {
    /// What would go in the repository.
    snapshot: Snapshot,
    /// The folders this PC knows, to read scopes both ways.
    projects: Projects,
    /// What this PC has but can't share (it looks like it holds a secret): the repository's copy of
    /// these, if it has one, is left alone.
    withheld: HashSet<String>,
}

fn withheld_note(withheld: &HashSet<String>) -> Option<String> {
    let n = withheld.len();
    (n > 0).then(|| if n == 1 { "Not saved to GitHub: 1 item that looks like it holds a secret.".to_owned() } else { format!("Not saved to GitHub: {n} items that look like they hold a secret.") })
}

/// "brought in 3 · saved 2 files to GitHub …": what a sync did, for a toast.
fn describe(r: &Report) -> String {
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if r.pulled > 0 {
        parts.push(format!("brought in {}", r.pulled));
    }
    if r.removed > 0 {
        parts.push(format!("removed {}", r.removed));
    }
    if r.pushed > 0 {
        parts.push(format!("saved {} to GitHub", plural(r.pushed, "file", "files")));
    }
    if r.combined > 0 {
        parts.push(format!("combined {}", plural(r.combined, "duplicate", "duplicates")));
    }
    if r.tidied > 0 {
        parts.push(format!("AI merged {}", r.tidied));
    }
    if parts.is_empty() { "everything was already up to date".to_owned() } else { parts.join(", ") }
}

impl Core {
    // ------------------------------------------------------------------ what the UI sees

    pub(super) fn sync_dto(&self) -> SyncDto {
        let s = &self.sync;
        let c = &s.config;
        SyncDto {
            git: s.git.is_some(),
            repo: c.repo.clone(),
            url: c.repo.as_ref().map(|r| format!("https://github.com/{r}")),
            account: c.account.clone(),
            device: s.device.clone(),
            phase: s.run.as_ref().map_or("idle", |r| r.phase.name()),
            busy: s.run.is_some(),
            last_sync: s.state.last_sync,
            last_error: s.state.last_error.clone(),
            report: s.state.report.as_ref().map(Into::into),
            pending: s.pending.as_ref().map(|p| match p {
                Pending::Public { input, .. } => SyncPendingDto::Public { repo: sync::git::parse_repo(input).map(|(o, n)| o.map_or(n.clone(), |o| format!("{o}/{n}"))).unwrap_or_else(|| input.clone()) },
                Pending::Merge { config, remote, local, from, .. } => SyncPendingDto::Merge { repo: config.repo.clone().unwrap_or_default(), remote: (*remote).into(), local: (*local).into(), from: from.as_ref().map(|f| f.0.clone()), at: from.as_ref().map(|f| f.1) },
                Pending::Removals { here, there, known } => SyncPendingDto::Removals { repo: c.repo.clone().unwrap_or_default(), here: *here, there: *there, known: *known },
            }),
            auto: c.auto,
            files: c.files,
            settings: c.settings,
            missions: c.missions,
            changes: s.changes,
        }
    }

    fn sync_save(&mut self) {
        // Never ahead of the memories and missions it describes: if Graysense stopped in between,
        // what the base says was merged would read as deleted here. Until they're saved, it waits.
        self.memory.lock().save_if_dirty();
        self.missions.save_if_dirty();
        let saved = !self.memory.lock().is_dirty() && !self.missions.is_dirty() && sync::save(&self.sync.config, &self.sync.state);
        self.sync.save_due = !saved;
    }

    /// Save the sync state if an earlier save had to wait.
    pub(super) fn sync_save_if_due(&mut self) {
        if self.sync.save_due {
            self.sync_save();
        }
    }

    // ------------------------------------------------------------------ what the user asks for

    /// Connect to a repository (or make a new private one) and take the first sync.
    pub fn sync_connect(&mut self, input: String, create: bool, allow_public: bool) -> Result<(), String> {
        if self.sync.run.is_some() {
            return Err("Sync is already running.".into());
        }
        if self.sync.config.connected() {
            return Err("Disconnect from the current repository first.".into());
        }
        let git = self.sync_git()?;
        self.sync.pending = None;
        self.sync.state.last_error = None;
        let device = self.sync.device.clone();
        let (tx, rx) = channel();
        let typed = input.clone();
        std::thread::spawn(move || {
            let result = git::connect(&typed, create, allow_public).and_then(|connected| {
                let remote = Remote::new(git, &connected.repo, &connected.branch, &connected.signin.token, &device);
                git::fetch(&remote).map(|fetched| (connected, fetched))
            });
            let _ = tx.send(Done::Connected(result));
        });
        let mut run = Run::new(Phase::Connecting, true, Strategy::Merge, 1, self.sync_options_now(&Config::default()));
        run.watch(rx);
        run.connect = Some((input, create, allow_public));
        self.sync.run = Some(run);
        Ok(())
    }

    /// The user's answer to what sync is waiting on.
    pub fn sync_answer(&mut self, choice: &str) -> Result<(), String> {
        match (self.sync.pending.take(), choice) {
            (Some(Pending::Public { input, create }), "public") => self.sync_connect(input, create, true),
            (Some(Pending::Public { .. }), _) => Ok(()),
            (Some(Pending::Merge { config, signin, .. }), "merge") => {
                self.sync_adopt(config, signin);
                self.sync_begin(true, Strategy::Merge)
            }
            (Some(Pending::Merge { config, signin, .. }), "replace") => {
                self.sync_adopt(config, signin);
                self.sync_begin(true, Strategy::UseRemote)
            }
            (Some(Pending::Merge { .. }), _) => {
                let _ = std::fs::remove_dir_all(sync::repo_dir());
                Ok(())
            }
            (Some(Pending::Removals { .. }), "keep") => self.sync_fetch(true, Strategy::Merge, 1, Some(Choice::Keep)),
            (Some(Pending::Removals { .. }), "remove") => self.sync_fetch(true, Strategy::Merge, 1, Some(Choice::Remove)),
            // Not now: nothing was changed. Background syncs leave it alone for a while, and "Sync now" asks again.
            (Some(Pending::Removals { .. }), _) => {
                self.sync.backoff = Some(Instant::now() + SNOOZE);
                Ok(())
            }
            (None, _) => Err("Sync isn't waiting for an answer.".into()),
        }
    }

    pub fn sync_now(&mut self) -> Result<(), String> {
        if self.sync.pending.is_some() {
            return Err("Answer the question about the repository first.".into());
        }
        self.sync_begin(true, Strategy::Merge)
    }

    /// Stop syncing: this PC's memory and the repository are both left as they are.
    pub fn sync_disconnect(&mut self) -> Result<(), String> {
        if self.sync.run.is_some() {
            return Err("Sync is running. Try again when it's done.".into());
        }
        self.sync.config = Config::default();
        self.sync.state = State::default();
        self.sync.pending = None;
        self.sync.signin = None;
        self.sync.changes = false;
        self.sync.changed_at = None;
        let _ = std::fs::remove_dir_all(sync::repo_dir());
        self.sync_save();
        self.toast(Level::Ok, "Disconnected from GitHub. Your memories are still here, and the repository is as it was.");
        Ok(())
    }

    pub fn sync_options(&mut self, auto: Option<bool>, files: Option<bool>, settings: Option<bool>, missions: Option<bool>) {
        let c = &mut self.sync.config;
        let before = (c.files, c.settings, c.missions);
        if let Some(v) = auto {
            c.auto = v;
        }
        if let Some(v) = files {
            c.files = v;
        }
        if let Some(v) = settings {
            c.settings = v;
        }
        if let Some(v) = missions {
            c.missions = v;
        }
        // What's now included goes up soon.
        if c.connected() && c.auto && (c.files, c.settings, c.missions) != before {
            self.sync.next_poll = Instant::now();
        }
        self.sync_save();
    }

    fn sync_git(&mut self) -> Result<PathBuf, String> {
        if self.sync.git.is_none() {
            self.sync.git = git::git_exe();
        }
        self.sync.git.clone().ok_or_else(|| Error::NoGit.message())
    }

    /// What a sync includes right now. Preferences wait until Copilot's models are known: a synced
    /// default model can't be checked against what this account offers before then, and leaving
    /// this PC's value out of the push keeps the repository's.
    fn sync_options_now(&self, config: &Config) -> Options {
        let o = config.options();
        Options { settings: o.settings && !self.models.is_empty(), ..o }
    }

    // ------------------------------------------------------------------ a sync

    /// Start a sync: fetch first, the rest follows as results come in.
    fn sync_begin(&mut self, manual: bool, strategy: Strategy) -> Result<(), String> {
        if self.sync.run.is_some() {
            return Err("Sync is already running.".into());
        }
        if !self.sync.config.connected() {
            return Err("Choose a repository first.".into());
        }
        self.sync_fetch(manual, strategy, 1, None)
    }

    fn sync_fetch(&mut self, manual: bool, strategy: Strategy, attempt: u32, choice: Option<Choice>) -> Result<(), String> {
        let git = self.sync_git()?;
        let Some(repo) = self.sync.config.repo.clone() else { return Err("Choose a repository first.".into()) };
        let branch = self.sync.config.branch.clone().unwrap_or_else(|| "main".into());
        let allow_public = self.sync.config.allow_public;
        let cached = self.sync.signin.clone();
        let device = self.sync.device.clone();
        self.sync.state.last_error = None;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(Done::Fetched(fetch_job(git, &repo, &branch, allow_public, cached, &device)));
        });
        let mut run = Run::new(Phase::Fetching, manual, strategy, attempt, self.sync_options_now(&self.sync.config));
        run.choice = choice;
        run.watch(rx);
        self.sync.run = Some(run);
        Ok(())
    }

    /// A sync is in progress (it may be changing memory).
    pub(super) fn sync_running(&self) -> bool {
        self.sync.run.is_some()
    }

    /// Worker results, and an AI step that's taking too long.
    pub(super) fn sync_tick(&mut self) {
        // Git installed while Graysense is open is noticed (the card asks for it until it's there).
        if self.sync.git.is_none() && self.sync.git_checked.elapsed() >= Duration::from_secs(5) {
            self.sync.git_checked = Instant::now();
            self.sync.git = git::git_exe();
        }
        let polled = self.sync.run.as_ref().and_then(|r| r.rx.as_ref()).map(Receiver::try_recv);
        match polled {
            Some(Ok(done)) => {
                if let Some(run) = self.sync.run.take() {
                    self.sync_done(run, done);
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                if let Some(run) = self.sync.run.take() {
                    self.sync_fail(run, Error::Other("Sync stopped unexpectedly.".into()));
                }
            }
            _ => {}
        }
        if self.sync.run.as_ref().is_some_and(|r| r.rx.is_some() && r.worker_since.elapsed() >= WORKER_LIMIT)
            && let Some(run) = self.sync.run.take()
        {
            self.sync_fail(run, Error::Other("Syncing took too long, so Graysense stopped it. It will try again later.".into()));
        }
        if self.sync.run.as_ref().is_some_and(|r| r.phase == Phase::Reconciling && r.deadline.is_some_and(|d| Instant::now() >= d))
            && let Some(mut run) = self.sync.run.take()
        {
            for (_, ids) in run.jobs.drain().collect::<Vec<_>>() {
                run.answers.push((ids, Err("it took too long".into())));
            }
            self.sync_reconciled_all(run);
        }
        self.sync_schedule();
    }

    fn sync_done(&mut self, mut run: Run, done: Done) {
        run.rx = None;
        match done {
            Done::Connected(Err(Error::Public)) => {
                if let Some((input, create, _)) = run.connect {
                    self.sync.pending = Some(Pending::Public { input, create });
                }
            }
            Done::Connected(Err(e)) | Done::Fetched(Err(e)) | Done::Pushed(Err(e)) if !matches!(e, Error::Race) || run.attempt >= ATTEMPTS => self.sync_fail(run, e),
            Done::Connected(Err(e)) | Done::Fetched(Err(e)) | Done::Pushed(Err(e)) => {
                // Someone pushed in between: fetch again.
                tracing::info!("sync lost a race ({e}); starting again");
                if let Err(msg) = self.sync_fetch(run.manual, Strategy::Merge, run.attempt + 1, run.choice) {
                    self.sync_fail(run, Error::Other(msg));
                }
            }
            Done::Connected(Ok((connected, fetched))) => self.sync_connected(run, connected, fetched),
            Done::Fetched(Ok((fetched, signin))) => {
                self.sync.signin = Some(signin);
                self.sync_merge(run, fetched);
            }
            Done::Pushed(Ok(pushed)) => self.sync_finish(run, pushed),
        }
    }

    fn sync_fail(&mut self, run: Run, e: Error) {
        tracing::warn!("sync failed: {e}");
        if matches!(e, Error::Auth(_) | Error::NotFound(_)) {
            self.sync.signin = None;
        }
        let message = e.message();
        self.sync.state.last_error = Some(message.clone());
        if run.manual {
            self.toast(Level::Error, message);
        } else {
            self.sync.backoff = Some(Instant::now() + BACKOFF);
        }
        self.sync_save();
    }

    /// The repository is chosen and fetched: either ask what to do with two sets of data, or
    /// remember the repository and take the first sync.
    fn sync_connected(&mut self, mut run: Run, connected: Connected, fetched: Fetched) {
        let allow_public = run.connect.as_ref().is_some_and(|c| c.2);
        let config = Config { repo: Some(connected.repo.clone()), branch: Some(fetched.branch.clone()), account: Some(connected.signin.label.clone()), allow_public, ..Config::default() };
        let local = self.sync_local().snapshot;
        if !fetched.snapshot.is_empty() && !local.is_empty() {
            self.sync.pending = Some(Pending::Merge { config, signin: connected.signin, remote: fetched.snapshot.counts(), local: local.counts(), from: fetched.last.clone() });
            return;
        }
        self.sync_adopt(config, connected.signin);
        run.options = self.sync_options_now(&self.sync.config);
        run.manual = true;
        self.sync_merge(run, fetched);
    }

    /// Start syncing with a repository from scratch (no history with it yet).
    fn sync_adopt(&mut self, config: Config, signin: Signin) {
        self.sync.config = config;
        self.sync.signin = Some(signin);
        self.sync.state = State::default();
        self.sync_save();
    }

    /// Merge what the repository holds into this PC, applying what changed here.
    fn sync_merge(&mut self, mut run: Run, fetched: Fetched) {
        run.phase = Phase::Merging;
        run.connect = None;
        let options = run.options;
        let local = self.sync_local();
        // What this PC has but can't share isn't missing from here: the merge must neither compare
        // it with the repository's copy nor remove that copy.
        let remote = fetched.snapshot.without(&local.withheld);
        let rejected = self.memory.lock().rejected_texts();
        let defaults = Settings::default().synced_values();

        // Most of what was synced going at once isn't an edit: ask before passing it on. Keeping it
        // means forgetting it was synced, so whichever side still has it is the one that's right.
        let mut base = self.sync.state.base.clone();
        let mut forget_rejections = false;
        let merged = loop {
            let none: &[String] = &[];
            let inputs = Inputs { base: &base, local: &local.snapshot, remote: &remote, unreadable: &fetched.unreadable, rejected: if forget_rejections { none } else { &rejected }, defaults: &defaults, options, strategy: run.strategy };
            let merged = merge::merge(&inputs);
            let gone = merge::removals(&inputs, &merged);
            if !gone.mass() || run.choice == Some(Choice::Remove) || forget_rejections {
                break merged;
            }
            if run.choice.is_none() {
                self.sync_ask_removals(gone);
                return;
            }
            for uid in gone.uids() {
                base.memories.remove(uid);
                base.missions.remove(uid);
            }
            forget_rejections = true;
        };

        if !merged.changes.is_empty() || !merged.mission_changes.is_empty() {
            self.sync_backup();
        }
        let repo = sync::repo_dir();
        let mut budget = sync::apply::Budget::new();
        {
            let mut store = self.memory.lock();
            sync::apply::memories(&mut store, &merged.changes, &local.projects, &repo, options, &mut budget);
            sync::apply::late_files(&mut store, &remote, &repo, options, &mut budget);
            for (remote, key) in local.projects.claims(&store) {
                store.adopt_project(&remote, &key);
            }
        }
        sync::apply::missions(&mut self.missions, &merged.mission_changes);
        if !merged.setting_changes.is_empty() {
            let models: Vec<String> = self.models.iter().map(|m| m.id.clone()).collect();
            let before = self.settings.clone();
            self.settings.apply_synced(&merged.setting_changes, &models);
            self.settings_changed(&before);
        }
        // Applying the merge should leave this PC exactly as the merge decided; if not, say so.
        let after = self.sync_local().snapshot;
        if after.base().memories != merged.snapshot.base().memories {
            tracing::warn!(head = ?fetched.head, "after a sync merge this PC differs from the merged result");
        }

        // What's new on both PCs waits for the AI (kept across syncs until it has looked).
        if run.strategy == Strategy::UseRemote {
            self.sync.state.reconcile.clear();
        }
        for c in merged.candidates {
            if !self.sync.state.reconcile.iter().any(|q| q.uid == c.uid) {
                self.sync.state.reconcile.push(c);
            }
        }
        self.sync_save();
        run.report = merged.report;
        run.report.notes.extend(fetched.skipped.iter().cloned());
        run.report.notes.extend(withheld_note(&local.withheld));
        if budget.deferred > 0 {
            let n = budget.deferred;
            run.more_files = true;
            run.report.notes.push(format!("{n} note file{} will come in at the next sync (a sync copies up to {} MB at a time).", if n == 1 { "" } else { "s" }, sync::MAX_DOWNLOAD / 1_048_576));
        }
        run.fetched = Some(fetched);
        self.sync_ai_or_push(run);
    }

    /// Most of what was synced is gone from one side: stop and ask. Nothing has been changed.
    fn sync_ask_removals(&mut self, gone: merge::Removals) {
        let n = gone.here.len() + gone.there.len();
        self.sync.pending = Some(Pending::Removals { here: gone.here.len(), there: gone.there.len(), known: gone.known });
        self.toast_with(Level::Warn, format!("Sync is waiting for you: it would remove {n} of the {} things it keeps in step.", gone.known), Some(("Take a look".into(), json!({ "type": "setSide", "panel": "settings" }))));
    }

    /// Ask the AI to compare what's new on both PCs, if there is any, then push.
    fn sync_ai_or_push(&mut self, mut run: Run) {
        if run.strategy == Strategy::Merge && !self.sync.state.reconcile.is_empty() {
            match self.sync_start_reconcile(&mut run) {
                Ok(true) => {
                    run.phase = Phase::Reconciling;
                    run.deadline = Some(Instant::now() + AI_LIMIT);
                    self.sync.run = Some(run);
                    return;
                }
                Ok(false) => {}
                Err(why) => run.report.notes.push(why),
            }
        }
        self.sync_push(run);
    }

    /// Send the AI what's waiting. Returns whether anything is out for an answer.
    fn sync_start_reconcile(&mut self, run: &mut Run) -> Result<bool, String> {
        if !self.settings.memory.enabled {
            return Err("The AI merge was skipped because memory is turned off.".into());
        }
        if !matches!(self.client, ClientState::Ready) {
            return Err("The AI merge is waiting for Copilot to connect. It will run at the next sync.".into());
        }
        let projects = self.sync_projects();
        let device = self.sync.device.clone();
        let model = self.curator_model();
        let queued: HashMap<String, bool> = self.sync.state.reconcile.iter().map(|c| (c.uid.clone(), c.elsewhere)).collect();
        let (chunks, keep): (Vec<reconcile::Chunk>, HashSet<String>) = {
            let store = self.memory.lock();
            let entries: Vec<(&Memory, Side)> = store
                .memories
                .iter()
                .filter(|m| m.state != memory::State::Archived && m.kind.is_knowledge())
                .filter_map(|m| queued.get(&m.uid).map(|elsewhere| (m, if *elsewhere { Side::Elsewhere } else { Side::Here })))
                .collect();
            let label = |scope: &Scope| projects.to_item(scope).label();
            let (chunks, waiting) = reconcile::chunks(&entries, &device, &label);
            // Anything neither sent nor waiting its turn has nothing from the other PC to be compared with.
            let involved: HashSet<MemoryId> = chunks.iter().flat_map(|c| c.ids.iter().copied()).chain(waiting.iter().copied()).collect();
            let keep = store.memories.iter().filter(|m| involved.contains(&m.id)).map(|m| m.uid.clone()).collect();
            (chunks, keep)
        };
        self.sync.state.reconcile.retain(|c| keep.contains(&c.uid));
        self.sync_save();
        if chunks.is_empty() {
            return Ok(false);
        }
        for chunk in chunks {
            let job = self.sync.next_job;
            self.sync.next_job += 1;
            run.jobs.insert(job, chunk.ids);
            self.backend.send(Cmd::Reconcile { job, model: model.clone(), message: chunk.message });
        }
        Ok(true)
    }

    /// The AI answered one request.
    pub(super) fn sync_reconciled(&mut self, job: u64, result: Result<String, String>) {
        let Some(run) = self.sync.run.as_mut() else { return };
        let Some(ids) = run.jobs.remove(&job) else { return };
        run.answers.push((ids, result));
        if run.jobs.is_empty()
            && let Some(run) = self.sync.run.take()
        {
            self.sync_reconciled_all(run);
        }
    }

    /// Every request is answered (or timed out): apply what the AI decided, then push.
    fn sync_reconciled_all(&mut self, mut run: Run) {
        let mut ops = Vec::new();
        let mut asked: HashSet<MemoryId> = HashSet::new();
        let mut failure: Option<String> = None;
        {
            let store = self.memory.lock();
            let protected = |id: MemoryId| store.get(id).is_some_and(|m| m.is_user_owned() || m.pinned);
            for (ids, result) in &run.answers {
                match result {
                    Ok(content) => {
                        let allowed: HashSet<MemoryId> = ids.iter().copied().collect();
                        ops.extend(reconcile::parse(content, &allowed, &protected));
                        asked.extend(ids.iter().copied());
                    }
                    Err(e) => failure = Some(e.clone()),
                }
            }
        }
        let source = Source { session: None, title: Some("Merged from your other PC".into()), quote: None, at: memory::now() };
        let auto_accept = self.settings.memory.auto_accept();
        let (outcome, done): (memory::Outcome, HashSet<String>) = {
            let mut store = self.memory.lock();
            let done = store.memories.iter().filter(|m| asked.contains(&m.id)).map(|m| m.uid.clone()).collect();
            (store.apply(ops, source, auto_accept), done)
        };
        run.report.tidied = outcome.removed + outcome.updated;
        run.report.review = outcome.suggested;
        // What was looked at doesn't need another look; what couldn't be asked about waits.
        self.sync.state.reconcile.retain(|c| !done.contains(&c.uid));
        if let Some(e) = failure {
            run.report.notes.push(format!("The AI merge couldn't finish ({e}). It will try again at the next sync."));
        }
        self.sync_save();
        self.sync_push(run);
    }

    /// Write the result to the repository.
    fn sync_push(&mut self, mut run: Run) {
        let Some(fetched) = run.fetched.take() else {
            self.sync_fail(run, Error::Other("Sync lost track of the repository. Try again.".into()));
            return;
        };
        let Some(signin) = self.sync.signin.clone() else {
            self.sync_fail(run, Error::Auth("Sign in to GitHub first (gh auth login).".into()));
            return;
        };
        let Ok(git) = self.sync_git() else {
            self.sync_fail(run, Error::NoGit);
            return;
        };
        run.phase = Phase::Pushing;
        let options = run.options;
        let Some(repo) = self.sync.config.repo.clone() else { return };
        // This PC as it is now (the merge was applied, and anything edited meanwhile goes too).
        // What a sync leaves out stays as the repository has it, and so does the repository's copy
        // of anything this PC can't share.
        let local = self.sync_local();
        let previous = fetched.snapshot.without(&local.withheld);
        let kept = fetched.snapshot.only(&local.withheld);
        run.withheld = local.withheld;
        let local = local.snapshot;
        let snapshot = Snapshot {
            memories: local.memories,
            missions: if options.missions { local.missions } else { previous.missions.clone() },
            settings: if options.settings { local.settings } else { previous.settings.clone() },
        };
        run.report.pushed = merge::changed_files(&previous, &snapshot, options);
        let n = run.report.pushed;
        let summary = format!("{n} change{}", if n == 1 { "" } else { "s" });
        let device = self.sync.device.clone();
        let branch = fetched.branch.clone();
        let pushed_snapshot = snapshot.clone();
        let (tx, rx) = channel();
        let remote = Remote::new(git, &repo, &branch, &signin.token, &device);
        std::thread::spawn(move || {
            let _ = tx.send(Done::Pushed(git::push(&remote, &previous, &snapshot, &kept, options, &summary)));
        });
        run.watch(rx);
        run.pushing = Some((pushed_snapshot, branch));
        self.sync.run = Some(run);
    }

    fn sync_finish(&mut self, mut run: Run, pushed: Pushed) {
        let Some((snapshot, branch)) = run.pushing.take() else { return };
        let options = run.options;
        let now = memory::now();
        let mut base = snapshot.base();
        // What this PC couldn't share keeps the entry it had: when it's deleted here later, that has
        // to read as a deletion of what the repository still holds, not as something new there.
        for uid in &run.withheld {
            if let Some(hash) = self.sync.state.base.memories.get(uid) {
                base.memories.insert(uid.clone(), hash.clone());
            }
            if let Some(hash) = self.sync.state.base.missions.get(uid) {
                base.missions.insert(uid.clone(), hash.clone());
            }
        }
        // A part this PC doesn't sync keeps the base it had, for when it's switched on.
        if !options.missions {
            base.missions = self.sync.state.base.missions.clone();
        }
        if !options.settings {
            base.settings = self.sync.state.base.settings.clone();
        }
        let mut report = std::mem::take(&mut run.report);
        report.at = now;
        report.notes.extend(pushed.notes);
        self.sync.state.base = base;
        self.sync.state.commit = pushed.head;
        self.sync.state.last_sync = Some(now);
        self.sync.state.last_error = None;
        self.sync.state.report = Some(report.clone());
        self.sync.config.branch = Some(branch);
        self.sync.next_poll = Instant::now() + if run.more_files { MORE_FILES } else { POLL };
        self.sync.backoff = None;
        self.sync.started = true;
        self.sync.changes = false;
        self.sync.changed_at = None;
        // What this sync applied reaches the disk before the base that says it did: if Graysense
        // were stopped in between, a base ahead of the memories would read as everything the
        // merge brought in having been deleted here.
        self.save();
        self.sync_save();
        if run.manual {
            let repo = self.sync.config.repo.clone().unwrap_or_default();
            self.toast(Level::Ok, format!("Synced with {repo}: {}", describe(&report)));
        }
        if report.review > 0 {
            let n = report.review;
            self.toast_with(Level::Info, format!("Merging left {n} change{} for you to review", if n == 1 { "" } else { "s" }), Some(("Review".into(), json!({ "type": "setSide", "panel": "memory" }))));
        }
        self.dirty.memory = true;
    }

    // ------------------------------------------------------------------ this PC

    /// What this PC would put in the repository, what it can't, and the projects it knows (so
    /// scopes can be read both ways).
    fn sync_local(&mut self) -> Local {
        self.memory.lock().ensure_uids();
        self.missions.ensure_uids();
        let projects = self.sync_projects();
        let store = self.memory.lock();
        let (snapshot, withheld) = Snapshot::local_with_withheld(store.syncable(), &self.missions.missions, self.settings.synced_values(), &projects);
        Local { snapshot, projects, withheld }
    }

    /// The folders this PC knows: where its memories apply, recent folders and open sessions. A
    /// folder on a network share isn't looked into (Windows would have to reach the server, and
    /// the check runs every few seconds): such a folder has no remote to find.
    fn sync_projects(&self) -> Projects {
        let mut keys: Vec<String> = self.memory.lock().memories.iter().filter_map(|m| if let Scope::Project(k) = &m.scope { Some(k.clone()) } else { None }).collect();
        keys.extend(self.settings.recent_dirs.iter().filter(|d| sync::is_local_path(d)).map(|d| Project::for_dir(Path::new(d)).key));
        keys.extend(self.bays.iter().filter_map(|b| if let BayKind::Agent(v) = &b.kind { Some(v.project.key.clone()) } else { None }));
        Projects::new(&self.settings.sessions_root(), keys)
    }

    /// Keep a copy of what's on this PC before a sync changes it (the last few are kept).
    fn sync_backup(&mut self) {
        self.memory.lock().save_if_dirty();
        self.missions.save_if_dirty();
        let dir = sync::dir().join("backups");
        let (memory_file, missions_file) = (memory::MemoryStore::path(), crate::settings::config_dir().join("missions.json"));
        sync::apply::backup(&[&memory_file, &missions_file], &dir, 5);
    }

    // ------------------------------------------------------------------ when to sync

    /// Notice changes made here, and sync by itself when it's time.
    fn sync_schedule(&mut self) {
        if !self.sync.config.connected() || self.sync.run.is_some() || self.sync.pending.is_some() || self.exiting || self.tidy.running() {
            return;
        }
        let now = Instant::now();
        if self.sync.checked.elapsed() >= CHECK {
            self.sync.checked = now;
            self.sync_check_changes();
        }
        if !self.sync.config.auto || self.sync.backoff.is_some_and(|t| now < t) {
            return;
        }
        let first = !self.sync.started && self.sync.born.elapsed() >= STARTUP_DELAY;
        let changed = self.sync.changed_at.is_some_and(|t| t.elapsed() >= DEBOUNCE);
        if first || changed || now >= self.sync.next_poll {
            self.sync.started = true;
            self.sync.next_poll = now + POLL;
            let _ = self.sync_begin(false, Strategy::Merge);
        }
    }

    /// Whether this PC has changes the repository doesn't, and when they were last made.
    fn sync_check_changes(&mut self) {
        let options = self.sync_options_now(&self.sync.config);
        let local = self.sync_local();
        let now = local.snapshot.base();
        // What this PC can't share isn't compared: the base still has its entry (see `sync_finish`).
        let mut was = self.sync.state.base.clone();
        for uid in &local.withheld {
            was.memories.remove(uid);
            was.missions.remove(uid);
        }
        let changed = now.memories != was.memories || (options.missions && now.missions != was.missions) || (options.settings && now.settings != was.settings);
        let print = sync::fnv(&[&serde_json::to_string(&now).unwrap_or_default()]);
        if changed && print != self.sync.fingerprint {
            self.sync.changed_at = Some(Instant::now());
        }
        if !changed {
            self.sync.changed_at = None;
        }
        self.sync.fingerprint = print;
        self.sync.changes = changed;
    }
}

/// Fetch with each sign-in until one gets in. Refuses a repository that's public (unless the
/// user was told and accepted), checked every time: a repository can be made public later.
fn fetch_job(git: PathBuf, repo: &str, branch: &str, allow_public: bool, cached: Option<Signin>, device: &str) -> Result<(Fetched, Signin), Error> {
    let signins = match cached {
        Some(s) => vec![s],
        None => git::signins(),
    };
    if signins.is_empty() {
        return Err(Error::Auth("Sign in to GitHub first: install the GitHub CLI (cli.github.com) and run `gh auth login`, or set GH_TOKEN.".into()));
    }
    let mut failure = None;
    for signin in signins {
        let attempt = (|| {
            if !allow_public && !git::is_private(repo, &signin.token)? {
                return Err(Error::Public);
            }
            git::fetch(&Remote::new(git.clone(), repo, branch, &signin.token, device))
        })();
        match attempt {
            Ok(fetched) => return Ok((fetched, signin)),
            Err(e @ (Error::Auth(_) | Error::NotFound(_))) => {
                failure.get_or_insert(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(failure.unwrap_or(Error::Offline))
}
