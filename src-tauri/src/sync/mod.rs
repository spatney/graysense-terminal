//! Sync to GitHub: memories, notes, missions and a few preferences kept in a private repository,
//! so they survive a reinstall and follow the user between PCs.
//!
//! How it works:
//! - The repository holds one JSON file per memory, note and mission (`memories/<uid>.json`, …),
//!   the files attached to notes, the preferences that make sense on any PC, and a generated
//!   `README.md` / `MEMORY.md` for people to read. Items are known by a uid that is the same on every
//!   PC. Approval rules (they let commands run without asking) and everything about one PC (folders,
//!   shell, window) never leave it.
//! - Each PC remembers what the repository held at its last sync: one hash per item. A sync is a
//!   three-way merge of that base, this PC and the repository, item by item. What only one side
//!   changed is taken, what both changed is settled by rules (what the user wrote wins, then the
//!   newer), and what the user deleted stays deleted. Git is only the transport: Graysense never
//!   asks it to merge, it fetches, merges itself, and pushes a commit on top.
//! - Memories that arrive from another PC and say the same thing in other words are combined
//!   by comparing their text; the AI then looks at what's new on both sides (`reconcile`), through
//!   the same update/delete rules learning uses, so anything the user wrote waits for review.
//! - Nothing is trusted because it came from the repository: every item is cleaned, secrets are
//!   refused and file names are checked before anything touches the disk.

pub mod apply;
pub mod git;
pub mod items;
pub mod merge;
pub mod projects;
pub mod reconcile;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::settings::{config_dir, read_json, write_json};
use items::Base;

/// The layout of the repository. A newer Graysense refuses to touch one it doesn't know.
pub const FORMAT: u32 = 1;
/// Marks a repository as Graysense's (and says which layout it uses).
pub const MANIFEST: &str = "graysense-sync.json";
pub const MEMORIES_DIR: &str = "memories";
pub const MISSIONS_DIR: &str = "missions";
pub const NOTES_DIR: &str = "notes";
pub const SETTINGS_FILE: &str = "settings.json";

/// A note's file bigger than this stays on the PC it was saved on (GitHub refuses 100 MB).
pub const MAX_FILE: u64 = 25 * 1024 * 1024;
/// A sync stops adding note files after this many bytes (the rest go next time).
pub const MAX_UPLOAD: u64 = 200 * 1024 * 1024;
/// A sync copies at most this many bytes of other PCs' note files into this one (the rest come
/// next time): copying happens while Graysense waits.
pub const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;

/// What the user chose: which repository, and what goes in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `owner/name` on github.com.
    pub repo: Option<String>,
    pub branch: Option<String>,
    /// Who Graysense signs in as ("GitHub CLI (octocat)"), for display.
    pub account: Option<String>,
    /// The user was told the repository is public and went ahead.
    pub allow_public: bool,
    /// Sync in the background (after changes, when Graysense opens, and now and then).
    pub auto: bool,
    /// Keep the files attached to notes in the repository too.
    pub files: bool,
    pub settings: bool,
    pub missions: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { repo: None, branch: None, account: None, allow_public: false, auto: true, files: true, settings: true, missions: true }
    }
}

/// What a sync includes (from [`Config`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub files: bool,
    pub settings: bool,
    pub missions: bool,
}

impl Config {
    pub fn options(&self) -> Options {
        Options { files: self.files, settings: self.settings, missions: self.missions }
    }

    pub fn connected(&self) -> bool {
        self.repo.is_some()
    }
}

/// What the last sync did, for the card in Settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub at: i64,
    /// Memories, notes and missions added or changed here.
    pub pulled: usize,
    /// … removed here, because they were removed elsewhere or combined.
    pub removed: usize,
    /// Files added, changed or removed in the repository.
    pub pushed: usize,
    /// Duplicates found by comparing text.
    pub combined: usize,
    /// Things both PCs changed that rules settled.
    pub conflicts: usize,
    /// Memories the AI combined or retired.
    pub tidied: usize,
    /// Changes the AI left for the user's review.
    pub review: usize,
    /// Anything the user should know (files left out, items skipped, an AI step that didn't run).
    pub notes: Vec<String>,
}

/// What this PC knows about the repository between syncs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The repository commit this PC last synced with.
    pub commit: Option<String>,
    /// What the repository held then: a hash per item.
    pub base: Base,
    /// Memories (by uid) that are new on both PCs and still wait for the AI to look at them.
    pub reconcile: Vec<merge::Candidate>,
    pub last_sync: Option<i64>,
    pub last_error: Option<String>,
    pub report: Option<Report>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct File {
    version: u32,
    config: Config,
    state: State,
}

/// The sync folder: `sync.json`, the clone of the repository, and backups (made when needed).
pub fn dir() -> PathBuf {
    config_dir().join("sync")
}

