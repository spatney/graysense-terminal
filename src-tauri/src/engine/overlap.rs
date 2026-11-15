//! Agents working in the same place. When a session starts (or moves) where other agents are
//! changing files, it says so: two agents editing the same files can undo each other's work. While
//! the session hasn't had a message, its home screen offers to start it over in a worktree of its
//! own ([`Core::restart_in_worktree`]); one that started with a message gets a toast.

use std::path::{Path, PathBuf};

use super::dto::OverlapDto;
use super::{BayKind, Core, plural};
use crate::agent::{AgentView, BayId, Level, Overlap, Status};
use crate::memory::project_key;
use crate::sessions;
use crate::worktree;

/// The agent is at work on a message, or waiting on the user to go on with it.
fn working(v: &AgentView) -> bool {
    match &v.status {
        // A reopened session starting is only loading; a new one with a message is at work.
        Status::Booting => v.message_on_its_way(),
        s => s.is_busy() || s.needs_attention(),
    }
}

/// The agent is changing files, or has changed some: work another agent could undo.
fn changing(v: &AgentView) -> bool {
    !v.deferred && v.status != Status::Offline && (working(v) || !v.files.is_empty())
}

/// The busiest first: those at work, then those that changed the most files.
fn busiest_first(agents: &mut [&AgentView]) {
    agents.sort_by_key(|v| (!working(v), std::cmp::Reverse(v.files.len())));
}

/// `path` as the file system knows it (links and `.` resolved), without Windows' `\\?\` prefix.
fn canonical(path: &Path) -> PathBuf {
    let Ok(full) = std::fs::canonicalize(path) else { return path.to_path_buf() };
    let text = full.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => match rest.strip_prefix(r"UNC\") {
            Some(share) => PathBuf::from(format!(r"\\{share}")),
            None => PathBuf::from(rest),
        },
        None => full,
    }
}

/// Where an agent working in `dir` changes files, as a key: the git work tree it's in (a worktree
/// is one of its own), or the folder itself outside one. A session folder, or a folder in a
/// repository of the whole home folder (dotfiles), counts by itself. `root` and `home` are canonical.
fn scope(dir: &Path, root: &Path, home: Option<&Path>) -> String {
    let dir = canonical(dir);
    let top = worktree::top_of(&dir);
    let alone = sessions::is_under(&dir, root) || home.is_some_and(|h| project_key(h) == project_key(&top));
    project_key(if alone { &dir } else { &top })
}

/// What to say about agents changing files where another one works: the busiest one's name,
/// whether it's at work and how many files it changed, how many more there are, and whether the
/// place is a repository.
pub(super) fn overlap_text(first: &str, working: bool, files: usize, others: usize, repo: bool) -> String {
    let place = if repo { "repository" } else { "folder" };
    let what = match (others, working) {
        (0, true) if files > 0 => format!("“{first}” is changing files in this {place} too ({files} so far)."),
        (0, true) => format!("“{first}” is working in this {place} too."),
        (0, false) => format!("“{first}” has changed {} in this {place}.", plural(files, "file", "files")),
        (n, true) => format!("“{first}” and {} are changing files in this {place} too.", plural(n, "other agent", "other agents")),
        (n, false) => format!("“{first}” and {} have changed files in this {place}.", plural(n, "other agent", "other agents")),
    };
    format!("{what} Two agents changing the same files can undo each other's work.")
}

fn describe(agents: &[&AgentView], repo: bool) -> Option<String> {
    let first = agents.first()?;
    Some(overlap_text(&first.display_title(), working(first), first.files.len(), agents.len() - 1, repo))
}

impl Core {
    /// Tell when other agents are changing files where the agent in `bay` now works: on its home
    /// screen while it hasn't had a message (with a worktree to start in instead), else in a toast.
    pub(super) fn note_overlap(&mut self, bay: BayId) {
        let Some(v) = self.agent(bay) else { return };
        if v.deferred || v.worktree_from.is_some() {
            return;
        }
        let (cwd, started) = (v.cwd.clone(), v.has_history);
        let root = canonical(&self.settings.sessions_root());
        let home = dirs::home_dir().map(|h| canonical(&h));
        let here = scope(&cwd, &root, home.as_deref());
        let mut with: Vec<&AgentView> = self
            .bays
            .iter()
            .filter(|b| b.id != bay)
            .filter_map(|b| match &b.kind {
                BayKind::Agent(o) => Some(o.as_ref()),
                BayKind::Shell(_) => None,
            })
            .filter(|o| changing(o) && scope(&o.cwd, &root, home.as_deref()) == here)
            .collect();
        busiest_first(&mut with);
        let repo = worktree::in_repo(&cwd);
        let text = describe(&with, repo);
        let overlap = (!with.is_empty()).then(|| Overlap { with: with.iter().map(|o| o.bay).collect(), repo });
        if let Some(v) = self.agent_mut(bay) {
            v.overlap = overlap;
        }
        if started && let Some(text) = text {
            self.toast(Level::Warn, text);
        }
    }

