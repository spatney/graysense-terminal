//! What sync keeps in the repository for each memory, note and mission, in a form that is the
//! same on every PC: a uid instead of a local id, a scope other PCs can resolve, and none of the
//! counters that only describe one PC's use (how often a memory was recalled, opened…).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::projects::Projects;
use super::{Counts, fnv, is_local_path, safe_file_name};
use crate::memory::{self, AttachKind, Attachment, Kind, Memory, Origin, Revision, Source, State};
use crate::missions::{Mission, Status};

/// Where a memory applies, in terms every PC can resolve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ItemScope {
    Global,
    /// The sessions folder, which every session folder shares.
    Sessions,
    /// A folder. `remote` (`github.com/owner/repo`) is what other PCs find it by; `key` is where
    /// it was on the PC that saved it.
    Project {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
}

impl ItemScope {
    /// What two scopes are compared by.
    pub fn id(&self) -> String {
        match self {
            ItemScope::Global => "global".into(),
            ItemScope::Sessions => "sessions".into(),
            ItemScope::Project { remote: Some(r), .. } => format!("git:{r}"),
            ItemScope::Project { key: Some(k), .. } => format!("path:{k}"),
            ItemScope::Project { name, .. } => format!("name:{name}"),
        }
    }

    /// How people and the AI read it.
    pub fn label(&self) -> String {
        match self {
            ItemScope::Global => "about the user".into(),
            ItemScope::Sessions => "the sessions folder".into(),
            ItemScope::Project { name, .. } => format!("the project {name}"),
        }
    }
}

/// A file, folder or link kept with a note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemAttachment {
    /// file, folder or link.
    pub kind: String,
    pub name: String,
    /// The original path, or the URL.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The file's name inside the note's folder in the repository, once it's there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Graysense's copy on this PC. Never written to the repository.
    #[serde(skip)]
    pub local: Option<PathBuf>,
}

impl ItemAttachment {
    pub fn from_local(a: &Attachment) -> Self {
        ItemAttachment {
            kind: match a.kind {
                AttachKind::File => "file",
                AttachKind::Folder => "folder",
                AttachKind::Link => "link",
            }
            .into(),
            name: a.name.clone(),
            target: a.target.clone(),
            size: a.size,
            file: None,
            local: a.stored.as_ref().map(PathBuf::from).filter(|p| p.is_file()),
        }
    }

    /// What makes two attachments the same one.
    pub fn ident(&self) -> String {
        match self.kind.as_str() {
            "link" => format!("link:{}", self.target),
            "folder" => format!("folder:{}", self.target),
            _ => format!("file:{}:{}", self.name, self.size.unwrap_or(0)),
        }
    }

    /// The attachment as it travels, the same in either direction, or `None` when it can't (it
    /// stays on the PC that has it). A file's or folder's original location only means something on
    /// the PC it was on, and a path that leads off this PC (a network share) is never kept: it
    /// would be opened from here.
    pub fn shared(mut self) -> Option<ItemAttachment> {
        if !["file", "folder", "link"].contains(&self.kind.as_str()) {
            return None;
        }
        self.name = self.name.chars().filter(|c| !c.is_control()).take(200).collect();
        self.target = self.target.chars().filter(|c| !c.is_control()).take(2_000).collect();
        if self.name.is_empty() || self.target.is_empty() {
            return None;
        }
        match self.kind.as_str() {
            "link" if !(self.target.starts_with("https://") || self.target.starts_with("http://")) => return None,
            "folder" if !is_local_path(&self.target) => return None,
            "file" if !is_local_path(&self.target) => self.target = self.name.clone(),
            _ => {}
        }
        let wanted = self.file.take();
        self.file = wanted.as_deref().and_then(safe_file_name).filter(|f| self.kind == "file" && Some(f.as_str()) == wanted.as_deref());
        Some(self)
    }
}

/// A note's attachments as they travel: the ones that can, the first few.
pub fn shared_attachments(list: impl IntoIterator<Item = ItemAttachment>) -> Vec<ItemAttachment> {
    list.into_iter().filter_map(ItemAttachment::shared).take(MAX_ATTACHMENTS).collect()
}

