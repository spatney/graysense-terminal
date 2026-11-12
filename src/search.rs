//! Ctrl+K search over session assets: session titles and conversation transcripts, plus the
//! names and text of the files in the session folders.
//!
//! The index is a flat list of documents matched by brute-force substring search over
//! ASCII-lowercased copies of their text, which is fast enough for tens of MB per keystroke.
//! Transcripts come from the Copilot runtime's `events.jsonl` logs, read with bounded IO and
//! cached on disk (keyed by log length and mtime) so unchanged logs are not read again.

use std::cmp::Reverse;
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// At most this much of an `events.jsonl` is read.
const MAX_LOG_READ: u64 = 48 << 20;
/// Cap on the transcript text of one session.
const MAX_TRANSCRIPT: usize = 256 << 10;
/// Only lines whose first bytes name one of these event types are parsed.
const MESSAGE_TYPES: [&[u8]; 2] = [
    br#""type":"user.message""#,
    br#""type":"assistant.message""#,
];
const LINE_HEAD: u64 = 64;
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 20_000;
/// Larger files are indexed by name only.
const MAX_FILE_SIZE: u64 = 1 << 20;
const MAX_FILE_TEXT: usize = 512 << 10;
/// Text indexed from all files together (bounds memory and search time); files beyond it are
/// indexed by name only.
const MAX_FILES_TEXT: usize = 32 << 20;
const SKIP_DIRS: &str =
    ".git node_modules target dist build out .venv venv __pycache__ .next .cache .idea .vs bin obj";
const BINARY_EXTS: &str = "png jpg jpeg gif webp bmp ico svgz pdf zip gz 7z rar tar exe dll so \
    dylib bin class jar woff woff2 ttf otf eot mp3 mp4 mov avi wav flac ogg webm db sqlite sqlite3 \
    pyc lock wasm psd ai docx xlsx pptx";
/// Body occurrences of a term beyond this don't raise the score.
const MAX_COUNT: usize = 20;
/// Snippet geometry in bytes: context before the match, whole window, and the plain lead-in.
const SNIPPET_BEFORE: usize = 60;
const SNIPPET_LEN: usize = 170;
const LEAD_LEN: usize = 140;

/// A session to index, supplied by the app.
#[derive(Clone, Debug)]
pub struct SessionSource {
    pub id: String,
    pub title: String,
    /// The session's working folder, if known.
    pub folder: Option<PathBuf>,
    /// Unix seconds of the last activity.
    pub modified: i64,
    /// Index the conversation text (true for Graysense's own sessions).
    pub transcript: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocKind {
    Session(String),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub kind: DocKind,
    /// Session title, or file name.
    pub title: String,
    /// Files: parent path relative to the sessions root, using '\' (e.g. "2026-11-12 Fix login
    /// bug\src"), "" for files directly in the root.
    /// Sessions: the folder's display path with the home dir shortened to "~" (e.g.
    /// "~\Graysense\Sessions\2026-11-12 Fix login bug"), or "" if unknown.
    pub location: String,
    /// Unix seconds.
    pub modified: i64,
    body: String,
    /// ASCII-lowercased copies (same byte offsets as the originals) used for matching.
    body_lc: String,
    title_lc: String,
}

#[derive(Clone, Debug, Default)]
pub struct IndexStats {
    /// Session docs.
    pub sessions: usize,
    /// Session docs with transcript text.
    pub transcripts: usize,
    /// File docs.
    pub files: usize,
    /// Indexed body text.
    pub bytes: usize,
    /// Build time.
    pub millis: u128,
}

#[derive(Debug)]
pub struct Index {
    pub docs: Vec<Doc>,
    pub stats: IndexStats,
}

#[derive(Clone, Debug)]
pub struct Hit {
    /// Index into `Index::docs`.
    pub doc: usize,
    #[cfg_attr(not(test), allow(dead_code))]
    pub score: f32,
    /// One-line excerpt around the best match; "…" at either end when cut. Empty when there is
    /// nothing to show.
    pub snippet: String,
    /// Byte ranges of matched terms inside `snippet` (sorted, non-overlapping).
    pub snippet_ranges: Vec<Range<usize>>,
    /// Byte ranges of matched terms inside the doc's `title` (sorted, non-overlapping).
    pub title_ranges: Vec<Range<usize>>,
    /// Files: 1-based line of the snippet's match.
    pub line: Option<usize>,
}

impl Index {
    /// An index without docs, e.g. until the first build finishes.
    pub fn empty() -> Index {
        Index {
            docs: Vec::new(),
            stats: IndexStats::default(),
        }
    }