    /// What the home screen of the agent `v` says about others changing files where it works: those
    /// still at it, until it has had a message.
    pub(super) fn overlap_dto(&self, v: &AgentView) -> Option<OverlapDto> {
        let o = v.overlap.as_ref().filter(|_| !v.has_history && !v.deferred && v.worktree_from.is_none())?;
        let mut with: Vec<&AgentView> = o.with.iter().filter_map(|b| self.agent(*b)).filter(|a| changing(a)).collect();
        busiest_first(&mut with);
        Some(OverlapDto { text: describe(&with, o.repo)?, worktree: o.repo })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Mode;

    fn agent(bay: BayId, status: Status, files: usize) -> AgentView {
        let mut v = AgentView::new(bay, std::env::temp_dir(), Mode::Interactive);
        v.status = status;
        v.files = (0..files).map(|i| format!("f{i}.rs")).collect();
        v
    }

    #[test]
    fn what_counts_as_changing_files() {
        assert!(changing(&agent(1, Status::Thinking, 0)));
        assert!(changing(&agent(1, Status::Waiting("permission".into()), 0)));
        assert!(changing(&agent(1, Status::Idle, 2)));
        assert!(!changing(&agent(1, Status::Idle, 0)));
        assert!(!changing(&agent(1, Status::Error("boom".into()), 0)));
        // A session that's closed, or hasn't started, isn't changing anything.
        assert!(!changing(&agent(1, Status::Offline, 3)));
        let mut later = agent(1, Status::Idle, 3);
        later.deferred = true;
        assert!(!changing(&later));
        // Starting: at work only with a message on its way (a reopened one is just loading).
        let mut new = agent(1, Status::Booting, 0);
        assert!(!changing(&new));
        new.echo_user("fix the build");
        assert!(changing(&new));
    }

    #[test]
    fn the_busiest_agent_comes_first() {
        let (idle, few, busy) = (agent(1, Status::Idle, 9), agent(2, Status::Idle, 2), agent(3, Status::Tool("edit".into()), 1));
        let mut all = vec![&few, &idle, &busy];
        busiest_first(&mut all);
        assert_eq!(all.iter().map(|v| v.bay).collect::<Vec<_>>(), [3, 1, 2]);
    }

    #[test]
    fn how_the_warning_reads() {
        let tail = " Two agents changing the same files can undo each other's work.";
        assert_eq!(overlap_text("Fix login", true, 5, 0, true), format!("“Fix login” is changing files in this repository too (5 so far).{tail}"));
        assert_eq!(overlap_text("Fix login", true, 0, 0, false), format!("“Fix login” is working in this folder too.{tail}"));
        assert_eq!(overlap_text("Fix login", false, 1, 0, true), format!("“Fix login” has changed 1 file in this repository.{tail}"));
        assert_eq!(overlap_text("Fix login", true, 3, 1, true), format!("“Fix login” and 1 other agent are changing files in this repository too.{tail}"));
        assert_eq!(overlap_text("Fix login", false, 3, 2, true), format!("“Fix login” and 2 other agents have changed files in this repository.{tail}"));
    }

    #[test]
    fn agents_share_a_place_by_work_tree() {
        let lab = std::env::temp_dir().join(format!("gs-overlap-{}", std::process::id()));
        let (repo, wt, plain, root, home) = (lab.join("app"), lab.join("app.worktrees").join("fix"), lab.join("notes"), lab.join("Sessions"), lab.join("home"));
        for d in [repo.join(".git"), repo.join("src"), wt.clone(), plain.join("sub"), root.join("one"), root.join("two"), home.join(".git"), home.join("scratch"), home.join("code")] {
            std::fs::create_dir_all(&d).unwrap();
        }
        // A worktree has a .git file, not a folder.
        std::fs::write(wt.join(".git"), "gitdir: elsewhere").unwrap();
        let (root_c, home_c) = (canonical(&root), canonical(&home));
        let at = |dir: &Path| scope(dir, &root_c, Some(&home_c));
        // A repository's folders are one place, however they're written; its worktree is another.
        assert_eq!(at(&repo.join("src")), at(&repo));
        assert_eq!(at(&repo.join("src").join("..")), at(&repo));
        assert_eq!(at(Path::new(&repo.display().to_string().to_uppercase())), at(&repo));
        assert_ne!(at(&wt), at(&repo));
        // Outside a repository, only the same folder is.
        assert_eq!(at(&plain), at(&plain));
        assert_ne!(at(&plain.join("sub")), at(&plain));
        // Session folders, and folders in a repository of the whole home folder, are each their own.
        assert_ne!(at(&root.join("one")), at(&root.join("two")));
        assert_ne!(at(&home.join("scratch")), at(&home.join("code")));
        assert_eq!(at(&home), at(&home));
        let _ = std::fs::remove_dir_all(&lab);
    }
}
