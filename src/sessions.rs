//! Sessions: every new session gets its own folder under the sessions root, named after its first
//! message, and Graysense keeps a small index of the sessions it knows (names, pins, folders).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::{config_dir, read_json, write_json};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    /// Name the user gave the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Short title taken from the first message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default)]
    pub created: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

#[derive(Default)]
pub struct SessionIndex {
    pub items: Vec<SessionMeta>,
    dirty: bool,
}

impl SessionIndex {
    fn path() -> PathBuf {
        config_dir().join("sessions.json")
    }

    pub fn load() -> Self {
        SessionIndex { items: read_json(&Self::path()).unwrap_or_default(), dirty: false }
    }

    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            write_json(&Self::path(), &self.items);
            self.dirty = false;
        }
    }

    pub fn get(&self, id: &str) -> Option<&SessionMeta> {
        self.items.iter().find(|m| m.id == id)
    }

    /// The entry for `id`, created if needed (marks the index for saving).
    pub fn entry(&mut self, id: &str) -> &mut SessionMeta {
        self.dirty = true;
        match self.items.iter().position(|m| m.id == id) {
            Some(i) => &mut self.items[i],
            None => {
                self.items.push(SessionMeta { id: id.to_owned(), created: chrono::Utc::now().timestamp(), ..Default::default() });
                self.items.last_mut().unwrap()
            }
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|m| m.id != id);
        self.dirty = true;
    }

    pub fn pinned(&self, id: &str) -> bool {
        self.get(id).is_some_and(|m| m.pinned)
    }
}

/// Words people open a request with that don't belong in a title.
const FILLER: &[&str] = &[
    "hey", "hi", "hello", "ok", "okay", "so", "please", "pls", "now", "can you", "could you", "would you", "will you", "can u", "i want you to",
    "i need you to", "i'd like you to", "i would like you to", "i want to", "i need to", "help me", "let's", "lets", "go ahead and", "quickly",
];

/// Words a cut-off title shouldn't end with.
const DANGLING: &[&str] = &["and", "or", "but", "the", "a", "an", "to", "of", "for", "with", "in", "on", "at", "by", "from", "into", "that", "then", "so", "my", "your", "its"];

/// A short title from a first message: "Hey, can you please fix the flaky login test in auth.rs?"
/// becomes "Fix the flaky login test in auth.rs".
pub fn short_title(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut s = first_sentence(line).trim();
    loop {
        let lower = s.to_lowercase();
        let Some(f) = FILLER.iter().find(|f| lower.starts_with(*f) && lower[f.len()..].starts_with(|c: char| !c.is_alphanumeric())) else { break };
        s = s[f.len()..].trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '!' | ':' | '-'));
    }
    let mut out = String::new();
    // End at a natural break ("… and …", "…, then …", "… (…") once there's enough to go on.
    let s = [" and ", " then ", ", ", "; ", " (", " - ", " — ", " so that ", " because "]
        .iter()
        .filter_map(|b| s.find(b))
        .filter(|&at| s[..at].split_whitespace().count() >= 3)
        .min()
        .map_or(s, |at| &s[..at]);
    for (i, word) in s.split_whitespace().enumerate() {
        if i == 8 || out.len() + word.len() > 52 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    // Don't end on a dangling joiner ("Switch to the light theme and").
    while let Some((head, last)) = out.rsplit_once(' ') {
        if !DANGLING.contains(&last.to_lowercase().trim_end_matches(|c: char| !c.is_alphanumeric())) {
            break;
        }
        out = head.to_owned();
    }
    let out = out.trim_end_matches(|c: char| !c.is_alphanumeric() && !matches!(c, ')' | '"' | '\'' | '`'));
    let mut c = out.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "New session".to_owned(),
    }
}

/// Up to the end of the first sentence ("e.g." and file names like "auth.rs" don't end it).
fn first_sentence(s: &str) -> &str {
    for (i, ch) in s.char_indices() {
        if matches!(ch, '.' | '?' | '!') {
            let rest = &s[i + ch.len_utf8()..];
            let word = s[..i].rsplit(char::is_whitespace).next().unwrap_or("");
            if (rest.is_empty() || rest.starts_with(' ')) && !matches!(word.to_ascii_lowercase().as_str(), "e.g" | "i.e" | "etc" | "vs") {
                return &s[..i];
            }
        }
    }
    s
}

/// A folder name Windows accepts: no reserved characters, no trailing dots or spaces, ≤ 60 chars.
fn folder_safe(title: &str) -> String {
    let cleaned: String = title.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c }).collect();
    let mut s = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 60 {
        s = s.chars().take(60).collect();
    }
    s.trim_end_matches(['.', ' ']).to_owned()
}

/// Create `<root>\<yyyy-mm-dd> <title>` (with " (2)", " (3)"… when taken) and return it.
pub fn create_folder(root: &Path, title: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let name = folder_safe(title);
    let base = format!("{} {}", chrono::Local::now().format("%Y-%m-%d"), if name.is_empty() { "Session" } else { &name });
    for n in 1..1000 {
        let dir = root.join(if n == 1 { base.clone() } else { format!("{base} ({n})") });
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many sessions with the same name today"))
}

/// Whether `path` is `root` or inside it (case-insensitive, as on Windows).
pub fn is_under(path: &Path, root: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let (p, r) = (norm(path), norm(root));
    !r.is_empty() && (p == r || p.starts_with(&format!("{r}\\")))
}

/// How a working folder reads in the UI: "Sessions › 2026-11-12 Fix login" for session folders,
/// otherwise the path with the home folder shortened to "~".
pub fn display_path(path: &Path, root: &Path) -> String {
    if is_under(path, root) {
        let rel = path.to_string_lossy().chars().skip(root.to_string_lossy().trim_end_matches(['\\', '/']).chars().count()).collect::<String>();
        let rel = rel.trim_start_matches(['\\', '/']);
        return if rel.is_empty() { "Sessions".to_owned() } else { format!("Sessions › {}", rel.replace(['\\', '/'], " › ")) };
    }
    crate::settings::short_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_short_and_skip_filler() {
        assert_eq!(short_title("Hey, can you please fix the flaky login test in auth.rs? It fails on CI."), "Fix the flaky login test in auth.rs");
        assert_eq!(short_title("count the lines of Rust in each file under src/ (read-only) and show the 10 biggest"), "Count the lines of Rust in each file");
        assert_eq!(short_title("\n\n  "), "New session");
        assert_eq!(short_title("Summarize e.g. the README"), "Summarize e.g. the README");
        assert_eq!(short_title("Please switch Graysense to the light theme and make the font 15"), "Switch Graysense to the light theme");
    }

    #[test]
    fn folders_are_safe_and_unique() {
        let root = std::env::temp_dir().join(format!("gs-sessions-{}", std::process::id()));
        let a = create_folder(&root, "Fix: login/auth? bug...").unwrap();
        let b = create_folder(&root, "Fix: login/auth? bug...").unwrap();
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(" Fix login auth bug"), "{name}");
        assert!(b.file_name().unwrap().to_string_lossy().ends_with("(2)"));
        assert!(is_under(&a, &root) && !is_under(&root.with_extension("x"), &root));
        assert_eq!(display_path(&a.join("src"), &root), format!("Sessions › {name} › src"));
        assert_eq!(display_path(&root, &root), "Sessions");
        let _ = std::fs::remove_dir_all(&root);
    }
}