    /// Build an index over `sessions` (title + optional transcript read from
    /// `state_dir/<id>/events.jsonl`) and all files under `sessions_root`. `cache_dir` holds
    /// `transcripts.json` (created if needed).
    pub fn build(
        sessions_root: &Path,
        sessions: &[SessionSource],
        state_dir: &Path,
        cache_dir: &Path,
    ) -> Index {
        let started = Instant::now();
        let home = dirs::home_dir();
        let cache_path = cache_dir.join("transcripts.json");
        let mut cache = TranscriptCache::load(&cache_path);
        let mut sessions: Vec<&SessionSource> = sessions.iter().collect();
        sessions.sort_by_key(|s| Reverse(s.modified));
        let mut docs: Vec<Doc> = sessions
            .iter()
            .map(|s| {
                let body = if s.transcript {
                    cache.transcript(&s.id, &state_dir.join(&s.id).join("events.jsonl"))
                } else {
                    String::new()
                };
                let location = s
                    .folder
                    .as_deref()
                    .map(|f| display_path(f, home.as_deref()))
                    .unwrap_or_default();
                Doc::new(
                    DocKind::Session(s.id.clone()),
                    s.title.clone(),
                    location,
                    s.modified,
                    body,
                )
            })
            .collect();
        cache.save(&cache_path);
        let transcripts = docs.iter().filter(|d| !d.body.is_empty()).count();
        docs.extend(file_docs(sessions_root, MAX_FILES_TEXT));
        let stats = IndexStats {
            sessions: sessions.len(),
            transcripts,
            files: docs.len() - sessions.len(),
            bytes: docs.iter().map(|d| d.body.len()).sum(),
            millis: started.elapsed().as_millis(),
        };
        Index { docs, stats }
    }

    /// Ranked results for `query` (empty/whitespace query => same as `recent`).
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let query = Query::parse(query);
        if query.terms.is_empty() {
            return self.recent(limit);
        }
        let now = unix_now();
        let mut ranked: Vec<(usize, f32)> = self
            .docs
            .iter()
            .enumerate()
            .filter_map(|(i, doc)| Some((i, doc.score(&query, now)?)))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| self.docs[b.0].modified.cmp(&self.docs[a.0].modified))
        });
        ranked
            .into_iter()
            .take(limit)
            .map(|(i, score)| self.hit(i, score, &query))
            .collect()
    }

    /// Most recently modified docs (snippet = start of the body), newest first.
    pub fn recent(&self, limit: usize) -> Vec<Hit> {
        let mut order: Vec<usize> = (0..self.docs.len()).collect();
        order.sort_by_key(|&i| Reverse(self.docs[i].modified));
        order
            .into_iter()
            .take(limit)
            .map(|doc| Hit {
                doc,
                score: 0.0,
                snippet: lead(&self.docs[doc].body),
                snippet_ranges: Vec::new(),
                title_ranges: Vec::new(),
                line: None,
            })
            .collect()
    }

    /// The doc a hit refers to.
    pub fn doc(&self, hit: &Hit) -> &Doc {
        &self.docs[hit.doc]
    }

    fn hit(&self, doc: usize, score: f32, query: &Query) -> Hit {
        let d = &self.docs[doc];
        let first =
            |t: &String| find_at(d.body_lc.as_bytes(), t.as_bytes(), 0).map(|i| (i, t.len()));
        // Center on the whole query if the body has it, else on the earliest term.
        let found = query
            .phrase
            .as_ref()
            .and_then(first)
            .or_else(|| query.terms.iter().filter_map(first).min());
        let snippet = match found {
            Some((at, len)) => excerpt_around(&d.body, at, len),
            None => lead(&d.body),
        };
        let line = found
            .filter(|_| matches!(d.kind, DocKind::File(_)))
            .map(|(at, _)| 1 + d.body[..at].matches('\n').count());
        Hit {
            doc,
            score,
            snippet_ranges: term_ranges(&snippet.to_ascii_lowercase(), &query.terms),
            snippet,
            title_ranges: term_ranges(&d.title_lc, &query.terms),
            line,
        }
    }
}

impl Doc {
    fn new(kind: DocKind, title: String, location: String, modified: i64, body: String) -> Doc {
        Doc {
            kind,
            title_lc: title.to_ascii_lowercase(),
            title,
            location,
            modified,
            body_lc: body.to_ascii_lowercase(),
            body,
        }
    }