/// One memory or note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub uid: String,
    /// preference, fact, procedure, episode or note.
    pub kind: String,
    pub scope: ItemScope,
    pub text: String,
    /// active, or pending (a suggestion waiting for review).
    #[serde(default = "active")]
    pub state: String,
    /// user, learned or agent.
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub votes: i32,
    pub created: i64,
    pub updated: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ItemAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Revision>,
    /// Search words learning gave it (see [`Memory::keywords`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

fn active() -> String {
    "active".into()
}

fn origin_name(o: Origin) -> &'static str {
    match o {
        Origin::User => "user",
        Origin::Learned => "learned",
        Origin::Agent => "agent",
    }
}

pub fn parse_origin(s: &str) -> Origin {
    match s {
        "user" => Origin::User,
        "agent" => Origin::Agent,
        _ => Origin::Learned,
    }
}

const MAX_ATTACHMENTS: usize = 40;
const MAX_SOURCES: usize = 5;
const MAX_HISTORY: usize = 8;

impl Item {
    /// A memory as it goes to the repository. `None` for what never leaves this PC (approval
    /// rules, deleted memories) and for what can't be shared (it looks like it holds a secret):
    /// those stay here, and the repository's copy of one that was shared before is left alone. It's
    /// cleaned by the rules everything that comes back is, so an item that went to the repository
    /// and returned is the one that left.
    pub fn from_memory(m: &Memory, projects: &Projects) -> Option<Item> {
        if m.state == State::Archived || m.kind == Kind::Rule || !memory::valid_uid(&m.uid) || m.text.trim().is_empty() || memory::looks_secret(&m.text) {
            return None;
        }
        Item {
            uid: m.uid.clone(),
            kind: m.kind.name().into(),
            scope: projects.to_item(&m.scope),
            text: m.text.clone(),
            state: if m.state == State::Pending { "pending" } else { "active" }.into(),
            origin: origin_name(m.origin).into(),
            confidence: m.confidence,
            pinned: m.pinned,
            votes: m.votes,
            created: m.created,
            updated: m.updated,
            attachments: m.attachments.iter().map(ItemAttachment::from_local).collect(),
            sources: m.sources.iter().rev().take(MAX_SOURCES).rev().filter_map(shareable_source).collect(),
            history: m.history.iter().rev().take(MAX_HISTORY).rev().filter(|r| !memory::looks_secret(&r.text)).cloned().collect(),
            keywords: m.keywords.clone(),
        }
        .normalized()
        .ok()
    }

    pub fn kind(&self) -> Kind {
        Kind::parse(&self.kind).unwrap_or(Kind::Fact)
    }

    pub fn is_note(&self) -> bool {
        self.kind == "note"
    }

    /// Something the user wrote, edited, pinned or saved as a note: never rewritten or dropped
    /// without them (the same rule learning follows).
    pub fn protected(&self) -> bool {
        self.is_note() || self.pinned || self.origin == "user" || self.history.iter().any(|r| r.why == "edited" || r.why == "restored")
    }

    /// Changes whenever something a person would call a change does: what it says, where it
    /// applies, its state, pin and votes, and its files. Not when it was last recalled or
    /// reinforced, or how sure the curator was, so using memory doesn't make every PC re-sync it.
    pub fn hash(&self) -> String {
        let mut attachments: Vec<String> = self.attachments.iter().map(ItemAttachment::ident).collect();
        attachments.sort();
        let votes = self.votes.to_string();
        fnv(&[&self.kind, &self.scope.id(), &self.text, &self.state, if self.pinned { "1" } else { "0" }, &votes, &attachments.join("\u{1e}")])
    }

    /// The file the repository keeps it in.
    pub fn file_name(&self) -> String {
        format!("{}.json", self.uid)
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }

    /// An item read from the repository, made safe to use: anyone who can write to the repository
    /// could have written it. Fails for what can't be trusted at all.
    pub fn sanitized(mut self) -> Result<Item, String> {
        for a in &mut self.attachments {
            a.local = None;
        }
        self.normalized()
    }

