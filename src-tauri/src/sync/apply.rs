//! Bringing this PC in line with a merge: memories, missions and preferences are added, updated
//! and removed the way the other PC (or the merge) decided.
//!
//! Nothing is destroyed outright. A memory that goes is archived like one the user deleted, so it
//! can be restored from the Deleted view for 30 days; a note's files stay until then.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::items::{Item, ItemAttachment, MissionItem, Snapshot, parse_origin, parse_status, shared_attachments};
use super::merge::{Change, MissionChange};
use super::projects::Projects;
use super::{MAX_DOWNLOAD, NOTES_DIR, Options, safe_file_name};
use crate::memory::{self, AttachKind, Attachment, Kind, Memory, MemoryStore, State};
use crate::missions::{Mission, MissionBoard};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

/// How much of other PCs' note files a sync may copy in (copying holds Graysense up). What
/// doesn't fit is picked up by a later sync.
pub struct Budget {
    left: u64,
    /// Files that didn't fit.
    pub deferred: usize,
}

impl Budget {
    pub fn new() -> Budget {
        Budget { left: MAX_DOWNLOAD, deferred: 0 }
    }
}

fn to_memory(id: memory::MemoryId, item: &Item, projects: &Projects) -> Memory {
    Memory {
        id,
        uid: item.uid.clone(),
        kind: item.kind(),
        scope: projects.to_local(&item.scope),
        text: item.text.clone(),
        state: if item.state == "pending" { State::Pending } else { State::Active },
        origin: parse_origin(&item.origin),
        confidence: item.confidence,
        pinned: item.pinned,
        proposal: None,
        created: item.created,
        updated: item.updated,
        last_used: None,
        uses: 0,
        reinforced: 0,
        sources: item.sources.clone(),
        history: item.history.clone(),
        commands: Vec::new(),
        attachments: Vec::new(),
        votes: item.votes,
        opens: 0,
        last_opened: None,
        keywords: item.keywords.clone(),
    }
}

fn kind_of(a: &ItemAttachment) -> AttachKind {
    match a.kind.as_str() {
        "link" => AttachKind::Link,
        "folder" => AttachKind::Folder,
        _ => AttachKind::File,
    }
}

/// How an attachment is identified once it's gone through what sync can carry (`None` when it
/// can't be carried at all).
fn shared_ident(a: &Attachment) -> Option<String> {
    ItemAttachment::from_local(a).shared().map(|s| s.ident())
}

/// Copy a note's file from the repository's work tree (`repo`) into `dir`, if it's there and fits
/// the budget. Returns the copy's path.
fn copy_in(a: &ItemAttachment, uid: &str, repo: &Path, dir: &Path, budget: &mut Budget) -> Option<String> {
    let src = repo.join(NOTES_DIR).join(uid).join(a.file.as_deref().and_then(safe_file_name)?);
    let size = std::fs::metadata(&src).ok().filter(|m| m.is_file())?.len();
    if size > budget.left {
        budget.deferred += 1;
        return None;
    }
    let copy = memory::copy_into(dir, &src, &safe_file_name(&a.name).unwrap_or_else(|| "file".into()))?;
    budget.left -= size;
    Some(copy)
}

fn has_copy(a: &Attachment) -> bool {
    a.stored.as_deref().is_some_and(|p| Path::new(p).is_file())
}