    /// The full indexed text (transcript or file contents); "" for binaries/title-only docs.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// `None` unless every term occurs in the title or body; otherwise
    ///   Σ per term: 8 if in the title (+4 if at a word start there)
    ///             + 1 + ln(1 + min(body occurrences, 20)) if in the body
    ///   + 10 if the whole query (2+ terms) is in the title, + 4 if it is in the body
    ///   + 2 · 0.5^(age in days / 30)   (recency, 30-day half-life)
    ///   + 0.5 for sessions.
    fn score(&self, query: &Query, now: i64) -> Option<f32> {
        let (title, body) = (self.title_lc.as_bytes(), self.body_lc.as_bytes());
        let mut score = 0.0;
        for term in &query.terms {
            let in_title = find_at(title, term.as_bytes(), 0).is_some();
            let hits = occurrences(body, term.as_bytes()).take(MAX_COUNT).count();
            if !in_title && hits == 0 {
                return None;
            }
            if in_title {
                score += 8.0;
                if at_word_start(&self.title_lc, term) {
                    score += 4.0;
                }
            }
            if hits > 0 {
                score += 1.0 + (1.0 + hits as f32).ln();
            }
        }
        if let Some(phrase) = &query.phrase {
            if find_at(title, phrase.as_bytes(), 0).is_some() {
                score += 10.0;
            }
            if find_at(body, phrase.as_bytes(), 0).is_some() {
                score += 4.0;
            }
        }
        let age_days = (now - self.modified).max(0) as f32 / 86_400.0;
        score += 2.0 * 0.5f32.powf(age_days / 30.0);
        if matches!(self.kind, DocKind::Session(_)) {
            score += 0.5;
        }
        Some(score)
    }
}

struct Query {
    /// Distinct ASCII-lowercased terms, longest (usually the most selective) first.
    terms: Vec<String>,
    /// The whole query, when it has 2+ terms.
    phrase: Option<String>,
}

impl Query {
    /// Whitespace-separated terms; a "double-quoted phrase" is one term.
    fn parse(query: &str) -> Query {
        let mut terms = Vec::new();
        for (i, part) in query.split('"').enumerate() {
            if i % 2 == 1 {
                terms.push(part.trim().to_ascii_lowercase());
            } else {
                terms.extend(part.split_whitespace().map(str::to_ascii_lowercase));
            }
        }
        terms.retain(|t| !t.is_empty());
        let phrase = (terms.len() > 1).then(|| terms.join(" "));
        terms.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        terms.dedup();
        Query { terms, phrase }
    }
}

/// Offset of the first `needle` in `hay` at or after `from`.
///
/// Compares the needle's first and last bytes at 32 positions per step, which the compiler
/// vectorizes, and verifies the candidates: several times faster than `str::find` on long text.
fn find_at(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    const LANES: usize = 32;
    let (&first, &last) = (needle.first()?, needle.last()?);
    let tail = needle.len() - 1;
    let is_match = |i: usize| hay.get(i..i + needle.len()) == Some(needle);
    let mut i = from;
    while let (Some(a), Some(b)) = (
        hay.get(i..).and_then(<[u8]>::first_chunk::<LANES>),
        hay.get(i + tail..).and_then(<[u8]>::first_chunk::<LANES>),
    ) {
        let mut mask = a.iter().zip(b).enumerate().fold(0u32, |m, (k, (&x, &y))| {
            m | (u32::from((x == first) & (y == last)) << k)
        });
        while mask != 0 {
            let j = i + mask.trailing_zeros() as usize;
            if is_match(j) {
                return Some(j);
            }
            mask &= mask - 1;
        }
        i += LANES;
    }
    (i..=hay.len().checked_sub(needle.len())?).find(|&j| is_match(j))
}

/// Offsets of the non-overlapping occurrences of `needle` in `hay`.
fn occurrences(hay: &[u8], needle: &[u8]) -> impl Iterator<Item = usize> {
    let mut from = 0;
    std::iter::from_fn(move || {
        let at = find_at(hay, needle, from)?;
        from = at + needle.len();
        Some(at)
    })
}

