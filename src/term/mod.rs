//! PTY-backed terminal bays: alacritty_terminal VT engine + ConPTY.

pub mod boxdraw;
pub mod input;
pub mod render;
pub mod view;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::Rgb;
use eframe::egui::{self, Color32};
use parking_lot::Mutex;

use crate::theme::Theme;

/// ConPTY reports the shell's executable path as the initial title; show the
/// program name instead of `C:\Program Files\...\pwsh.exe`.
fn friendly_title(t: &str) -> String {
    let trimmed = t.trim().trim_matches('"');
    let p = Path::new(trimmed);
    let is_exe = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if is_exe && (p.is_absolute() || trimmed.contains('\\'))
        && let Some(stem) = p.file_stem() {
            return stem.to_string_lossy().into_owned();
        }
    t.to_owned()
}

/// Events forwarded from the PTY thread to the UI thread.
pub enum TermEvent {
    Title(String),
    ResetTitle,
    Bell,
    Exit,
    ChildExit(i32),
    Clipboard(String),
}

/// Shared state the PTY thread needs to answer terminal queries without the UI thread.
struct Shared {
    size: Mutex<WindowSize>,
    palette: Mutex<Palette>,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub fg: Color32,
    pub bg: Color32,
    pub cursor: Color32,
    pub ansi: [Color32; 16],
}

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        Palette { fg: theme.term_fg, bg: theme.term_bg, cursor: theme.accent, ansi: theme.ansi }
    }
}

#[derive(Clone)]
pub struct Proxy {
    tx: Sender<TermEvent>,
    ctx: egui::Context,
    writer: Arc<OnceLock<EventLoopSender>>,
    shared: Arc<Shared>,
    dirty: Arc<AtomicBool>,
    wakeups: Arc<AtomicU64>,
}

impl Proxy {
    fn write(&self, bytes: Vec<u8>) {
        if let Some(w) = self.writer.get() {
            let _ = w.send(Msg::Input(Cow::Owned(bytes)));
        }
    }
}

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => {
                self.dirty.store(true, Ordering::Relaxed);
                self.wakeups.fetch_add(1, Ordering::Relaxed);
                self.ctx.request_repaint();
            }
            Event::PtyWrite(text) => self.write(text.into_bytes()),
            Event::ColorRequest(index, fmt) => {
                let p = *self.shared.palette.lock();
                let c = match index {
                    0..=15 => p.ansi[index],
                    16..=255 => xterm_256(index as u8, &p),
                    256 => p.fg,
                    257 => p.bg,
                    _ => p.cursor,
                };
                self.write(fmt(Rgb { r: c.r(), g: c.g(), b: c.b() }).into_bytes());
            }
            Event::TextAreaSizeRequest(fmt) => {
                let size = *self.shared.size.lock();
                self.write(fmt(size).into_bytes());
            }
            Event::Title(t) => self.forward(TermEvent::Title(t)),
            Event::ResetTitle => self.forward(TermEvent::ResetTitle),
            Event::Bell => self.forward(TermEvent::Bell),
            Event::Exit => self.forward(TermEvent::Exit),
            Event::ChildExit(status) => self.forward(TermEvent::ChildExit(status.code().unwrap_or(-1))),
            Event::ClipboardStore(_, text) => self.forward(TermEvent::Clipboard(text)),
            Event::ClipboardLoad(_, _) | Event::CursorBlinkingChange | Event::MouseCursorDirty => {}
        }
    }
}

impl Proxy {
    fn forward(&self, ev: TermEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }
}