/// Make `m`'s attachments the note's: ones this PC already has stay as they are, new ones bring
/// their file from the repository's work tree (`repo`) when this PC syncs files, and ones the
/// note no longer has lose their copy. What sync can't carry (an attachment on a network share,
/// the ones past the limit) was never in the repository, so the note keeps it.
fn sync_attachments(m: &mut Memory, item: &Item, repo: &Path, dir: &Path, files: bool, budget: &mut Budget) {
    let visible: HashSet<String> = shared_attachments(m.attachments.iter().map(ItemAttachment::from_local)).iter().map(ItemAttachment::ident).collect();
    let mut next: Vec<Attachment> = Vec::new();
    for a in &item.attachments {
        let ident = a.ident();
        let mut attachment = match m.attachments.iter().find(|x| shared_ident(x).as_deref() == Some(ident.as_str())) {
            Some(have) => have.clone(),
            // It came from another PC: its original location is only what it was called there.
            None => Attachment { kind: kind_of(a), name: a.name.clone(), target: a.target.clone(), stored: None, size: a.size, foreign: true },
        };
        if files && attachment.kind == AttachKind::File && !has_copy(&attachment) {
            attachment.stored = copy_in(a, &item.uid, repo, dir, budget).or(attachment.stored);
        }
        next.push(attachment);
    }
    for old in &m.attachments {
        let ident = shared_ident(old);
        if !ident.as_ref().is_some_and(|i| visible.contains(i)) {
            next.push(old.clone());
        } else if !next.iter().any(|n| shared_ident(n) == ident)
            && let Some(copy) = &old.stored
        {
            let _ = std::fs::remove_file(copy);
        }
    }
    m.attachments = next;
}

/// Copy in the files of notes that reached the repository after their note did (a sync adds only
/// so much at once, or the other PC turned files on later), and any this PC has lost. Returns how
/// many arrived.
pub fn late_files(store: &mut MemoryStore, remote: &Snapshot, repo: &Path, options: Options, budget: &mut Budget) -> usize {
    if !options.files {
        return 0;
    }
    let mut copied = 0;
    for i in 0..store.memories.len() {
        let m = &store.memories[i];
        let Some(item) = remote.memories.get(&m.uid).filter(|_| m.state != State::Archived) else { continue };
        let missing: Vec<(usize, &ItemAttachment)> = item
            .attachments
            .iter()
            .filter(|a| a.kind == "file" && a.file.is_some())
            .filter_map(|a| m.attachments.iter().position(|x| x.kind == AttachKind::File && !has_copy(x) && shared_ident(x).as_deref() == Some(a.ident().as_str())).map(|at| (at, a)))
            .collect();
        if missing.is_empty() {
            continue;
        }
        let dir = store.note_dir(m.id);
        let arrived: Vec<(usize, String)> = missing.into_iter().filter_map(|(at, a)| copy_in(a, &item.uid, repo, &dir, budget).map(|copy| (at, copy))).collect();
        copied += arrived.len();
        for (at, copy) in arrived {
            store.memories[i].attachments[at].stored = Some(copy);
        }
    }
    if copied > 0 {
        store.touch();
    }
    copied
}

/// Apply a merge's changes to the memory store. `repo` is the clone's work tree, which holds the
/// files of notes that came from another PC.
pub fn memories(store: &mut MemoryStore, changes: &[Change], projects: &Projects, repo: &Path, options: Options, budget: &mut Budget) -> Applied {
    let mut applied = Applied::default();
    for change in changes {
        match change {
            Change::Add(item) | Change::Update(item) => {
                // A memory that was deleted here and comes back (the user's own words, changed
                // elsewhere) is the same memory: restore it rather than make a second with its id.
                let existing = store.memories.iter().position(|m| m.uid == item.uid);
                match existing {
                    Some(i) => {
                        let dir = store.note_dir(store.memories[i].id);
                        let m = &mut store.memories[i];
                        // Words this PC has only differ from the item's by the cleaning every item
                        // gets (a text from before the length limit, say): they stay whole.
                        let cleaned = if m.kind == Kind::Note { memory::clean_note(&m.text) } else { memory::clean(&m.text) };
                        m.kind = item.kind();
                        m.scope = projects.to_local(&item.scope);
                        if cleaned != item.text {
                            m.text = item.text.clone();
                        }
                        m.state = if item.state == "pending" { State::Pending } else { State::Active };
                        m.origin = parse_origin(&item.origin);
                        m.confidence = item.confidence;
                        m.pinned = item.pinned;
                        m.votes = item.votes;
                        m.created = item.created;
                        m.updated = item.updated;
                        m.sources = item.sources.clone();
                        m.history = item.history.clone();
                        // A PC from before search words were learned sends none: keep this PC's.
                        if !item.keywords.is_empty() {
                            m.keywords = item.keywords.clone();
                        }
                        // Whatever it proposed was about the words it had.
                        m.proposal = None;
                        sync_attachments(m, item, repo, &dir, options.files, budget);
                        applied.updated += 1;
                    }
                    None => {
                        let id = store.alloc_id();
                        let dir = store.note_dir(id);
                        let mut m = to_memory(id, item, projects);
                        sync_attachments(&mut m, item, repo, &dir, options.files, budget);
                        store.memories.push(m);
                        applied.added += 1;
                    }
                }
            }
            Change::Remove { uid, merged } => {
                let Some(id) = store.memories.iter().find(|m| m.uid == *uid && m.state != State::Archived).map(|m| m.id) else { continue };
                store.archive(id, if *merged { "merged" } else { "deleted" });
                applied.removed += 1;
            }
        }
    }
    if !changes.is_empty() {
        store.touch();
    }
    applied
}

