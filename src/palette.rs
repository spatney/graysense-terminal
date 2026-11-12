//! Command palette (Ctrl+Shift+P): fuzzy commands, session resume, folder picker.

use eframe::egui::{self, Align2, CornerRadius, Id, Key, Order, RichText, Sense, Stroke, StrokeKind, TextEdit, pos2, vec2};

use crate::agent::backend::SessionInfo;
use crate::kit::{self, icon};
use crate::settings::{LayoutMode, SidePanel};
use crate::theme::{Theme, ThemeId, icon_font, semibold, ui as ui_font};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    NewAgent,
    NewAgentIn,
    NewIncognitoAgent,
    NewShell,
    NewShellIn,
    Resume,
    CaptureIdea,
    CloseBay,
    NextBay,
    PrevBay,
    Maximize,
    Layout(LayoutMode),
    Theme(ThemeId),
    Side(SidePanel),
    ToggleSide,
    FontBigger,
    FontSmaller,
    FontReset,
    ToggleReasoning,
    TogglePerf,
    OpenConfig,
    ReloadMcp,
    ToggleMemory,
    ReviewMemories,
    Search,
    Shortcuts,
    ResumeSession(String),
    OpenDir(String, bool),
    BrowseDir(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Commands,
    Sessions,
    Dirs { agent: bool },
}

struct Entry {
    cmd: Command,
    title: String,
    detail: String,
    icon: &'static str,
    keys: &'static [&'static str],
}

pub struct Palette {
    pub open: bool,
    pub mode: Mode,
    query: String,
    selected: usize,
    focus: bool,
    /// Current memory switch, so the toggle command says what it will do.
    pub memory_on: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Palette { open: false, mode: Mode::Commands, query: String::new(), selected: 0, focus: false, memory_on: true }
    }
}

fn commands(memory_on: bool) -> Vec<Entry> {
    let e = |cmd, title: &str, icon, keys| Entry { cmd, title: title.into(), detail: String::new(), icon, keys };
    let mut v = vec![
        e(Command::NewAgent, "New session", icon::BOT, &["Ctrl", "Shift", "N"][..]),
        e(Command::Search, "Search sessions and files…", icon::SEARCH, &["Ctrl", "K"][..]),
        e(Command::NewAgentIn, "New session in folder…", icon::OPEN_FOLDER, &[][..]),
        e(Command::NewIncognitoAgent, "New incognito session (no memory)", icon::HIDE, &[][..]),
        e(Command::NewShell, "New terminal", icon::TERMINAL, &["Ctrl", "Shift", "T"][..]),
        e(Command::NewShellIn, "New terminal in folder…", icon::OPEN_FOLDER, &[][..]),
        e(Command::Resume, "Resume a past Copilot session…", icon::HISTORY, &[][..]),
        e(Command::CaptureIdea, "Capture an idea", icon::LIGHTBULB, &["Ctrl", "Shift", "I"][..]),
        e(Command::CloseBay, "Close focused bay", icon::CANCEL, &["Ctrl", "Shift", "W"][..]),
        e(Command::NextBay, "Focus next bay", icon::CHEVRON_RIGHT, &["Ctrl", "Tab"][..]),
        e(Command::PrevBay, "Focus previous bay", icon::CHEVRON_RIGHT, &["Ctrl", "Shift", "Tab"][..]),
        e(Command::Maximize, "Maximize / restore focused bay", icon::FULLSCREEN, &["Ctrl", "Shift", "Enter"][..]),
        e(Command::Layout(LayoutMode::Auto), "Layout: grid", icon::TILES, &[][..]),
        e(Command::Layout(LayoutMode::Focus), "Layout: focus + stack", icon::TILES, &[][..]),
        e(Command::Layout(LayoutMode::Columns), "Layout: columns", icon::TILES, &[][..]),
        e(Command::Layout(LayoutMode::Rows), "Layout: rows", icon::TILES, &[][..]),
        e(Command::ToggleSide, "Toggle side panel", icon::DOCK, &["Ctrl", "Shift", "B"][..]),
        e(Command::Side(SidePanel::Missions), "Show missions", icon::LIGHTBULB, &[][..]),
        e(Command::Side(SidePanel::Sessions), "Show sessions", icon::HISTORY, &["Ctrl", "Shift", "H"][..]),
        e(Command::Side(SidePanel::Memory), "Show memory", icon::MEMORY, &["Ctrl", "Shift", "M"][..]),
        e(Command::ReviewMemories, "Review memory suggestions", icon::MEMORY, &[][..]),
        e(Command::ToggleMemory, if memory_on { "Turn memory off" } else { "Turn memory on" }, icon::MEMORY, &[][..]),
        e(Command::Side(SidePanel::Mcp), "Show MCP servers", icon::CONNECT, &[][..]),
        e(Command::Side(SidePanel::Plugins), "Show plugins", icon::PUZZLE, &[][..]),
        e(Command::Side(SidePanel::Settings), "Open settings", icon::SETTINGS, &[][..]),
        e(Command::ReloadMcp, "Reload MCP servers", icon::REFRESH, &[][..]),
        e(Command::FontBigger, "Terminal font: larger", icon::ADD, &["Ctrl", "="][..]),
        e(Command::FontSmaller, "Terminal font: smaller", icon::REMOVE, &["Ctrl", "-"][..]),
        e(Command::FontReset, "Terminal font: reset", icon::REFRESH, &["Ctrl", "0"][..]),
        e(Command::ToggleReasoning, "Toggle agent reasoning", icon::BRAIN, &[][..]),
        e(Command::TogglePerf, "Toggle performance stats", icon::BAR_CHART, &[][..]),
        e(Command::OpenConfig, "Open config folder", icon::FOLDER, &[][..]),
        e(Command::Shortcuts, "Keyboard shortcuts", icon::INFO, &["Ctrl", "/"][..]),
    ];
    for t in ThemeId::ALL {
        v.push(Entry { cmd: Command::Theme(t), title: format!("Theme: {}", t.name()), detail: String::new(), icon: icon::SETTINGS, keys: &[] });
    }
    v
}