/// Where the repository is cloned.
pub fn repo_dir() -> PathBuf {
    dir().join("repo")
}

fn file_path() -> PathBuf {
    dir().join("sync.json")
}

pub fn load() -> (Config, State) {
    let file: File = read_json(&file_path()).unwrap_or_default();
    (file.config, file.state)
}

pub fn save(config: &Config, state: &State) -> bool {
    write_json(&file_path(), &File { version: 1, config: config.clone(), state: state.clone() }).is_ok()
}

/// What this PC is called in commits and in what the AI reads ("DESKTOP-4F2K").
pub fn device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"].iter().filter_map(|k| std::env::var(k).ok()).map(|v| v.trim().to_owned()).find(|v| !v.is_empty()).unwrap_or_else(|| "this PC".to_owned())
}

/// Whether `path` is something on this PC. A network share or a device path (`\\server\share`,
/// `\\?\C:\…`, `//host/x`) isn't: Windows connects to the server, and signs in to it, just to look
/// at the path. A path that came out of a repository must never be touched if it leads off the PC.
pub fn is_local_path(path: &str) -> bool {
    let separator = |c: char| c == '/' || c == '\\';
    let mut chars = path.trim_start().chars();
    let double = chars.next().is_some_and(separator) && chars.next().is_some_and(separator);
    !double && !path.chars().any(char::is_control)
}

/// A stable 64-bit FNV-1a hash of `parts` as hex. Hashes are kept between runs and compared across
/// versions, so this can't be Rust's own hasher (which may change between compilers).
pub fn fnv(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.bytes().chain(std::iter::once(0x1f)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// A file name that's safe to create on Windows: no separators or reserved characters, not a
/// device name, no trailing dots or spaces. `None` when nothing usable is left.
pub fn safe_file_name(name: &str) -> Option<String> {
    let cleaned: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_owned();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()) || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    let named = if reserved { format!("_{cleaned}") } else { cleaned };
    Some(named.chars().take(120).collect())
}

/// How many of each thing a snapshot holds, for the "merge?" question.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub memories: usize,
    pub notes: usize,
    pub missions: usize,
}

/// A setting's value as a hash (settings are compared whole, key by key).
pub fn hash_value(v: &serde_json::Value) -> String {
    fnv(&[&v.to_string()])
}

/// `(key, hash)` for each setting in `values`.
pub fn hash_settings(values: &BTreeMap<String, serde_json::Value>) -> BTreeMap<String, String> {
    values.iter().map(|(k, v)| (k.clone(), hash_value(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_depend_on_every_part() {
        // Persisted between runs: these values must never change.
        assert_eq!(fnv(&["a", "b"]), "e8bcb18230513c4a");
        assert_eq!(fnv(&["ab", ""]), "fd86c483ef3f2b70");
        assert_eq!(fnv(&["x"]), "08f0e907b58d3dc8");
    }

    #[test]
    fn file_names_are_made_safe_for_windows() {
        assert_eq!(safe_file_name("logo.png").as_deref(), Some("logo.png"));
        assert_eq!(safe_file_name("a/b\\c:d.txt").as_deref(), Some("a_b_c_d.txt"));
        assert_eq!(safe_file_name("..").as_deref(), None);
        assert_eq!(safe_file_name("  . ").as_deref(), None);
        assert_eq!(safe_file_name("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(safe_file_name("LPT1").as_deref(), Some("_LPT1"));
        assert_eq!(safe_file_name("notes.").as_deref(), Some("notes"));
        assert_eq!(safe_file_name(&"x".repeat(300)).unwrap().len(), 120);
    }

    #[test]
    fn paths_that_lead_off_this_pc_are_not_local() {
        for local in ["C:\\Users\\me\\notes.txt", "c:/work/app", "d:\\", "/home/me/app", "relative\\dir", "notes.txt", "\\single"] {
            assert!(is_local_path(local), "{local}");
        }
        for remote in ["\\\\evil.example.com\\share\\a.pdf", "//evil.example.com/share", "\\/evil\\share", "/\\evil\\share", "\\\\?\\C:\\Users", "\\\\.\\pipe\\x", "  \\\\server\\share", "C:\\ok\u{0}\\x"] {
            assert!(!is_local_path(remote), "{remote}");
        }
    }

    #[test]
    fn config_from_an_older_file_keeps_its_defaults() {
        let file: File = serde_json::from_str(r#"{"version":1,"config":{"repo":"me/mem"}}"#).unwrap();
        assert_eq!(file.config.repo.as_deref(), Some("me/mem"));
        assert!(file.config.auto && file.config.files && file.config.settings && file.config.missions);
        assert!(file.state.base.memories.is_empty());
    }
}
