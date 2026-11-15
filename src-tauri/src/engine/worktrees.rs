//! Sessions that work in a worktree of their own (see [`crate::worktree`]): made with a new
//! session's first message (or for a fork), merged back or pushed for a pull request when the
//! work is done, and removed when the session closes unless there's work in it. Git runs on
//! threads of its own; the tick picks up what it did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};

use serde_json::json;

use super::{Bay, BayKind, Core, capture_explicit_memory};
use crate::agent::backend::{Cmd, ForkJob};
use crate::agent::{AgentView, BayId, Level, Mode, Status};
use crate::sessions;
use crate::settings::short_path;
use crate::worktree::{self, Created, Info, MergeError, Merged, Published};

/// What a session waiting on its worktree shows it's doing.
const MAKING: &str = "Making a worktree…";

pub struct Worktrees {
    tx: Sender<(u64, Done)>,
    rx: Receiver<(u64, Done)>,
    next: u64,
    jobs: HashMap<u64, Job>,
}

impl Worktrees {
    pub fn new() -> Worktrees {
        let (tx, rx) = channel();
        Worktrees { tx, rx, next: 1, jobs: HashMap::new() }
    }
}

/// A session's first message, waiting on the worktree it starts in.
struct Start {
    bay: BayId,
    shown: String,
    sent: String,
    files: Vec<PathBuf>,
    /// The transcript's length and the title before the message, to put back if the worktree
    /// can't be made.
    items: usize,
    title: Option<String>,
}

/// What git was set to do, and for whom.
enum Job {
    Start(Start),
    /// The worktree a fork goes to, made before the runtime copies the session.
    Fork(ForkJob),
    Merge { bay: BayId, info: Info, close: bool },
    Publish { bay: BayId, info: Info },
    /// A session closed: its worktree goes, unless there's work in it.
    Closed { info: Info, session: Option<String> },
    /// `quiet`: the user didn't ask for it, so only a failure is told.
    Remove { info: Info, quiet: bool },
}

/// What git did.
enum Done {
    Created(Result<Created, String>),
    Merged(Result<Merged, MergeError>),
    Published(Result<Published, String>),
    /// Whether a closed session's worktree was removed.
    Tidied(Result<bool, String>),
    Removed(Result<(), String>),
}

/// The top folder of the work tree `dir` is in.
fn top_of(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

/// Where a file attached from the work tree `top` is in the worktree `to` (if it's there too;
/// otherwise where it was).
fn moved(path: &Path, top: &Path, to: &Path) -> PathBuf {
    if sessions::is_under(path, top) {
        let rel: PathBuf = path.components().skip(top.components().count()).collect();
        let there = if rel.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rel) };
        if there.exists() {
            return there;
        }
    }
    path.to_path_buf()
}