    /// The item as it travels, whichever way it's going (see [`Item::from_memory`]).
    fn normalized(mut self) -> Result<Item, String> {
        if !memory::valid_uid(&self.uid) {
            return Err("it has no valid id".into());
        }
        let kind = Kind::parse(&self.kind).ok_or("it isn't a kind of memory Graysense knows")?;
        self.kind = kind.name().into();
        let tidy = |text: &str| if kind == Kind::Note { memory::clean_note(text) } else { memory::clean(text) };
        self.text = tidy(&self.text);
        if self.text.is_empty() {
            return Err("it says nothing".into());
        }
        if memory::looks_secret(&self.text) {
            return Err("it looks like it holds a secret".into());
        }
        self.state = if self.state == "pending" { "pending" } else { "active" }.into();
        self.origin = origin_name(parse_origin(&self.origin)).into();
        self.confidence = if self.confidence.is_finite() { self.confidence.clamp(0.0, 1.0) } else { 0.5 };
        self.votes = self.votes.clamp(-5, 10);
        let now = memory::now();
        if self.created <= 0 || self.created > now + 86_400 {
            self.created = now;
        }
        if self.updated < self.created || self.updated > now + 86_400 {
            self.updated = self.created;
        }
        self.scope = match self.scope {
            ItemScope::Project { name, remote, key } => {
                let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
                let remote = remote.filter(|r| r.len() <= 200 && !r.chars().any(|c| c.is_whitespace() || c.is_control()));
                // The path is only what the project is called here: nothing reads it (see `git_remote`).
                let key = key.filter(|k| k.len() <= 400 && !k.chars().any(char::is_control));
                if remote.is_none() && key.is_none() {
                    return Err("its project is unknown".into());
                }
                ItemScope::Project { name, remote, key }
            }
            other => other,
        };
        self.attachments = shared_attachments(self.attachments.drain(..));
        self.sources = self.sources.drain(..).filter_map(|s| shareable_source(&s)).take(MAX_SOURCES).collect();
        self.history = self
            .history
            .drain(..)
            .take(MAX_HISTORY)
            .map(|r| Revision { text: tidy(&r.text), at: r.at, why: r.why.chars().filter(|c| !c.is_control()).take(80).collect() })
            .filter(|r| !r.text.is_empty() && !memory::looks_secret(&r.text))
            .collect();
        self.keywords = memory::clean_keywords(&self.keywords, &self.text);
        Ok(self)
    }
}

/// A source as it may leave this PC: the words quoted are shortened and anything that looks like a
/// credential is blotted out (a quote can be a whole prompt the user typed).
fn shareable_source(s: &Source) -> Option<Source> {
    let short = |t: &str, max: usize| -> Option<String> {
        let t = memory::redact_secrets(t.trim());
        let t: String = t.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect();
        (!t.trim().is_empty()).then_some(t)
    };
    Some(Source { session: s.session.clone().filter(|id| id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')), title: s.title.as_deref().and_then(|t| short(t, 120)), quote: s.quote.as_deref().and_then(|q| short(q, 240)), at: s.at })
}

/// One mission on the board.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionItem {
    pub uid: String,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    /// idea, active, review or done.
    pub status: String,
    pub created: i64,
    #[serde(default)]
    pub from_agent: bool,
}

pub fn status_name(s: Status) -> &'static str {
    match s {
        Status::Idea => "idea",
        Status::Active => "active",
        Status::Review => "review",
        Status::Done => "done",
    }
}

pub fn parse_status(s: &str) -> Status {
    match s {
        "active" => Status::Active,
        "review" => Status::Review,
        "done" => Status::Done,
        _ => Status::Idea,
    }
}

impl MissionItem {
    /// A mission as it goes to the repository. `None` when it can't be shared (no title, or it
    /// looks like it holds a secret): it stays here, and the repository's copy of one that was
    /// shared before is left alone. Cleaned by the same rules as what comes back.
    pub fn from_mission(m: &Mission) -> Option<MissionItem> {
        if !memory::valid_uid(&m.uid) || m.title.trim().is_empty() || memory::looks_secret(&m.title) || memory::looks_secret(&m.notes) {
            return None;
        }
        MissionItem { uid: m.uid.clone(), title: m.title.clone(), notes: m.notes.clone(), status: status_name(m.status).into(), created: m.created, from_agent: m.from_agent }.normalized().ok()
    }

    /// A title as it travels.
    pub fn tidy_title(title: &str) -> String {
        title.chars().filter(|c| !c.is_control()).take(200).collect::<String>().trim().to_owned()
    }

    /// Notes as they travel.
    pub fn tidy_notes(notes: &str) -> String {
        memory::clean_note(notes)
    }

    pub fn hash(&self) -> String {
        fnv(&[&self.title, &self.notes, &self.status])
    }

    pub fn file_name(&self) -> String {
        format!("{}.json", self.uid)
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }

    /// A mission read from the repository, made safe to use.
    pub fn sanitized(self) -> Result<MissionItem, String> {
        self.normalized()
    }

    fn normalized(mut self) -> Result<MissionItem, String> {
        if !memory::valid_uid(&self.uid) {
            return Err("it has no valid id".into());
        }
        self.title = Self::tidy_title(&self.title);
        self.notes = Self::tidy_notes(&self.notes);
        if self.title.is_empty() {
            return Err("it has no title".into());
        }
        if memory::looks_secret(&self.title) || memory::looks_secret(&self.notes) {
            return Err("it looks like it holds a secret".into());
        }
        self.status = status_name(parse_status(&self.status)).into();
        let now = memory::now();
        if self.created <= 0 || self.created > now + 86_400 {
            self.created = now;
        }
        Ok(self)
    }
}

/// What the repository holds, or what this PC would put there: the same shape either way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub memories: BTreeMap<String, Item>,
    pub missions: BTreeMap<String, MissionItem>,
    pub settings: BTreeMap<String, Value>,
}