/// Subsequence fuzzy score (higher is better); `None` if no match.
fn score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text_l = text.to_lowercase();
    if let Some(pos) = text_l.find(&query.to_lowercase()) {
        return Some(1000 - pos as i32);
    }
    let mut it = text_l.chars();
    let mut s = 0;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let mut gap = 0;
        loop {
            match it.next() {
                Some(c) if c == qc => break,
                Some(_) => gap += 1,
                None => return None,
            }
        }
        s -= gap;
    }
    Some(s)
}

impl Palette {
    pub fn open(&mut self, mode: Mode) {
        self.open = true;
        self.mode = mode;
        self.query.clear();
        self.selected = 0;
        self.focus = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, sessions: &[SessionInfo], recent: &[String]) -> Option<Command> {
        if !self.open {
            return None;
        }
        let mut picked = None;
        let screen = ctx.content_rect();
        // Backdrop.
        let backdrop = egui::Area::new(Id::unique("palette-backdrop")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close palette"));
            ui.painter().rect_filled(r, 0.0, egui::Color32::from_black_alpha(if theme.dark { 90 } else { 40 }));
            resp
        });
        if backdrop.inner.clicked() {
            self.open = false;
            return None;
        }

        let entries: Vec<Entry> = match self.mode {
            Mode::Commands => commands(self.memory_on),
            Mode::Sessions => sessions
                .iter()
                .map(|s| Entry {
                    cmd: Command::ResumeSession(s.id.clone()),
                    title: s.summary.clone().filter(|x| !x.is_empty()).unwrap_or_else(|| format!("Session {}", &s.id[..8.min(s.id.len())])),
                    detail: [crate::app::relative_time(&s.modified, chrono::Utc::now()), s.client.clone().unwrap_or_default()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("  ·  "),
                    icon: icon::HISTORY,
                    keys: &[],
                })
                .collect(),
            Mode::Dirs { agent } => {
                let mut v: Vec<Entry> = recent
                    .iter()
                    .map(|d| Entry { cmd: Command::OpenDir(d.clone(), agent), title: crate::settings::dir_name(std::path::Path::new(d)), detail: d.clone(), icon: icon::FOLDER, keys: &[] })
                    .collect();
                v.insert(0, Entry { cmd: Command::BrowseDir(agent), title: "Browse for a folder…".into(), detail: String::new(), icon: icon::OPEN_FOLDER, keys: &[] });
                v
            }
        };
        let mut scored: Vec<(i32, &Entry)> = entries.iter().filter_map(|e| score(&self.query, &format!("{} {}", e.title, e.detail)).map(|s| (s, e))).collect();
        if !self.query.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0));
        }
        let n = scored.len();
        if self.selected >= n {
            self.selected = n.saturating_sub(1);
        }

        let width = 560.0f32.min(screen.width() - 40.0);
        let pos = pos2(screen.center().x - width / 2.0, screen.top() + 80.0);
        egui::Area::new(Id::unique("palette")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme.surface)
                .stroke(Stroke::new(1.0, theme.border_strong))
                .corner_radius(CornerRadius::same(10))
                .shadow(ui.visuals().popup_shadow)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(width);
                    let hint = match self.mode {
                        Mode::Commands => "Type a command…",
                        Mode::Sessions => "Search past sessions…",
                        Mode::Dirs { .. } => "Pick a folder…",
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::SEARCH).font(icon_font(14.0)).color(theme.text_dim));
                        let resp = ui.add(TextEdit::singleline(&mut self.query).hint_text(hint).frame(egui::Frame::NONE).font(ui_font(15.0)).desired_width(f32::INFINITY));
                        if self.focus {
                            resp.request_focus();
                            self.focus = false;
                        }
                        if resp.changed() {
                            self.selected = 0;
                        }
                    });
                    ui.add_space(4.0);
                    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, theme.border));
                    ui.add_space(4.0);
                    if n == 0 {
                        ui.label(RichText::new(if self.mode == Mode::Sessions && sessions.is_empty() { "Loading sessions…" } else { "No matches" }).color(theme.text_dim));
                    }
                    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                        let pointer_moved = ui.input(|inp| inp.pointer.is_moving());
                        for (i, (_, e)) in scored.iter().enumerate() {
                            let selected = i == self.selected;
                            let (rect, resp) = ui.allocate_exact_size(vec2(width, if e.detail.is_empty() { 32.0 } else { 44.0 }), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &e.title));
                            if resp.hovered() && pointer_moved {
                                self.selected = i;
                            }
                            let p = ui.painter();
                            if selected {
                                p.rect(rect, CornerRadius::same(6), theme.accent_soft(), Stroke::NONE, StrokeKind::Inside);
                                if ui.input(|inp| inp.key_pressed(Key::ArrowUp) || inp.key_pressed(Key::ArrowDown)) {
                                    resp.scroll_to_me(None);
                                }
                            } else if resp.hovered() {
                                p.rect(rect, CornerRadius::same(6), theme.surface2, Stroke::NONE, StrokeKind::Inside);
                            }
                            p.text(rect.left_center() + vec2(14.0, 0.0), Align2::CENTER_CENTER, e.icon, icon_font(13.0), if selected { theme.accent } else { theme.text_dim });
                            let keys_w = if e.keys.is_empty() { 0.0 } else { 26.0 * e.keys.len() as f32 + 10.0 };
                            let text_w = (rect.width() - 32.0 - 12.0 - keys_w).max(40.0);
                            if e.detail.is_empty() {
                                let g = kit::elided(p, &e.title, ui_font(13.5), theme.text, text_w);
                                p.galley(pos2(rect.left() + 32.0, rect.center().y - g.size().y / 2.0), g, theme.text);
                            } else {
                                let g = kit::elided(p, &e.title, semibold(13.0), theme.text, text_w);
                                p.galley(rect.left_top() + vec2(32.0, 6.0), g, theme.text);
                                let g = kit::elided(p, &e.detail, ui_font(11.5), theme.text_faint, text_w);
                                p.galley(rect.left_top() + vec2(32.0, 25.0), g, theme.text_faint);
                            }
                            if !e.keys.is_empty() {
                                kit::kbd(p, rect.right_center() - vec2(10.0, 0.0), e.keys, theme);
                            }
                            if resp.clicked() {
                                picked = Some(e.cmd.clone());
                            }
                        }
                    });
                });
        });

        let (up, down, enter, esc) = ctx.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        if down && self.selected + 1 < n {
            self.selected += 1;
        }
        if enter
            && let Some((_, e)) = scored.get(self.selected) {
                picked = Some(e.cmd.clone());
            }
        if esc {
            self.open = false;
        }
        if picked.is_some() {
            self.open = false;
        }
        picked
    }
}