/// Apply a merge's changes to the mission board.
pub fn missions(board: &mut MissionBoard, changes: &[MissionChange]) -> Applied {
    let mut applied = Applied::default();
    for change in changes {
        match change {
            MissionChange::Add(item) => {
                let id = board.alloc_id();
                board.missions.insert(0, to_mission(id, item));
                applied.added += 1;
            }
            MissionChange::Update(item) => {
                let Some(m) = board.missions.iter_mut().find(|m| m.uid == item.uid) else { continue };
                // A title or notes too long to travel whole stay whole here unless the other PC changed them.
                if MissionItem::tidy_title(&m.title) != item.title {
                    m.title = item.title.clone();
                }
                if MissionItem::tidy_notes(&m.notes) != item.notes {
                    m.notes = item.notes.clone();
                }
                // A mission an agent is working on here keeps the status its work gives it.
                if m.bay.is_none() {
                    m.status = parse_status(&item.status);
                }
                applied.updated += 1;
            }
            MissionChange::Remove(uid) => {
                let before = board.missions.len();
                board.missions.retain(|m| m.uid != *uid);
                applied.removed += before - board.missions.len();
            }
        }
    }
    if !changes.is_empty() {
        board.touch();
    }
    applied
}

/// Copy `files` that exist into `dir` with the time in their names, then keep only the newest
/// `keep` copies of each. A safety net before a sync changes what's on this PC.
pub fn backup(files: &[&Path], dir: &Path, keep: usize) {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let _ = std::fs::create_dir_all(dir);
    for file in files.iter().filter(|f| f.is_file()) {
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let _ = std::fs::copy(file, dir.join(format!("{stem}-{stamp}{ext}")));
        let mut mine: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{stem}-")) && n.to_string_lossy().ends_with(&ext))).collect();
        mine.sort();
        let extra = mine.len().saturating_sub(keep);
        for old in mine.into_iter().take(extra) {
            let _ = std::fs::remove_file(old);
        }
    }
}