impl Core {
    /// Run `work` (git) on a thread of its own; [`Self::worktree_tick`] finishes the job.
    fn spawn_git(&mut self, job: Job, work: impl FnOnce() -> Done + Send + 'static) {
        let w = &mut self.worktrees;
        let id = w.next;
        w.next += 1;
        w.jobs.insert(id, job);
        let tx = w.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((id, work()));
        });
    }

    /// Finish what git did since the last tick.
    pub(super) fn worktree_tick(&mut self) {
        while let Ok((id, done)) = self.worktrees.rx.try_recv() {
            let Some(job) = self.worktrees.jobs.remove(&id) else { continue };
            match (job, done) {
                (Job::Start(start), Done::Created(r)) => self.started_in_worktree(start, r),
                (Job::Fork(job), Done::Created(r)) => self.fork_worktree_made(job, r),
                (Job::Merge { bay, info, close }, Done::Merged(r)) => self.merged(bay, info, close, r),
                (Job::Publish { bay, info }, Done::Published(r)) => self.published(bay, info, r),
                (Job::Closed { info, session }, Done::Tidied(r)) => self.closed_worktree_tidied(info, session, r),
                (Job::Remove { info, quiet }, Done::Removed(r)) => self.worktree_removed(info, quiet, r),
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------ new sessions

    /// A new session that works in a worktree of its own, made from the repository folder `from`
    /// when its first message is sent (and named after it, like a session folder).
    pub fn new_worktree_session(&mut self, from: PathBuf) -> BayId {
        let key = crate::memory::project_key(&from);
        // One for the same folder is open and unused: go to it rather than adding another.
        let blank = self.bays.iter().find_map(|b| match &b.kind {
            BayKind::Agent(v)
                if v.deferred && v.worktree_busy.is_none() && v.items.is_empty() && v.attachments.is_empty() && v.worktree_from.as_deref().is_some_and(|f| crate::memory::project_key(f) == key) =>
            {
                Some(b.id)
            }
            _ => None,
        });
        if let Some(id) = blank {
            self.focus(id);
            return id;
        }
        let id = self.alloc_bay();
        let mut view = AgentView::new(id, from.clone(), Mode::Interactive);
        view.auto_approve.store(self.settings.default_auto_approve, Ordering::Relaxed);
        view.project = self.project_for(&from);
        view.deferred = true;
        view.status = Status::Idle;
        view.model = self.settings.default_model.clone();
        view.effort = self.settings.default_effort.clone();
        view.context_tier = self.settings.default_context.clone();
        // Made from a worktree, it still gets one of its own.
        view.worktree = None;
        view.worktree_from = Some(from.clone());
        self.push_bay(Bay { id, kind: BayKind::Agent(Box::new(view)) });
        self.remember_dir(&from);
        if self.models.is_empty() {
            self.backend.send(Cmd::ListModels);
        }
        id
    }

    /// A new session in a worktree of `dir`'s repository, started with `prompt` if there's one.
    pub(super) fn new_agent_in_worktree(&mut self, dir: PathBuf, prompt: Option<String>) -> Result<BayId, String> {
        if !worktree::in_repo(&dir) {
            return Err(format!("{} isn't in a Git repository, so a session there can't have a worktree of its own.", short_path(&dir)));
        }
        let bay = self.new_worktree_session(dir);
        if let Some(p) = prompt {
            self.start_in_worktree(bay, p.clone(), p);
        }
        Ok(bay)
    }

    /// Launch a mission in a new session with a worktree of its own, made from the mission's folder.
    pub(super) fn launch_mission_in_worktree(&mut self, id: u64) -> Result<(), String> {
        let m = self.missions.get(id).cloned().ok_or("That mission no longer exists.")?;
        let dir = m.dir.as_ref().map(PathBuf::from).filter(|d| d.is_dir()).ok_or("Choose the mission's folder first: its worktree is made from it.")?;
        let bay = self.new_agent_in_worktree(dir, None)?;
        if let Some(v) = self.agent_mut(bay) {
            v.mission = Some(id);
        }
        self.start_in_worktree(bay, m.brief(), m.brief());
        self.missions.link(id, bay);
        self.toast(Level::Ok, format!("Launched “{}” in a new worktree", m.title));
        Ok(())
    }

    /// The first message of a session waiting on its worktree: make the worktree, then start the
    /// session there with the message ([`Self::started_in_worktree`]).
    pub(super) fn start_in_worktree(&mut self, bay: BayId, shown: String, sent: String) {
        let Some(v) = self.agent_mut(bay) else { return };
        let Some(from) = v.worktree_from.clone().filter(|_| v.worktree_busy.is_none()) else { return };
        let files = std::mem::take(&mut v.attachments);
        let (items, title) = (v.items.len(), v.title.clone());
        v.status = Status::Booting;
        v.worktree_busy = Some(MAKING);
        v.title = Some(shown.clone());
        v.echo_user_shown(&shown, &sent, files.iter().map(|p| p.display().to_string()).collect(), false);
        let hint = sessions::short_title(&shown);
        self.spawn_git(Job::Start(Start { bay, shown, sent, files, items, title }), move || Done::Created(worktree::create(&from, &hint, false, None)));
    }

    fn started_in_worktree(&mut self, start: Start, result: Result<Created, String>) {
        let Start { bay, shown, sent, files, items, title } = start;
        let created = match result {
            Ok(created) => created,
            Err(e) => {
                // The message goes back in the message box, as if it was never sent.
                let Some(v) = self.agent_mut(bay) else { return };
                v.items.truncate(items);
                v.send_failed(&sent);
                v.sent_history.pop();
                v.has_history = false;
                v.title = title;
                v.status = Status::Idle;
                v.busy_since = None;
                v.worktree_busy = None;
                let files: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
                self.give_back(bay, shown, &files);
                self.toast(Level::Error, format!("Couldn't make a worktree: {e}"));
                return;
            }
        };
        let Some(from) = self.agent(bay).and_then(|v| v.worktree_from.clone()) else {
            // Closed while it was made: nothing will work in it.
            self.remove_worktree(created.info, true);
            return;
        };
        let project = self.project_for(&created.cwd);
        let (enabled, memory) = (self.settings.memory.enabled, self.memory.clone());
        let top = top_of(&from);
        let files: Vec<PathBuf> = files.iter().map(|p| moved(p, &top, &created.info.path)).collect();
        let info = created.info;
        let Some(v) = self.agent_mut(bay) else { return };
        v.granted.lock().extend(files.iter().cloned());
        v.cwd = created.cwd;
        v.project = project;
        v.deferred = false;
        v.worktree_from = None;
        v.worktree_busy = None;
        capture_explicit_memory(&memory, enabled, v, &shown);
        let base = info.base_branch.as_deref().map(|b| format!(" (from {b})")).unwrap_or_default();
        v.notice(
            Level::Info,
            format!("This session works in a new worktree, {}, on the branch {}{base}. Its changes stay out of {} until you merge them.", short_path(&info.path), info.branch, short_path(&info.repo)),
        );
        v.worktree = Some(info);
        let (name, user) = (v.name.clone().unwrap_or_else(|| sessions::short_title(&shown)), v.name.is_some());
        let Some(v) = self.agent(bay) else { return };
        let spec = self.agent_spec(v, Some(sent), files, None);
        self.backend.send(Cmd::Create { bay, spec });
        self.backend.send(Cmd::NameSession { bay, name, user });
        if let Some(w) = created.warning {
            self.toast(Level::Warn, w);
        }
        self.dirty.sessions = true;
    }

    // ------------------------------------------------------------------ forks

    /// Fork the session in `bay` (see [`Self::fork`]) into a worktree of its own, made from the
    /// original's folder with the changes not committed there, so the two don't share files.
    pub(super) fn fork_into_worktree(&mut self, bay: BayId, index: Option<usize>) -> Result<(), String> {
        let job = self.fork_job(bay, index)?;
        let v = self.agent(bay).ok_or("That agent is closed.")?;
        let from = v.cwd.clone();
        if !worktree::in_repo(&from) {
            return Err(format!("{} isn't in a Git repository, so the fork can't have a worktree of its own.", short_path(&from)));
        }
        let hint = v.display_title();
        self.spawn_git(Job::Fork(job), move || Done::Created(worktree::create(&from, &hint, true, Some(&from))));
        self.toast(Level::Info, "Making a worktree for the fork…");
        Ok(())
    }

    fn fork_worktree_made(&mut self, mut job: ForkJob, result: Result<Created, String>) {
        let created = match result {
            Ok(created) => created,
            Err(e) => {
                self.toast(Level::Error, format!("Couldn't make a worktree for the fork: {e}"));
                return;
            }
        };
        if let Some(w) = created.warning {
            self.toast(Level::Warn, w);
        }
        // The message it was forked before keeps its files, as they are in the worktree.
        if let Some(from) = &created.info.forked_from {
            let top = top_of(from);
            job.files = job.files.iter().map(|f| moved(Path::new(f), &top, &created.info.path).display().to_string()).collect();
        }
        job.cwd = Some(created.cwd);
        self.backend.send(Cmd::Fork { job });
    }

    // ------------------------------------------------------------------ closing

    /// Close a bay. A session's worktree is kept (`keep`), deleted with its branch (`discard`), or
    /// by default deleted only if nothing was done in it.
    pub(super) fn close_with_worktree(&mut self, bay: BayId, choice: Option<&str>) {
        // One being merged or pushed is left as it is.
        let worktree = self.agent(bay).filter(|v| v.worktree_busy.is_none()).and_then(|v| Some((v.worktree.clone()?, v.session_id.clone())));
        self.close_bay(bay);
        let Some((info, session)) = worktree else { return };
        if self.bays.iter().any(|b| sessions::is_under(b.cwd(), &info.path)) {
            if choice == Some("discard") {
                self.toast(Level::Info, format!("Kept the worktree {}: another session or terminal is working in it.", short_path(&info.path)));
            }
            return;
        }
        match choice {
            Some("keep") => {}
            Some("discard") => self.remove_worktree(info, false),
            _ => {
                let work = info.clone();
                self.spawn_git(Job::Closed { info, session }, move || Done::Tidied(worktree::remove_if_unused(&work)));
            }
        }
    }

    fn closed_worktree_tidied(&mut self, info: Info, session: Option<String>, result: Result<bool, String>) {
        match result {
            Ok(true) => {}
            Ok(false) => {
                // Reopening the session is how its work gets merged or pushed.
                let (reopen, then) = match session.filter(|id| self.session_index.get(id).is_some()) {
                    Some(id) => (Some(("Reopen".to_owned(), json!({ "type": "resumeSession", "id": id }))), "Reopen the session to merge it or open a pull request."),
                    None => (None, "Merge its branch, or remove it with git worktree remove, when you're done with it."),
                };
                self.toast_with(Level::Info, format!("Kept the worktree {} (branch {}): it has work in it. {then}", short_path(&info.path), info.branch), reopen);
            }
            Err(e) => tracing::warn!("couldn't tidy the worktree {}: {e}", info.path.display()),
        }
    }

    pub(super) fn remove_worktree(&mut self, info: Info, quiet: bool) {
        let work = info.clone();
        self.spawn_git(Job::Remove { info, quiet }, move || Done::Removed(worktree::remove(&work, true)));
    }

    fn worktree_removed(&mut self, info: Info, quiet: bool, result: Result<(), String>) {
        match result {
            Ok(()) if !quiet => self.toast(Level::Ok, format!("Deleted the worktree {} and its branch {}", short_path(&info.path), info.branch)),
            Ok(()) => {}
            Err(e) => self.toast(Level::Error, format!("Couldn't delete the worktree: {e}")),
        }
    }

    // ------------------------------------------------------------------ finishing

    /// The worktree of the session in `bay`, if it can be merged or pushed now (it's then busy
    /// `doing` that).
    fn worktree_ready(&mut self, bay: BayId, doing: &'static str) -> Result<Info, String> {
        let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
        let info = v.worktree.clone().ok_or("This session doesn't work in a worktree of its own.")?;
        if let Some(busy) = v.worktree_busy {
            return Err(format!("{busy} Try again when it's done."));
        }
        if v.status.is_busy() || v.status.needs_attention() {
            return Err("Wait for the agent to finish (or stop it) first: it may still be changing files.".into());
        }
        v.worktree_busy = Some(doing);
        Ok(info)
    }

    /// The message to commit what isn't committed yet with: the one given, or the session's title.
    fn commit_message(&self, bay: BayId, message: String) -> String {
        let message = message.trim();
        if !message.is_empty() {
            return message.to_owned();
        }
        self.agent(bay).map(|v| v.display_title()).unwrap_or_else(|| "Work from Graysense".into())
    }

    /// Merge the worktree's branch into the branch it started from (`close`: then close the
    /// session and remove the worktree).
    pub(super) fn merge_worktree(&mut self, bay: BayId, message: String, close: bool) -> Result<(), String> {
        let message = self.commit_message(bay, message);
        let info = self.worktree_ready(bay, "Merging…")?;
        let work = info.clone();
        self.spawn_git(Job::Merge { bay, info, close }, move || Done::Merged(worktree::merge(&work, &message)));
        Ok(())
    }

    fn merged(&mut self, bay: BayId, info: Info, close: bool, result: Result<Merged, MergeError>) {
        if let Some(v) = self.agent_mut(bay) {
            v.worktree_busy = None;
        }
        let base = info.base_branch.clone().unwrap_or_default();
        match result {
            Ok(m) => {
                let what = match m.commits {
                    0 => format!("{} already had everything on {}", m.into, info.branch),
                    1 => format!("Merged 1 commit into {}", m.into),
                    n => format!("Merged {n} commits into {}", m.into),
                };
                if !close {
                    if let Some(v) = self.agent_mut(bay) {
                        v.notice(Level::Info, format!("{what}."));
                    }
                    self.toast(Level::Ok, what);
                    return;
                }
                if self.agent(bay).is_some() {
                    self.close_bay(bay);
                }
                if self.bays.iter().any(|b| sessions::is_under(b.cwd(), &info.path)) {
                    self.toast(Level::Ok, format!("{what}. The worktree was kept: another session or terminal is working in it."));
                } else {
                    self.remove_worktree(info, true);
                    self.toast(Level::Ok, format!("{what}, and removed the worktree."));
                }
            }
            Err(MergeError::Conflict(files)) => {
                let what = match files.len() {
                    0 => "the same lines".to_owned(),
                    n if n <= 3 => files.join(", "),
                    n => format!("{} and {} more", files[..3].join(", "), n - 3),
                };
                let fix = self.agent(bay).map(|_| {
                    let text = format!("Merge `{base}` into this branch (`{}`), resolve the conflicts, and commit the result.", info.branch);
                    ("Ask the agent to fix it".to_owned(), json!({ "type": "send", "bay": bay, "text": text }))
                });
                self.toast_with(Level::Warn, format!("Nothing was merged: {} and {base} both changed {what}. Bring {base} into the branch and resolve the conflicts first.", info.branch), fix);
            }
            Err(MergeError::Failed(e)) => self.toast(Level::Error, format!("Couldn't merge: {e}")),
        }
    }

    /// Push the worktree's branch for a pull request, and open the page for one.
    pub(super) fn publish_worktree(&mut self, bay: BayId, message: String) -> Result<(), String> {
        let message = self.commit_message(bay, message);
        let info = self.worktree_ready(bay, "Pushing…")?;
        let work = info.clone();
        self.spawn_git(Job::Publish { bay, info }, move || {
            let r = worktree::publish(&work, &message);
            if let Ok(Published { url: Some(url), .. }) = &r {
                let _ = tauri_plugin_opener::open_url(url, None::<&str>);
            }
            Done::Published(r)
        });
        Ok(())
    }

    fn published(&mut self, bay: BayId, info: Info, result: Result<Published, String>) {
        if let Some(v) = self.agent_mut(bay) {
            v.worktree_busy = None;
        }
        match result {
            Ok(p) => {
                let text = format!("Pushed {} to {}", info.branch, p.remote);
                if let Some(v) = self.agent_mut(bay) {
                    v.notice(Level::Info, format!("{text}."));
                }
                match p.url {
                    Some(url) => self.toast_with(Level::Ok, format!("{text}. Finish the pull request in your browser."), Some(("Open pull request".into(), json!({ "type": "openUrl", "url": url })))),
                    None => self.toast(Level::Ok, format!("{text}. Open a pull request for it on the repository's site.")),
                }
            }
            Err(e) => self.toast(Level::Error, format!("Couldn't push {}: {e}", info.branch)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_files_follow_the_session_into_its_worktree() {
        let root = std::env::temp_dir().join(format!("gs-moved-{}", std::process::id()));
        let (repo, wt) = (root.join("app"), root.join("app.worktrees").join("fix"));
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join("src").join("main.rs"), "").unwrap();
        // In the worktree too: there. Only in the repository (not committed), or elsewhere: as it was.
        assert_eq!(moved(&repo.join("src").join("main.rs"), &repo, &wt), wt.join("src").join("main.rs"));
        assert_eq!(moved(&repo.join("src").join("main.rs"), &root.join("APP"), &wt), wt.join("src").join("main.rs"));
        assert_eq!(moved(&repo.join("notes.txt"), &repo, &wt), repo.join("notes.txt"));
        assert_eq!(moved(&repo, &repo, &wt), wt);
        assert_eq!(moved(&root.join("other.txt"), &repo, &wt), root.join("other.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