/// A hash per item: what a PC keeps to know, at the next sync, what changed since this one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Base {
    pub memories: BTreeMap<String, String>,
    pub missions: BTreeMap<String, String>,
    pub settings: BTreeMap<String, String>,
}

impl Snapshot {
    /// This PC's memories, missions and preferences as the repository would hold them. Memories
    /// need uids (`MemoryStore::ensure_uids`).
    #[cfg(test)]
    pub fn local<'a>(memories: impl IntoIterator<Item = &'a Memory>, missions: &[Mission], settings: BTreeMap<String, Value>, projects: &Projects) -> Snapshot {
        Self::local_with_withheld(memories, missions, settings, projects).0
    }

    /// [`Snapshot::local`], and the uids of what this PC has but can't share (it looks like it holds
    /// a secret). Those aren't missing: they're only not here for the repository's copy, if it has
    /// one, to be compared with.
    pub fn local_with_withheld<'a>(memories: impl IntoIterator<Item = &'a Memory>, missions: &[Mission], settings: BTreeMap<String, Value>, projects: &Projects) -> (Snapshot, HashSet<String>) {
        let mut snapshot = Snapshot { settings, ..Snapshot::default() };
        let mut withheld = HashSet::new();
        for m in memories {
            match Item::from_memory(m, projects) {
                Some(i) => {
                    snapshot.memories.insert(i.uid.clone(), i);
                }
                None => {
                    withheld.insert(m.uid.clone());
                }
            }
        }
        for m in missions {
            match MissionItem::from_mission(m) {
                Some(i) => {
                    snapshot.missions.insert(i.uid.clone(), i);
                }
                None => {
                    withheld.insert(m.uid.clone());
                }
            }
        }
        (snapshot, withheld)
    }

    pub fn base(&self) -> Base {
        Base {
            memories: self.memories.iter().map(|(k, v)| (k.clone(), v.hash())).collect(),
            missions: self.missions.iter().map(|(k, v)| (k.clone(), v.hash())).collect(),
            settings: super::hash_settings(&self.settings),
        }
    }

    /// This snapshot without the memories and missions of `uids`.
    pub fn without(&self, uids: &HashSet<String>) -> Snapshot {
        Snapshot {
            memories: self.memories.iter().filter(|(uid, _)| !uids.contains(*uid)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            missions: self.missions.iter().filter(|(uid, _)| !uids.contains(*uid)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            settings: self.settings.clone(),
        }
    }

    /// Only the memories and missions of `uids` (no preferences).
    pub fn only(&self, uids: &HashSet<String>) -> Snapshot {
        Snapshot {
            memories: self.memories.iter().filter(|(uid, _)| uids.contains(*uid)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            missions: self.missions.iter().filter(|(uid, _)| uids.contains(*uid)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            settings: BTreeMap::new(),
        }
    }

    pub fn counts(&self) -> Counts {
        let notes = self.memories.values().filter(|i| i.is_note()).count();
        Counts { memories: self.memories.len() - notes, notes, missions: self.missions.len() }
    }

    /// Nothing a person made (preferences alone don't count).
    pub fn is_empty(&self) -> bool {
        self.memories.is_empty() && self.missions.is_empty()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::memory::Scope;

    pub fn memory(uid: &str, kind: Kind, text: &str) -> Memory {
        Memory {
            id: 1,
            uid: uid.into(),
            kind,
            scope: Scope::Global,
            text: text.into(),
            state: State::Active,
            origin: Origin::Learned,
            confidence: 0.9,
            pinned: false,
            proposal: None,
            created: 1_700_000_000,
            updated: 1_700_000_100,
            last_used: None,
            uses: 0,
            reinforced: 0,
            sources: Vec::new(),
            history: Vec::new(),
            commands: Vec::new(),
            attachments: Vec::new(),
            votes: 0,
            opens: 0,
            last_opened: None,
            keywords: Vec::new(),
        }
    }

    pub fn item(uid: &str, kind: &str, text: &str) -> Item {
        Item {
            uid: uid.into(),
            kind: kind.into(),
            scope: ItemScope::Global,
            text: text.into(),
            state: "active".into(),
            origin: "learned".into(),
            confidence: 0.9,
            pinned: false,
            votes: 0,
            created: 1_700_000_000,
            updated: 1_700_000_100,
            attachments: Vec::new(),
            sources: Vec::new(),
            history: Vec::new(),
            keywords: Vec::new(),
        }
    }

    fn projects() -> Projects {
        Projects::new(std::path::Path::new("c:\\sessions"), Vec::new())
    }

    #[test]
    fn using_a_memory_does_not_change_it_but_editing_does() {
        let p = projects();
        let mut m = memory("aaaaaaaa11111111", Kind::Preference, "Prefers pnpm");
        let before = Item::from_memory(&m, &p).unwrap().hash();
        m.uses = 9;
        m.reinforced = 4;
        m.confidence = 0.5;
        m.last_used = Some(5);
        m.opens = 3;
        m.updated += 500;
        assert_eq!(Item::from_memory(&m, &p).unwrap().hash(), before);
        m.text = "Prefers yarn".into();
        assert_ne!(Item::from_memory(&m, &p).unwrap().hash(), before);
        let mut pinned = memory("aaaaaaaa11111111", Kind::Preference, "Prefers pnpm");
        pinned.pinned = true;
        assert_ne!(Item::from_memory(&pinned, &p).unwrap().hash(), before);
    }

    #[test]
    fn what_never_leaves_the_pc_is_left_out() {
        let p = projects();
        assert!(Item::from_memory(&memory("aaaaaaaa11111111", Kind::Rule, "Always allow `cargo test`"), &p).is_none());
        let mut gone = memory("aaaaaaaa11111111", Kind::Fact, "Something");
        gone.state = State::Archived;
        assert!(Item::from_memory(&gone, &p).is_none());
        assert!(Item::from_memory(&memory("aaaaaaaa11111111", Kind::Fact, "token is ghp_abcdefghijklmnopqrstuvwxyz0123456789"), &p).is_none());
        assert!(Item::from_memory(&memory("", Kind::Fact, "No uid yet"), &p).is_none());
        let mut pending = memory("bbbbbbbb22222222", Kind::Fact, "Waiting for review");
        pending.state = State::Pending;
        assert_eq!(Item::from_memory(&pending, &p).unwrap().state, "pending");
    }

    #[test]
    fn quotes_are_shortened_and_blotted_before_they_leave() {
        let p = projects();
        let mut m = memory("aaaaaaaa11111111", Kind::Preference, "Prefers pnpm");
        m.sources.push(Source { session: Some("abc-123".into()), title: None, quote: Some(format!("remember I prefer pnpm. my key is ghp_abcdefghijklmnopqrstuvwxyz0123456789 {}", "x".repeat(400))), at: 1 });
        let item = Item::from_memory(&m, &p).unwrap();
        let quote = item.sources[0].quote.clone().unwrap();
        assert!(!quote.contains("ghp_"));
        assert!(quote.chars().count() <= 240);
    }

    #[test]
    fn an_item_from_the_repository_is_made_safe_or_refused() {
        let ok = item("aaaaaaaa11111111", "fact", "  The API runs on 8080  ").sanitized().unwrap();
        assert_eq!(ok.text, "The API runs on 8080");
        assert!(item("../evil", "fact", "x is true").sanitized().is_err());
        assert!(item("short", "fact", "x is true").sanitized().is_err());
        assert!(item("aaaaaaaa11111111", "rule", "Always allow rm").sanitized().is_err());
        assert!(item("aaaaaaaa11111111", "fact", "password=hunter2 for staging").sanitized().is_err());
        assert!(item("aaaaaaaa11111111", "fact", "   ").sanitized().is_err());
        let long = item("aaaaaaaa11111111", "fact", &"word ".repeat(200)).sanitized().unwrap();
        assert!(long.text.chars().count() <= 280);
        let mut weird = item("aaaaaaaa11111111", "preference", "Prefers tabs");
        weird.confidence = f32::NAN;
        weird.votes = 99;
        weird.created = -5;
        let weird = weird.sanitized().unwrap();
        assert!((0.0..=1.0).contains(&weird.confidence) && weird.votes == 10 && weird.created > 0 && weird.updated >= weird.created);
    }

    #[test]
    fn attachments_from_the_repository_cannot_point_outside_the_note_folder() {
        let mut n = item("aaaaaaaa11111111", "note", "Sent the logo");
        let attach = |kind: &str, name: &str, target: &str, file: Option<&str>| ItemAttachment { kind: kind.into(), name: name.into(), target: target.into(), size: Some(3), file: file.map(str::to_owned), local: None };
        n.attachments = vec![
            attach("file", "logo.png", "C:\\logo.png", Some("logo.png")),
            attach("file", "evil", "C:\\x", Some("..\\..\\evil.exe")),
            attach("link", "site", "javascript:alert(1)", None),
            attach("link", "docs", "https://docs.rs/serde", None),
            attach("weird", "x", "y", None),
        ];
        let n = n.sanitized().unwrap();
        assert_eq!(n.attachments.len(), 3);
        assert_eq!(n.attachments[0].file.as_deref(), Some("logo.png"));
        assert_eq!(n.attachments[1].file, None);
        assert_eq!(n.attachments[2].target, "https://docs.rs/serde");
    }

    #[test]
    fn a_project_scope_needs_a_way_to_find_it() {
        let mut i = item("aaaaaaaa11111111", "fact", "Builds with just");
        i.scope = ItemScope::Project { name: "x".into(), remote: None, key: None };
        assert!(i.sanitized().is_err());
    }

    #[test]
    fn a_path_that_leads_off_the_pc_is_never_taken_from_the_repository() {
        // A project's path is only its name here (WSL and network folders keep theirs): nothing reads it.
        let mut i = item("aaaaaaaa11111111", "fact", "Builds with just");
        i.scope = ItemScope::Project { name: "x".into(), remote: None, key: Some("\\\\wsl.localhost\\ubuntu\\home\\me\\app".into()) };
        assert!(i.sanitized().is_ok());

        // A note's file keeps only its name as where it came from, and a folder on a share isn't carried at all:
        // those paths are opened from the note.
        let mut n = item("aaaaaaaa11111111", "note", "Plan");
        let attach = |kind: &str, target: &str| ItemAttachment { kind: kind.into(), name: "plan".into(), target: target.into(), size: Some(3), file: None, local: None };
        n.attachments = vec![attach("file", "\\\\evil.example.com\\share\\plan.pdf"), attach("folder", "\\\\evil.example.com\\share"), attach("folder", "d:\\plans")];
        let n = n.sanitized().unwrap();
        assert_eq!(n.attachments.iter().map(|a| (a.kind.as_str(), a.target.as_str())).collect::<Vec<_>>(), [("file", "plan"), ("folder", "d:\\plans")]);
    }

    #[test]
    fn what_leaves_this_pc_comes_back_as_it_left() {
        // Whatever this PC would put in the repository is already what reading it back gives: a
        // sync that finds the two different would treat the read-back copy as the newer one.
        let p = projects();
        let mut note = memory("aaaaaaaa11111111", Kind::Note, "Plan\n\n\n\nfor the share  ");
        note.attachments.push(Attachment { kind: AttachKind::File, name: "plan.pdf".into(), target: "\\\\nas\\share\\plan.pdf".into(), stored: None, size: Some(9), foreign: false });
        note.attachments.push(Attachment { kind: AttachKind::Folder, name: "share".into(), target: "\\\\nas\\share".into(), stored: None, size: None, foreign: false });
        note.attachments.extend((0..45).map(|n| Attachment { kind: AttachKind::Link, name: format!("l{n}"), target: format!("https://rayfin.ai/{n}"), stored: None, size: None, foreign: false }));
        note.votes = 7;
        note.sources = (0..9).map(|n| Source { session: None, title: Some(format!("t{n}")), quote: Some("q".into()), at: n }).collect();
        let sent = Item::from_memory(&note, &p).unwrap();
        assert_eq!(sent.attachments.len(), 40);
        assert_eq!(sent.sources.len(), 5);
        assert_eq!(sent.text, "Plan\n\nfor the share");
        let mut back = sent.clone();
        back.attachments.iter_mut().for_each(|a| a.local = None);
        assert_eq!(sent.clone().sanitized().unwrap(), back);

        let mut long = Mission { id: 1, uid: "bbbbbbbb22222222".into(), title: format!("{}\n", "T".repeat(300)), notes: format!("{}\n\n\n\nend", "n ".repeat(6_000)), dir: None, status: Status::Idea, created: 1_700_000_000, session_id: None, from_agent: false, bay: None };
        let sent = MissionItem::from_mission(&long).unwrap();
        assert_eq!(sent.title.chars().count(), 200);
        assert!(sent.notes.chars().count() <= 8_000);
        assert_eq!(sent.clone().sanitized().unwrap(), sent);
        // What can't be shared at all isn't sent.
        long.notes = "the password: hunter2hunter2".into();
        assert!(MissionItem::from_mission(&long).is_none());
    }

    #[test]
    fn what_this_pc_cannot_share_is_withheld_not_missing() {
        let p = projects();
        let fine = memory("aaaaaaaa11111111", Kind::Fact, "Builds with just");
        // A project on a network folder is shared by its path (nothing reads that path), not withheld.
        let mut on_a_share = memory("bbbbbbbb22222222", Kind::Fact, "Deploys from the share");
        on_a_share.scope = Scope::Project("\\\\nas\\share\\app".into());
        let edited_to_look_secret = memory("cccccccc33333333", Kind::Fact, "token: ghp_abcdefghijklmnopqrstuvwxyz0123456789");
        let mission = |uid: &str, title: &str| Mission { id: 1, uid: uid.into(), title: title.into(), notes: String::new(), dir: None, status: Status::Idea, created: 1_700_000_000, session_id: None, from_agent: false, bay: None };
        let missions = [mission("dddddddd44444444", "Ship it"), mission("eeeeeeee55555555", "password=hunter2hunter2")];
        let memories = [fine, on_a_share, edited_to_look_secret];
        let (snapshot, withheld) = Snapshot::local_with_withheld(memories.iter(), &missions, Default::default(), &p);
        assert_eq!(snapshot.memories.keys().collect::<Vec<_>>(), ["aaaaaaaa11111111", "bbbbbbbb22222222"]);
        assert_eq!(snapshot.missions.keys().collect::<Vec<_>>(), ["dddddddd44444444"]);
        let mut want = HashSet::new();
        want.extend(["cccccccc33333333".to_owned(), "eeeeeeee55555555".to_owned()]);
        assert_eq!(withheld, want);
    }

    #[test]
    fn items_round_trip_through_json_without_this_pcs_paths() {
        let mut i = item("aaaaaaaa11111111", "note", "Sent the logo");
        i.attachments.push(ItemAttachment { kind: "file".into(), name: "logo.png".into(), target: "C:\\logo.png".into(), size: Some(3), file: Some("logo.png".into()), local: Some(PathBuf::from("C:\\Users\\me\\notes\\1\\logo.png")) });
        let text = i.to_json();
        assert!(!text.contains("Users"));
        let back: Item = serde_json::from_str(&text).unwrap();
        assert_eq!(back.attachments[0].local, None);
        assert_eq!(back.hash(), i.hash());
    }
}