fn to_mission(id: u64, item: &MissionItem) -> Mission {
    Mission { id, uid: item.uid.clone(), title: item.title.clone(), notes: item.notes.clone(), dir: None, status: parse_status(&item.status), created: item.created, session_id: None, from_agent: item.from_agent, bay: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Origin, Scope};
    use crate::sync::items::tests::item;
    use crate::sync::items::{ItemScope, Snapshot};
    use std::path::PathBuf;

    const OPTS: Options = Options { files: true, settings: true, missions: true };

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gs-apply-{tag}-{}", memory::new_uid()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_in(dir: &Path) -> MemoryStore {
        MemoryStore::load_from(dir.join("memory.json"))
    }

    fn projects(dir: &Path) -> Projects {
        Projects::new(&dir.join("sessions"), Vec::new())
    }

    #[test]
    fn a_note_from_another_pc_arrives_with_its_files() {
        let dir = scratch("note");
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join("notes").join("aaaaaaaa00000001")).unwrap();
        std::fs::write(repo.join("notes").join("aaaaaaaa00000001").join("logo.png"), b"png").unwrap();
        let mut n = item("aaaaaaaa00000001", "note", "Sent Charles the dark logo");
        n.attachments.push(ItemAttachment { kind: "file".into(), name: "logo.png".into(), target: "C:\\elsewhere\\logo.png".into(), size: Some(3), file: Some("logo.png".into()), local: None });
        n.attachments.push(ItemAttachment { kind: "link".into(), name: "site".into(), target: "https://rayfin.ai/brand".into(), size: None, file: None, local: None });
        let mut store = store_in(&dir);
        let applied = memories(&mut store, &[Change::Add(n.clone())], &projects(&dir), &repo, OPTS, &mut Budget::new());
        assert_eq!(applied, Applied { added: 1, updated: 0, removed: 0 });
        let m = store.memories.iter().find(|m| m.uid == n.uid).unwrap();
        assert_eq!(m.kind, Kind::Note);
        let copy = m.attachments[0].stored.clone().unwrap();
        assert!(PathBuf::from(&copy).starts_with(store.note_dir(m.id)));
        assert_eq!(std::fs::read(&copy).unwrap(), b"png");
        assert_eq!(m.attachments[1].target, "https://rayfin.ai/brand");
        // Searchable by its file's name like any note.
        assert_eq!(store.search("logo.png", None, 5)[0].uid, n.uid);

        // A PC that doesn't sync files still gets the note, without the copy.
        let other = scratch("nofiles");
        let mut bare = store_in(&other);
        memories(&mut bare, &[Change::Add(n)], &projects(&other), &repo, Options { files: false, ..OPTS }, &mut Budget::new());
        assert_eq!(bare.memories[0].attachments[0].stored, None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn files_that_reach_the_repository_after_their_note_are_copied_in_later() {
        let dir = scratch("late");
        let repo = dir.join("repo");
        let note_dir = repo.join("notes").join("aaaaaaaa00000001");
        std::fs::create_dir_all(&note_dir).unwrap();
        // The note got to the repository first; its file came with a later sync.
        let mut n = item("aaaaaaaa00000001", "note", "Brand files");
        n.attachments.push(ItemAttachment { kind: "file".into(), name: "logo.png".into(), target: "C:\\elsewhere\\logo.png".into(), size: Some(3), file: None, local: None });
        let mut store = store_in(&dir);
        memories(&mut store, &[Change::Add(n.clone())], &projects(&dir), &repo, OPTS, &mut Budget::new());
        assert_eq!(store.memories[0].attachments[0].stored, None);

        std::fs::write(note_dir.join("logo.png"), b"png").unwrap();
        n.attachments[0].file = Some("logo.png".into());
        let remote = Snapshot { memories: [(n.uid.clone(), n.clone())].into(), ..Default::default() };
        // A PC that doesn't sync files brings in nothing.
        assert_eq!(late_files(&mut store, &remote, &repo, Options { files: false, ..OPTS }, &mut Budget::new()), 0);
        assert_eq!(late_files(&mut store, &remote, &repo, OPTS, &mut Budget::new()), 1);
        let copy = store.memories[0].attachments[0].stored.clone().unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"png");
        // Once it's here there's nothing more to bring.
        assert_eq!(late_files(&mut store, &remote, &repo, OPTS, &mut Budget::new()), 0);

        // A copy this PC lost is brought back the same way.
        std::fs::remove_file(&copy).unwrap();
        assert_eq!(late_files(&mut store, &remote, &repo, OPTS, &mut Budget::new()), 1);
        assert!(store.memories[0].attachments[0].stored.as_deref().is_some_and(|p| Path::new(p).is_file()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sync_copies_only_so_much_and_the_rest_comes_next_time() {
        let dir = scratch("budget");
        let repo = dir.join("repo");
        let note_dir = repo.join("notes").join("aaaaaaaa00000001");
        std::fs::create_dir_all(&note_dir).unwrap();
        let mut n = item("aaaaaaaa00000001", "note", "Two files");
        for name in ["one.bin", "two.bin"] {
            std::fs::write(note_dir.join(name), b"123").unwrap();
            n.attachments.push(ItemAttachment { kind: "file".into(), name: name.into(), target: format!("C:\\x\\{name}"), size: Some(3), file: Some(name.into()), local: None });
        }
        let mut store = store_in(&dir);
        let mut budget = Budget { left: 4, deferred: 0 };
        memories(&mut store, &[Change::Add(n.clone())], &projects(&dir), &repo, OPTS, &mut budget);
        assert_eq!(budget.deferred, 1);
        let stored: Vec<bool> = store.memories[0].attachments.iter().map(has_copy).collect();
        assert_eq!(stored, [true, false]);

        let remote = Snapshot { memories: [(n.uid.clone(), n)].into(), ..Default::default() };
        assert_eq!(late_files(&mut store, &remote, &repo, OPTS, &mut Budget::new()), 1);
        assert!(store.memories[0].attachments.iter().all(has_copy));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_sync_cannot_carry_stays_with_the_note_when_another_pc_changes_it() {
        let dir = scratch("hidden");
        let mut store = store_in(&dir);
        let id = store.insert(Kind::Note, Scope::Global, "Plan for the share", Origin::User, State::Active, 1.0, None).unwrap();
        store.ensure_uids();
        let uid = store.get(id).unwrap().uid.clone();
        let link = |n: usize| Attachment { kind: AttachKind::Link, name: format!("site {n}"), target: format!("https://rayfin.ai/{n}"), stored: None, size: None, foreign: false };
        {
            // A folder on a network share never travels, and neither does anything past the 40th attachment.
            let m = store.get_mut(id).unwrap();
            m.attachments.push(Attachment { kind: AttachKind::Folder, name: "plan".into(), target: "\\\\nas\\share\\plan".into(), stored: None, size: None, foreign: false });
            m.attachments.extend((1..=41).map(link));
        }
        let p = projects(&dir);
        let local = Snapshot::local(store.syncable(), &[], Default::default(), &p);
        assert_eq!(local.memories[&uid].attachments.len(), 40);

        // The other PC edited the words and removed the first link.
        let mut changed = local.memories[&uid].clone();
        changed.text = "Plan for the share, revised".into();
        changed.attachments.remove(0);
        memories(&mut store, &[Change::Update(changed)], &p, &dir, OPTS, &mut Budget::new());
        let m = store.get(id).unwrap();
        assert_eq!(m.text, "Plan for the share, revised");
        // 39 links travelled and stayed; the removed one is gone; the folder and the 41st link were never theirs to remove.
        let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names.len(), 41);
        assert!(!names.contains(&"site 1") && names.contains(&"plan") && names.contains(&"site 41") && names.contains(&"site 2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_another_pc_names_is_never_handed_to_an_agent_from_here() {
        let dir = scratch("foreign");
        // A file that exists on this PC, named as the original of an attachment in a note from the repository.
        let private = dir.join("id_rsa");
        std::fs::write(&private, b"private").unwrap();
        let repo = dir.join("repo");
        let mut n = item("aaaaaaaa00000001", "note", "Q3 plan");
        n.attachments.push(ItemAttachment { kind: "file".into(), name: "Q3 plan.txt".into(), target: private.display().to_string(), size: Some(7), file: None, local: None });
        let mut store = store_in(&dir);
        memories(&mut store, &[Change::Add(n)], &projects(&dir), &repo, OPTS, &mut Budget::new());
        let a = store.memories[0].attachments[0].clone();
        assert!(a.foreign);
        // The user can still open it themselves (they see what it is); no agent is given it, and its
        // location is not offered to the agent either.
        assert_eq!(a.open_path(), Some(private.clone()));
        assert_eq!(a.trusted_path(), None);
        assert!(memory::MemoryStore::note_for_agent(&store.memories[0]).contains("Q3 plan.txt (not on this PC)"));
        assert!(!memory::MemoryStore::note_for_agent(&store.memories[0]).contains("id_rsa"));
        // What the user attached on this PC themselves is theirs to give, and so is a copy Graysense made.
        assert_eq!(Attachment { foreign: false, ..a.clone() }.trusted_path(), Some(private.clone()));
        let copy = dir.join("copy.txt");
        std::fs::write(&copy, b"copy").unwrap();
        assert_eq!(Attachment { stored: Some(copy.display().to_string()), ..a }.trusted_path(), Some(copy));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_changes_what_was_synced_and_keeps_what_belongs_to_this_pc() {
        let dir = scratch("update");
        let mut store = store_in(&dir);
        let id = store.insert(Kind::Preference, Scope::Global, "Prefers pnpm", Origin::Learned, State::Active, 0.9, None).unwrap();
        store.mark_used(&[id]);
        store.mark_used(&[id]);
        store.get_mut(id).unwrap().proposal = Some(memory::Proposal::Delete);
        store.get_mut(id).unwrap().keywords = vec!["package manager".into()];
        store.ensure_uids();
        let uid = store.get(id).unwrap().uid.clone();
        let mut changed = item(&uid, "preference", "Prefers yarn");
        changed.origin = "user".into();
        changed.pinned = true;
        changed.scope = ItemScope::Project { name: "app".into(), remote: Some("github.com/me/app".into()), key: None };
        let applied = memories(&mut store, &[Change::Update(changed)], &projects(&dir), &dir, OPTS, &mut Budget::new());
        assert_eq!(applied.updated, 1);
        let m = store.get(id).unwrap();
        assert_eq!((m.text.as_str(), m.origin, m.pinned), ("Prefers yarn", Origin::User, true));
        // A project with no folder here waits under its remote.
        assert_eq!(m.scope, Scope::Project("git:github.com/me/app".into()));
        // Its use here is this PC's own business; the proposal was about the old words.
        assert_eq!(m.uses, 2);
        assert!(m.proposal.is_none());
        // A PC that doesn't learn search words yet doesn't wipe this one's.
        assert_eq!(m.keywords, ["package manager"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_removed_memory_can_be_restored_and_only_a_deletion_stops_it_coming_back() {
        let dir = scratch("remove");
        let mut store = store_in(&dir);
        let gone = store.insert(Kind::Fact, Scope::Global, "Deploys go out on Fridays", Origin::Learned, State::Active, 0.9, None).unwrap();
        let combined = store.insert(Kind::Fact, Scope::Global, "The API runs on port 8080", Origin::Learned, State::Active, 0.9, None).unwrap();
        store.ensure_uids();
        let (a, b) = (store.get(gone).unwrap().uid.clone(), store.get(combined).unwrap().uid.clone());
        let applied = memories(&mut store, &[Change::Remove { uid: a, merged: false }, Change::Remove { uid: b, merged: true }], &projects(&dir), &dir, OPTS, &mut Budget::new());
        assert_eq!(applied.removed, 2);
        assert!(store.syncable().next().is_none());
        assert_eq!(store.get(gone).unwrap().archived_reason().0, "Deleted");
        // Deleted memories aren't relearned; combined ones are just gone.
        assert_eq!(store.rejected_texts(), vec!["deploys go out on fridays".to_owned()]);
        store.restore(gone);
        assert_eq!(store.syncable().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_memory_deleted_here_and_changed_elsewhere_comes_back_as_itself() {
        let dir = scratch("revive");
        let mut store = store_in(&dir);
        let id = store.insert(Kind::Fact, Scope::Global, "Staging password rotates monthly", Origin::User, State::Active, 1.0, None).unwrap();
        store.ensure_uids();
        let uid = store.get(id).unwrap().uid.clone();
        store.archive(id, "deleted");
        let back = Item { origin: "user".into(), ..item(&uid, "fact", "Staging credentials rotate monthly") };
        memories(&mut store, &[Change::Add(back)], &projects(&dir), &dir, OPTS, &mut Budget::new());
        // One memory with that id, active again, with the other PC's words.
        let same: Vec<&Memory> = store.memories.iter().filter(|m| m.uid == uid).collect();
        assert_eq!(same.len(), 1);
        assert_eq!((same[0].id, same[0].state, same[0].text.as_str()), (id, State::Active, "Staging credentials rotate monthly"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missions_are_added_changed_and_removed() {
        let dir = scratch("missions");
        let mut board = MissionBoard::load_from(dir.join("missions.json"));
        board.add("Fix the login bug", "", false);
        board.add("Old idea", "", false);
        board.ensure_uids();
        let (fix, old) = (board.missions.iter().find(|m| m.title == "Fix the login bug").unwrap().clone(), board.missions.iter().find(|m| m.title == "Old idea").unwrap().uid.clone());
        let item = |uid: &str, title: &str, status: &str| MissionItem { uid: uid.into(), title: title.into(), notes: "n".into(), status: status.into(), created: 5, from_agent: true };
        let applied = missions(&mut board, &[MissionChange::Add(item("bbbbbbbb00000002", "New idea", "idea")), MissionChange::Update(item(&fix.uid, "Fix the login bug on CI", "done")), MissionChange::Remove(old)]);
        assert_eq!(applied, Applied { added: 1, updated: 1, removed: 1 });
        let titles: Vec<&str> = board.missions.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["New idea", "Fix the login bug on CI"]);
        assert_eq!(board.missions[1].status, crate::missions::Status::Done);

        // A mission an agent is working on here keeps its status.
        board.link(fix.id, 7);
        missions(&mut board, &[MissionChange::Update(item(&fix.uid, "Fix the login bug on CI", "idea"))]);
        assert_eq!(board.missions[1].status, crate::missions::Status::Active);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mission_too_long_to_travel_whole_is_not_cut_here_when_only_its_status_changes() {
        let dir = scratch("long");
        let mut board = MissionBoard::load_from(dir.join("missions.json"));
        let notes = "word ".repeat(2_000);
        board.add("Big idea", &notes, false);
        board.ensure_uids();
        let sent = MissionItem::from_mission(&board.missions[0]).unwrap();
        assert!(sent.notes.chars().count() <= 8_000 && sent.notes.len() < notes.trim().len());
        // The other PC only moved it to done: what it sent back is the cut copy, but the words here stay whole.
        missions(&mut board, &[MissionChange::Update(MissionItem { status: "done".into(), ..sent.clone() })]);
        assert_eq!(board.missions[0].notes, notes.trim());
        assert_eq!(board.missions[0].status, crate::missions::Status::Done);
        // When they did change the words, theirs are taken.
        missions(&mut board, &[MissionChange::Update(MissionItem { notes: "shorter".into(), ..sent })]);
        assert_eq!(board.missions[0].notes, "shorter");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applying_the_merge_of_two_pcs_makes_this_pc_match_the_result() {
        // End to end through the merge: what this PC applies equals what the repository gets.
        let dir = scratch("e2e");
        let mut store = store_in(&dir);
        store.insert(Kind::Preference, Scope::Global, "Prefers pnpm over npm", Origin::Learned, State::Active, 0.9, None);
        store.ensure_uids();
        let p = projects(&dir);
        let local = Snapshot::local(store.syncable(), &[], Default::default(), &p);
        let mut remote = Snapshot::default();
        let other = item("remote00000000001", "fact", "The API runs on port 8080");
        remote.memories.insert(other.uid.clone(), other);
        let base = Default::default();
        let defaults = Default::default();
        let merged = crate::sync::merge::merge(&crate::sync::merge::Inputs { base: &base, local: &local, remote: &remote, unreadable: &Default::default(), rejected: &[], defaults: &defaults, options: OPTS, strategy: crate::sync::merge::Strategy::Merge });
        memories(&mut store, &merged.changes, &p, &dir, OPTS, &mut Budget::new());
        let after = Snapshot::local(store.syncable(), &[], Default::default(), &p);
        assert_eq!(after.base().memories, merged.snapshot.base().memories);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