/// Merged byte ranges of the occurrences of `terms` in the lowercased `text`.
fn term_ranges(text: &str, terms: &[String]) -> Vec<Range<usize>> {
    let mut found: Vec<Range<usize>> = terms
        .iter()
        .flat_map(|t| occurrences(text.as_bytes(), t.as_bytes()).map(move |i| i..i + t.len()))
        .collect();
    found.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(found.len());
    for r in found {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// Whether `needle` occurs in `text` at the start of a word.
fn at_word_start(text: &str, needle: &str) -> bool {
    occurrences(text.as_bytes(), needle.as_bytes()).any(|i| {
        text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

/// One-line excerpt of `body` around the match at `at..at + len`.
fn excerpt_around(body: &str, at: usize, len: usize) -> String {
    let bytes = body.as_bytes();
    let mut start = body.floor_char_boundary(at.saturating_sub(SNIPPET_BEFORE));
    // Begin at a word when one starts within 15 bytes (but never past the match).
    if start > 0
        && !bytes[start - 1].is_ascii_whitespace()
        && let Some(k) = bytes[start..at.min(start + 15)]
            .iter()
            .position(u8::is_ascii_whitespace)
    {
        start += k + 1;
    }
    let end = body.floor_char_boundary((start + SNIPPET_LEN).max(at + len));
    excerpt(body, start, end)
}

/// The start of `body` as a one-line excerpt.
fn lead(body: &str) -> String {
    let start = body.len() - body.trim_start().len();
    excerpt(body, start, body.floor_char_boundary(start + LEAD_LEN))
}

/// `body[start..end]` with whitespace runs collapsed to single spaces and "…" where text was cut.
fn excerpt(body: &str, start: usize, end: usize) -> String {
    let words: Vec<&str> = body[start..end]
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(end - start + 6);
    if !body[..start].trim_end().is_empty() {
        out.push('…');
    }
    out.push_str(&words.join(" "));
    if !body[end..].trim_start().is_empty() {
        out.push('…');
    }
    out
}

/// Cuts `text` to at most `max` bytes, at a char boundary.
fn truncate(text: &mut String, max: usize) {
    text.truncate(text.floor_char_boundary(max));
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    len: u64,
    mtime: i64,
    text: String,
}

/// Transcripts by session id, persisted between builds in `transcripts.json`.
struct TranscriptCache {
    /// Entries from the previous build not used (yet) by this one.
    old: BTreeMap<String, CacheEntry>,
    new: BTreeMap<String, CacheEntry>,
    changed: bool,
}

impl TranscriptCache {
    fn load(path: &Path) -> TranscriptCache {
        let old = fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        TranscriptCache {
            old,
            new: BTreeMap::new(),
            changed: false,
        }
    }

    /// The transcript in `log`, reusing the cached text while the log's length and mtime match.
    fn transcript(&mut self, id: &str, log: &Path) -> String {
        let Ok(meta) = fs::metadata(log) else {
            return String::new();
        };
        let (len, mtime) = (meta.len(), modified_secs(&meta));
        let entry = match self.old.remove(id) {
            Some(entry) if entry.len == len && entry.mtime == mtime => entry,
            _ => {
                self.changed = true;
                let Some(text) = read_transcript(log) else {
                    return String::new();
                };
                CacheEntry { len, mtime, text }
            }
        };
        let text = entry.text.clone();
        self.new.insert(id.to_owned(), entry);
        text
    }

    /// Rewrites the cache with this build's entries only, if anything changed.
    fn save(self, path: &Path) {
        // Leftover old entries belong to sessions no longer indexed.
        if !self.changed && self.old.is_empty() {
            return;
        }
        if let Ok(data) = serde_json::to_vec(&self.new) {
            let _ = write_replace(path, &data);
        }
    }
}

/// Writes `path` through a temp file and a rename, so readers never see a partial file.
fn write_replace(path: &Path, data: &[u8]) -> std::io::Result<()> {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{seq}.tmp", std::process::id()));
    fs::write(&tmp, data)
        .and_then(|()| fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
}

/// "You: …\n\nCopilot: …" from the user and assistant messages of an `events.jsonl` log, or
/// `None` if it can't be opened.
fn read_transcript(log: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Event {
        #[serde(rename = "type")]
        kind: String,
        data: Data,
    }
    #[derive(Deserialize)]
    struct Data {
        // Never `transformedContent`: that carries injected system text.
        content: Option<String>,
    }

    let file = File::open(log).ok()?;
    let mut reader = BufReader::with_capacity(64 << 10, file.take(MAX_LOG_READ));
    let mut text = String::new();
    let mut line = Vec::new();
    while text.len() < MAX_TRANSCRIPT {
        line.clear();
        // Read the head first, so huge lines of other events (tool output) are skipped unbuffered.
        match (&mut reader).take(LINE_HEAD).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let is_message = MESSAGE_TYPES.iter().any(|t| find_at(&line, t, 0).is_some());
        if line.last() != Some(&b'\n') {
            let rest = if is_message {
                reader.read_until(b'\n', &mut line)
            } else {
                reader.skip_until(b'\n')
            };
            if rest.is_err() {
                break;
            }
        }
        if !is_message {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(&String::from_utf8_lossy(&line)) else {
            continue;
        };
        let who = match event.kind.as_str() {
            "user.message" => "You",
            "assistant.message" => "Copilot",
            _ => continue,
        };
        let Some(content) = event
            .data
            .content
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        else {
            continue;
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(who);
        text.push_str(": ");
        text.push_str(content);
    }
    truncate(&mut text, MAX_TRANSCRIPT);
    Some(text)
}

/// Docs for the files under `root`, breadth-first (newest first within a folder), at most
/// `MAX_DEPTH` deep and `MAX_FILES` in total, skipping dependency/build folders and symlinked
/// dirs. Bodies take at most `text_budget` bytes together.
fn file_docs(root: &Path, mut text_budget: usize) -> Vec<Doc> {
    let mut docs = Vec::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), String::new(), 0)]);
    while let Some((dir, location, depth)) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        // Newest first, so recent sessions win when the caps are reached.
        let mut entries: Vec<fs::DirEntry> = entries.flatten().collect();
        entries.sort_by_cached_key(|e| Reverse(e.metadata().map_or(0, |m| modified_secs(&m))));
        for entry in entries {
            if docs.len() == MAX_FILES {
                return docs;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if kind.is_dir() {
                if depth + 1 < MAX_DEPTH
                    && !SKIP_DIRS.split(' ').any(|d| d.eq_ignore_ascii_case(&name))
                {
                    let sub = if location.is_empty() {
                        name
                    } else {
                        format!("{location}\\{name}")
                    };
                    queue.push_back((path, sub, depth + 1));
                }
                continue;
            }
            // `is_dir` is false for symlinked dirs (and junctions), so they are never entered;
            // symlinked files are followed.
            let meta = if kind.is_symlink() {
                fs::metadata(&path)
            } else {
                entry.metadata()
            };
            let Some(meta) = meta.ok().filter(fs::Metadata::is_file) else {
                continue;
            };
            let body = if meta.len() <= text_budget as u64 {
                file_text(&path, meta.len())
            } else {
                String::new()
            };
            text_budget = text_budget.saturating_sub(body.len());
            docs.push(Doc::new(
                DocKind::File(path),
                name,
                location.clone(),
                modified_secs(&meta),
                body,
            ));
        }
    }
    docs
}

/// The text of a file to index; "" for big or binary files.
fn file_text(path: &Path, size: u64) -> String {
    let binary = path
        .extension()
        .is_some_and(|e| BINARY_EXTS.split(' ').any(|b| e.eq_ignore_ascii_case(b)));
    if binary || size > MAX_FILE_SIZE {
        return String::new();
    }
    let mut data = Vec::new();
    let read = File::open(path).and_then(|f| f.take(MAX_FILE_SIZE).read_to_end(&mut data));
    if read.is_err() {
        return String::new();
    }
    let mut text = match data.as_slice() {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        bytes if bytes[..bytes.len().min(8 << 10)].contains(&0) => return String::new(),
        bytes => String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
            .into_owned(),
    };
    truncate(&mut text, MAX_FILE_TEXT);
    text
}

/// Decodes UTF-16 text (after its byte-order mark), replacing invalid sequences.
fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|c| unit([c[0], c[1]]));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// `path` with '\' separators and the home dir shortened to "~".
fn display_path(path: &Path, home: Option<&Path>) -> String {
    let path = path.to_string_lossy().replace('/', "\\");
    if let Some(home) = home {
        let home = home.to_string_lossy().replace('/', "\\");
        let home = home.trim_end_matches('\\');
        if let Some(rest) = path.get(home.len()..)
            && !home.is_empty()
            && path[..home.len()].eq_ignore_ascii_case(home)
            && (rest.is_empty() || rest.starts_with('\\'))
        {
            return format!("~{rest}");
        }
    }
    path
}

fn modified_secs(meta: &fs::Metadata) -> i64 {
    meta.modified().map_or(0, unix_secs)
}

fn unix_now() -> i64 {
    unix_secs(SystemTime::now())
}

fn unix_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir()
                .join(format!("graysense-search-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, data: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    /// One `events.jsonl` line, with "type" first as the runtime writes it.
    fn event(kind: &str, data: Value) -> String {
        format!("{{\"type\":\"{kind}\",\"data\":{data},\"id\":\"e\"}}\n")
    }

    fn index(docs: Vec<Doc>) -> Index {
        Index {
            docs,
            stats: IndexStats::default(),
        }
    }

    fn file(name: &str, body: &str, modified: i64) -> Doc {
        Doc::new(
            DocKind::File(name.into()),
            name.into(),
            String::new(),
            modified,
            body.into(),
        )
    }

    fn session(title: &str, body: &str, modified: i64) -> Doc {
        Doc::new(
            DocKind::Session(title.into()),
            title.into(),
            String::new(),
            modified,
            body.into(),
        )
    }

    fn titles(index: &Index, hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| index.doc(h).title.clone()).collect()
    }

    /// The parts of `text` covered by `ranges`.
    fn marked<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|r| &text[r.clone()]).collect()
    }

    #[test]
    fn transcript_keeps_only_message_content() {
        let tmp = TempDir::new("transcript");
        let log = tmp.0.join("events.jsonl");
        let mut data = [
            event("session.start", json!({"content": "START"})),
            event(
                "user.message",
                json!({
                    "content": "Respond to Arun's email",
                    "transformedContent": "<current_datetime>INJECTED</current_datetime>",
                }),
            ),
            event(
                "assistant.message",
                json!({"content": "", "toolRequests": [{"name": "view"}]}),
            ),
            event(
                "tool.execution_complete",
                json!({"content": "TOOL ".repeat(100_000)}),
            ),
            event("assistant.message_delta", json!({"content": "DELTA"})),
            "{\"type\":\"assistant.message\",\"data\":{\"content\":\"TRUNCATED\n".into(),
            event(
                "assistant.message",
                json!({"content": "Here is the draft.\nThanks"}),
            ),
        ]
        .concat()
        .into_bytes();
        data.extend_from_slice(b"{\"type\":\"user.message\",\"data\":{\"content\":\"caf\xE9 \"}}");
        write(&log, data);
        assert_eq!(
            read_transcript(&log).unwrap(),
            "You: Respond to Arun's email\n\nCopilot: Here is the draft.\nThanks\n\n\
             You: caf\u{FFFD}"
        );
        assert_eq!(read_transcript(&tmp.0.join("missing.jsonl")), None);
    }

    #[test]
    fn transcript_is_capped() {
        let tmp = TempDir::new("cap");
        let log = tmp.0.join("events.jsonl");
        let message = |i| {
            event(
                "user.message",
                json!({"content": format!("{i} {}", "é".repeat(500))}),
            )
        };
        write(&log, (0..400).map(message).collect::<String>());
        let text = read_transcript(&log).unwrap();
        assert!(
            text.len() <= MAX_TRANSCRIPT && text.len() > MAX_TRANSCRIPT - 4,
            "{}",
            text.len()
        );
    }

    #[test]
    fn every_term_must_match_and_title_matches_rank_first() {
        let now = unix_now();
        let idx = index(vec![
            file(
                "notes.txt",
                "login login login login login form validation",
                now,
            ),
            session("Fix login bug", "", now - 60 * 86_400),
            file("other.txt", "login only", now),
        ]);
        let hits = idx.search("login", 10);
        assert_eq!(
            titles(&idx, &hits),
            ["Fix login bug", "notes.txt", "other.txt"]
        );
        assert_eq!(hits[0].snippet, "");
        assert_eq!(marked("Fix login bug", &hits[0].title_ranges), ["login"]);
        assert_eq!(titles(&idx, &idx.search("LOGIN Form", 10)), ["notes.txt"]);
        assert!(idx.search("login missing", 10).is_empty());
    }

    #[test]
    fn quoted_phrase_is_one_term() {
        let idx = index(vec![
            file("a.txt", "the login page is down", 1),
            file("b.txt", "page: login", 2),
        ]);
        // Both have the terms; the one with the whole query ranks first although it is older.
        assert_eq!(
            titles(&idx, &idx.search("login page", 10)),
            ["a.txt", "b.txt"]
        );
        let hits = idx.search("\"Login Page\"", 10);
        assert_eq!(titles(&idx, &hits), ["a.txt"]);
        assert_eq!(hits[0].snippet, "the login page is down");
        assert_eq!(
            marked(&hits[0].snippet, &hits[0].snippet_ranges),
            ["login page"]
        );
    }

    #[test]
    fn snippet_is_a_window_around_the_match() {
        let filler = "naïve café — résumé\n\t ".repeat(30);
        let body = format!("{filler}Le NEEDLE est là, naïve café. {filler}");
        let idx = index(vec![file("long.md", &body, 1)]);
        let hit = &idx.search("needle \"EST là\"", 1)[0];
        let s = hit.snippet.as_str();
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.len() <= SNIPPET_LEN + 2 * '…'.len_utf8(), "{s}");
        assert!(
            s.contains("café — résumé naïve") && s.contains("Le NEEDLE est là, naïve café."),
            "{s}"
        );
        assert!(!s.contains(['\n', '\t']) && !s.contains("  "), "{s}");
        let first_word = s.trim_start_matches('…').split(' ').next().unwrap();
        assert!(
            ["naïve", "café", "—", "résumé"].contains(&first_word),
            "{s}"
        );
        let matched = marked(s, &hit.snippet_ranges);
        assert_eq!(matched, ["NEEDLE", "est là"]);
        assert_eq!(hit.line, Some(31));
    }

    #[test]
    fn build_indexes_file_names_and_text() {
        let tmp = TempDir::new("files");
        let root = tmp.0.join("Sessions");
        let folder = root.join("2026-11-12 Fix login bug");
        let main_rs = folder.join("src").join("main.rs");
        write(&main_rs, "fn main() {}\n// Handles the login flow\n");
        write(
            &folder.join("node_modules").join("pkg").join("index.js"),
            "skippedmodule",
        );
        write(&folder.join("blob.dat"), b"abc\0binarysecret");
        write(&folder.join("photo.PNG"), "pngsecret");
        let wide: Vec<u8> = [0xFEFF]
            .into_iter()
            .chain("widetext".encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        write(&folder.join("utf16.txt"), wide);
        write(&root.join("readme.md"), "\u{feff}rootnote");
        let idx = Index::build(&root, &[], &tmp.0.join("state"), &tmp.0.join("cache"));
        assert_eq!((idx.stats.sessions, idx.stats.files), (0, 5));
        for query in ["skippedmodule", "index.js", "binarysecret", "pngsecret"] {
            assert!(idx.search(query, 10).is_empty(), "{query}");
        }
        let hit = &idx.search("LOGIN flow", 10)[0];
        let doc = idx.doc(hit);
        assert_eq!(doc.kind, DocKind::File(main_rs));
        assert_eq!(
            (doc.title.as_str(), doc.location.as_str()),
            ("main.rs", "2026-11-12 Fix login bug\\src")
        );
        assert_eq!(hit.line, Some(2));
        let hit = &idx.search("blob", 10)[0];
        assert_eq!((idx.doc(hit).body(), hit.snippet.as_str()), ("", ""));
        assert_eq!(marked("blob.dat", &hit.title_ranges), ["blob"]);
        assert_eq!(idx.doc(&idx.search("widetext", 10)[0]).body(), "widetext");
        let doc = idx.doc(&idx.search("rootnote", 10)[0]);
        assert_eq!((doc.location.as_str(), doc.body()), ("", "rootnote"));
    }

    #[test]
    fn file_text_budget_prefers_shallow_files() {
        let tmp = TempDir::new("budget");
        write(&tmp.0.join("top.txt"), "shallow");
        write(&tmp.0.join("sub").join("deep.txt"), "deeper");
        let bodies: Vec<(String, String)> = file_docs(&tmp.0, 10)
            .into_iter()
            .map(|d| (d.title, d.body))
            .collect();
        assert_eq!(
            bodies,
            [
                ("top.txt".into(), "shallow".into()),
                ("deep.txt".into(), String::new())
            ]
        );
    }

    #[test]
    fn transcript_cache_is_reused_until_the_log_changes() {
        let tmp = TempDir::new("cache");
        let (root, state, cache) = (
            tmp.0.join("Sessions"),
            tmp.0.join("state"),
            tmp.0.join("cache"),
        );
        let log = state.join("s1").join("events.jsonl");
        let cache_file = cache.join("transcripts.json");
        let sessions = [SessionSource {
            id: "s1".into(),
            title: "One".into(),
            folder: None,
            modified: 1,
            transcript: true,
        }];
        let build = || Index::build(&root, &sessions, &state, &cache);
        write(&log, event("user.message", json!({"content": "first"})));
        assert_eq!(build().docs[0].body(), "You: first");

        // Unchanged log: the cached text is used and the cache file is left alone.
        let mut cached: Value = serde_json::from_slice(&fs::read(&cache_file).unwrap()).unwrap();
        cached["s1"]["text"] = "SENTINEL".into();
        write(&cache_file, format!("{cached} "));
        let idx = build();
        assert_eq!((idx.docs[0].body(), idx.stats.transcripts), ("SENTINEL", 1));
        assert!(fs::read_to_string(&cache_file).unwrap().ends_with(' '));

        // Changed log: extracted again.
        let grown = fs::read_to_string(&log).unwrap()
            + &event("assistant.message", json!({"content": "second"}));
        write(&log, grown);
        assert_eq!(build().docs[0].body(), "You: first\n\nCopilot: second");

        // Sessions no longer indexed are dropped from the cache.
        Index::build(&root, &[], &state, &cache);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&cache_file).unwrap()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn empty_query_lists_recent_docs() {
        let idx = index(vec![
            file("old.txt", "\n  old\n\n  content ", 1),
            session("New", "You: hi", 3),
            file("mid.txt", "", 2),
        ]);
        let recent = idx.recent(10);
        assert_eq!(titles(&idx, &recent), ["New", "mid.txt", "old.txt"]);
        assert_eq!(recent[2].snippet, "old content");
        let key = |hits: &[Hit]| {
            hits.iter()
                .map(|h| (h.doc, h.snippet.clone(), h.line))
                .collect::<Vec<_>>()
        };
        for query in ["", "  \t", "\"\""] {
            assert_eq!(key(&idx.search(query, 2)), key(&recent[..2]));
        }
    }

    #[test]
    fn find_at_agrees_with_std() {
        let hay = format!("{}abc é xyz {}abc", "ab".repeat(50), "b".repeat(70));
        let long = "b".repeat(40);
        for needle in [
            "a",
            "ab",
            "abc",
            "bab",
            "é x",
            long.as_str(),
            "zzz",
            "abcabc",
            hay.as_str(),
        ] {
            let expected: Vec<usize> = hay.match_indices(needle).map(|(i, _)| i).collect();
            assert_eq!(
                occurrences(hay.as_bytes(), needle.as_bytes()).collect::<Vec<_>>(),
                expected,
                "{needle}"
            );
        }
        assert_eq!(find_at(b"abc", b"abcd", 0), None);
        assert_eq!(find_at(b"abc", b"c", 9), None);
        assert_eq!(find_at(b"abc", b"", 0), None);
    }

    #[test]
    fn display_path_shortens_home() {
        let home = Some(Path::new(r"C:\Users\Me"));
        assert_eq!(
            display_path(Path::new(r"c:\users\me\Graysense\Sessions\x"), home),
            r"~\Graysense\Sessions\x"
        );
        assert_eq!(display_path(Path::new("C:/Users/Me"), home), "~");
        assert_eq!(
            display_path(Path::new(r"C:\Users\Meg\x"), home),
            r"C:\Users\Meg\x"
        );
    }

    #[test]
    #[ignore = "perf: run with `cargo test --release -- --ignored --nocapture`"]
    fn search_over_20_mb_is_fast() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as usize
        };
        // English-like letter frequencies; 600 words, so most docs hold a given word a few times.
        let letters = b"eeeeeeetttttaaaaooooiiiinnnnsssrrrhhhllddccuummfpgwybvkxjqz";
        let vocab: Vec<String> = (0..600)
            .map(|_| {
                (0..2 + next() % 8)
                    .map(|_| letters[next() % letters.len()] as char)
                    .collect()
            })
            .collect();
        let now = unix_now();
        let docs = (0..2_000)
            .map(|i| {
                let mut body = String::with_capacity(10_300);
                while body.len() < 10_240 {
                    body += &vocab[next() % vocab.len()];
                    body.push(if next() % 12 == 0 { '\n' } else { ' ' });
                }
                file(&format!("file{i}.txt"), &body, now - i * 600)
            })
            .collect();
        let idx = index(docs);
        let mb = idx.docs.iter().map(|d| d.body.len()).sum::<usize>() as f64 / 1e6;
        let long: Vec<&String> = vocab.iter().filter(|w| w.len() >= 6).collect();
        let two_terms = format!("{} {}", long[0], long[1]);
        let queries = [
            two_terms.clone(),
            "zqxjv nothing".into(),
            format!("\"{} {}\" e", long[2], long[3]),
            "e".into(),
        ];
        for query in &queries {
            let mut times: Vec<_> = (0..5)
                .map(|_| {
                    let start = Instant::now();
                    let hits = idx.search(query, 50);
                    (start.elapsed(), hits.len())
                })
                .collect();
            times.sort();
            let (slowest, hits) = times[4];
            println!(
                "{query:?}: {hits} hits, median {:?}, max {slowest:?} over {mb:.1} MB",
                times[2].0
            );
            // Debug builds are far slower; the target is for release builds.
            if *query == two_terms && !cfg!(debug_assertions) {
                assert!(slowest.as_millis() < 50, "{slowest:?}");
            }
        }
    }
}