pub fn xterm_256(i: u8, p: &Palette) -> Color32 {
    match i {
        0..=15 => p.ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub struct TermPane {
    pub term: Arc<FairMutex<Term<Proxy>>>,
    sender: EventLoopSender,
    events: Receiver<TermEvent>,
    shared: Arc<Shared>,
    pub dirty: Arc<AtomicBool>,
    wakeups: Arc<AtomicU64>,
    last_wakeups: u64,
    pub title: String,
    pub default_title: String,
    pub cwd: PathBuf,
    pub spec: SpawnSpec,
    pub exited: Option<i32>,
    pub cols: usize,
    pub rows: usize,
    /// Decaying output activity in [0, 1] used for glow intensity.
    pub activity: f32,
    pub bell: f32,
    pub history: Vec<f32>,
    pub(crate) view: view::ViewState,
    pub snapshot: render::Snapshot,
}

impl TermPane {
    pub fn spawn(ctx: &egui::Context, spec: SpawnSpec, theme: &Theme, cols: usize, rows: usize) -> anyhow::Result<Self> {
        let cols = cols.max(20);
        let rows = rows.max(5);
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            size: Mutex::new(WindowSize { num_lines: rows as u16, num_cols: cols as u16, cell_width: 9, cell_height: 18 }),
            palette: Mutex::new(Palette::from_theme(theme)),
        });
        let dirty = Arc::new(AtomicBool::new(true));
        let wakeups = Arc::new(AtomicU64::new(0));
        let proxy = Proxy {
            tx,
            ctx: ctx.clone(),
            writer: Arc::new(OnceLock::new()),
            shared: shared.clone(),
            dirty: dirty.clone(),
            wakeups: wakeups.clone(),
        };

        let mut env = std::collections::HashMap::new();
        env.insert("TERM_PROGRAM".to_string(), "Graysense".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        let options = tty::Options {
            shell: Some(tty::Shell::new(spec.program.clone(), spec.args.clone())),
            working_directory: Some(spec.cwd.clone()),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        let window_size = *shared.size.lock();
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let pty = tty::new(&options, window_size, NEXT_ID.fetch_add(1, Ordering::Relaxed))?;

        let config = Config { scrolling_history: 20_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &TermSize::new(cols, rows), proxy.clone())));
        let event_loop = EventLoop::new(term.clone(), proxy.clone(), pty, true, false)?;
        let sender = event_loop.channel();
        let _ = proxy.writer.set(sender.clone());
        event_loop.spawn();

        let title = Path::new(&spec.program)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "shell".into());
        Ok(TermPane {
            term,
            sender,
            events: rx,
            shared,
            dirty,
            wakeups,
            last_wakeups: 0,
            default_title: title.clone(),
            title,
            cwd: spec.cwd.clone(),
            spec,
            exited: None,
            cols,
            rows,
            activity: 0.0,
            bell: 0.0,
            history: vec![0.0; 48],
            view: view::ViewState::default(),
            snapshot: render::Snapshot::default(),
        })
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            let _ = self.sender.send(Msg::Input(bytes));
        }
    }

    pub fn write_str(&self, s: &str) {
        self.write(s.as_bytes().to_vec());
    }

    /// Paste text, honouring bracketed-paste mode.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            let clean = text.replace('\x1b', "");
            self.write(format!("\x1b[200~{clean}\x1b[201~").into_bytes());
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize, cell_w: f32, cell_h: f32) {
        let cols = cols.max(10);
        let rows = rows.max(3);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let ws = WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: cell_w.round().max(1.0) as u16,
            cell_height: cell_h.round().max(1.0) as u16,
        };
        *self.shared.size.lock() = ws;
        self.term.lock().resize(TermSize::new(cols, rows));
        let _ = self.sender.send(Msg::Resize(ws));
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn set_palette(&self, theme: &Theme) {
        *self.shared.palette.lock() = Palette::from_theme(theme);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn palette(&self) -> Palette {
        *self.shared.palette.lock()
    }

    /// Drain PTY events; returns clipboard text requested via OSC 52 (if any).
    pub fn pump(&mut self, dt: f32) -> Option<String> {
        let mut clip = None;
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                TermEvent::Title(t) => self.title = friendly_title(&t),
                TermEvent::ResetTitle => self.title = self.default_title.clone(),
                TermEvent::Bell => self.bell = 1.0,
                TermEvent::Exit => {
                    if self.exited.is_none() {
                        self.exited = Some(0);
                    }
                }
                TermEvent::ChildExit(code) => self.exited = Some(code),
                TermEvent::Clipboard(text) => clip = Some(text),
            }
        }
        let w = self.wakeups.load(Ordering::Relaxed);
        let delta = w.saturating_sub(self.last_wakeups);
        self.last_wakeups = w;
        if delta > 0 {
            self.activity = (self.activity + 0.35 + delta as f32 * 0.05).min(1.0);
        }
        self.activity = (self.activity - dt * 0.9).max(0.0);
        self.bell = (self.bell - dt * 1.8).max(0.0);
        clip
    }

    /// Called ~4x per second to roll the activity sparkline.
    pub fn sample_history(&mut self) {
        self.history.remove(0);
        self.history.push(self.activity);
    }

    pub fn scroll(&mut self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn scroll_to_bottom(&mut self) {
        let mut term = self.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn clear_selection(&mut self) {
        let mut term = self.term.lock();
        if term.selection.take().is_some() {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    pub fn select_all(&mut self) {
        let mut term = self.term.lock();
        let top = term.grid().topmost_line();
        let bottom = term.grid().bottommost_line();
        let last_col = Column(term.grid().columns().saturating_sub(1));
        let mut sel = Selection::new(alacritty_terminal::selection::SelectionType::Simple, Point::new(top, Column(0)), alacritty_terminal::index::Side::Left);
        sel.update(Point::new(bottom, last_col), alacritty_terminal::index::Side::Right);
        term.selection = Some(sel);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn clear_scrollback(&mut self) {
        // ED 3 clears scrollback in conhost-compatible shells; also ask the shell to clear.
        self.write_str("\x1b[3J");
        self.write_str("cls\r");
    }

    pub fn shutdown(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }

    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    pub fn line_text(&self, line: Line) -> String {
        let term = self.term.lock();
        let cols = term.grid().columns();
        if line < term.grid().topmost_line() || line > term.grid().bottommost_line() {
            return String::new();
        }
        term.bounds_to_string(Point::new(line, Column(0)), Point::new(line, Column(cols - 1)))
    }
}

impl Drop for TermPane {
    fn drop(&mut self) {
        self.shutdown();
    }
}
