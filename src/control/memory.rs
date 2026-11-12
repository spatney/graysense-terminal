//! Memory panel: everything Graysense has learned, with a review inbox, search, inline editing,
//! provenance, approval rules and the on/off controls.

use eframe::egui::{self, Align2, CornerRadius, CursorIcon, Frame, Id, Margin, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, pos2, vec2};

use crate::agent::backend::ModelInfo;
use crate::kit::{self, icon};
use crate::memory::{Kind, Memory, MemoryId, MemoryStore, Origin, Project, Proposal, Scope, State, now};
use crate::settings::MemorySettings;
use crate::theme::{Theme, icon_font, mix, semibold, ui as ui_font};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Review,
    All,
    You,
    Project,
    Sessions,
    Rules,
    Deleted,
}

struct Draft {
    id: Option<MemoryId>,
    text: String,
    kind: Kind,
    global: bool,
    /// The user picked a scope; stop following the kind.
    scope_touched: bool,
    focus: bool,
}

pub enum MemoryAction {
    SettingsChanged,
    ExportJson,
    ExportMarkdown,
    Import,
    Toast(String),
}

pub struct MemoryPanel {
    filter: Filter,
    query: String,
    draft: Option<Draft>,
    show_settings: bool,
    confirm_clear: bool,
    /// Memory to scroll to and flash (e.g. from an agent's "Remembered" chip).
    highlight: Option<(MemoryId, f64)>,
    /// A curator run is in progress.
    pub busy: bool,
    /// When learning last ran, and why it failed if it did.
    pub last_run: Option<(i64, Option<String>)>,
}

impl Default for MemoryPanel {
    fn default() -> Self {
        Self { filter: Filter::All, query: String::new(), draft: None, show_settings: false, confirm_clear: false, highlight: None, busy: false, last_run: None }
    }
}

fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Preference => icon::HEART,
        Kind::Fact => icon::INFO,
        Kind::Procedure => icon::CHECKLIST,
        Kind::Episode => icon::HISTORY,
        Kind::Rule => icon::SHIELD,
    }
}

fn ago(t: i64) -> String {
    let d = (now() - t).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        86_400..=2_591_999 => format!("{}d ago", d / 86_400),
        _ => chrono::DateTime::from_timestamp(t, 0).map(|x| x.with_timezone(&chrono::Local).format("%b %-d").to_string()).unwrap_or_default(),
    }
}

impl MemoryPanel {
    /// Open the panel focused on one memory.
    pub fn reveal(&mut self, id: MemoryId, store: &MemoryStore, t: f64) {
        self.query.clear();
        self.filter = match store.get(id) {
            Some(m) if m.state == State::Pending || m.proposal.is_some() => Filter::Review,
            _ => Filter::All,
        };
        self.highlight = Some((id, t));
    }

    pub fn show_review(&mut self) {
        self.filter = Filter::Review;
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        store: &mut MemoryStore,
        settings: &mut MemorySettings,
        project: Option<&Project>,
        models: &[ModelInfo],
        t: f64,
    ) -> Vec<MemoryAction> {
        let mut out = Vec::new();

        // Header: title, add, settings, master switch.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Memory").font(semibold(15.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut on = settings.enabled;
                if kit::toggle_named(ui, &mut on, theme, "Memory").on_hover_text(if on { "Turn memory off" } else { "Turn memory on" }).changed() {
                    settings.enabled = on;
                    out.push(MemoryAction::SettingsChanged);
                    out.push(MemoryAction::Toast(if on { "Memory is on".into() } else { "Memory is off: agents won't learn or remember".into() }));
                }
                if kit::icon_button_sized(ui, icon::SETTINGS, "Memory settings", theme, 26.0, self.show_settings).clicked() {
                    self.show_settings = !self.show_settings;
                }
                if kit::icon_button_sized(ui, icon::ADD, "Add a memory", theme, 26.0, false).clicked() {
                    self.draft = Some(Draft { id: None, text: String::new(), kind: Kind::Preference, global: true, scope_touched: false, focus: true });
                }
            });
        });

        let inbox = store.inbox_count();
        if !settings.enabled {
            ui.label(RichText::new("Off — agents don't learn or see memories.").font(ui_font(12.0)).color(theme.warn));
        } else {
            let n = store.knowledge_count();
            let count = format!("{n} {}", if n == 1 { "memory" } else { "memories" });
            let (detail, color, tip) = match (&self.last_run, self.busy, settings.learn) {
                (_, _, false) => ("not learning".to_owned(), theme.text_dim, None),
                (_, true, _) => ("learning from a conversation…".to_owned(), theme.accent, None),
                (Some((_, Some(e))), _, _) => ("couldn't learn last time".to_owned(), theme.warn, Some(e.clone())),
                (Some((at, None)), _, _) => (format!("learned {}", ago(*at)), theme.text_dim, None),
                (None, _, _) => ("learning from your agents".to_owned(), theme.text_dim, None),
            };
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new(format!("{count}  ·  ")).font(ui_font(12.0)).color(theme.text_dim));
                let r = ui.label(RichText::new(detail).font(ui_font(12.0)).color(color));
                if let Some(e) = tip {
                    r.on_hover_text(e);
                }
            });
        }
        ui.add_space(6.0);

        if self.show_settings {
            self.settings_ui(ui, theme, store, settings, models, &mut out);
            ui.add_space(6.0);
        }

        if let Some(d) = &mut self.draft
            && d.id.is_none() {
                let mut done = None;
                draft_ui(ui, theme, d, project, &mut done);
                match done {
                    Some(true) => {
                        let scope = if d.global { Scope::Global } else { project.map(|p| p.scope()).unwrap_or(Scope::Global) };
                        if store.insert(d.kind, scope, &d.text, Origin::User, State::Active, 1.0, None).is_none() {
                            out.push(MemoryAction::Toast("That looks like a secret, so it wasn't saved.".into()));
                        }
                        self.draft = None;
                    }
                    Some(false) => self.draft = None,
                    None => {}
                }
                ui.add_space(6.0);
            }

        // Search.
        ui.add(TextEdit::singleline(&mut self.query).hint_text(format!("{}  Search memories", icon::SEARCH)).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6)));
        ui.add_space(6.0);

        // Review and Deleted are separate lists with a way back; everything else is a scope chip.
        let deleted = store.deleted_count();
        match self.filter {
            Filter::Review | Filter::Deleted => {
                let (title, note) = if self.filter == Filter::Review {
                    ("Review suggestions", "Things your agents picked up that need a quick yes or no.")
                } else {
                    ("Recently deleted", "Kept for 30 days so learning doesn't bring them back.")
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if kit::icon_button_sized(ui, icon::BACK, "Back to all memories", theme, 24.0, false).clicked() {
                        self.filter = Filter::All;
                    }
                    ui.label(RichText::new(title).font(semibold(13.0)));
                });
                ui.add(egui::Label::new(RichText::new(note).font(ui_font(11.5)).color(theme.text_faint)).wrap());
            }
            _ => {
                if inbox > 0 && review_banner(ui, theme, inbox).clicked() {
                    self.filter = Filter::Review;
                }
                let project_label = project.map(|p| p.name.clone()).unwrap_or_else(|| "Projects".into());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for (f, label) in [(Filter::All, "All"), (Filter::You, "You"), (Filter::Project, project_label.as_str()), (Filter::Sessions, "Sessions"), (Filter::Rules, "Rules")] {
                        if filter_chip(ui, theme, self.filter == f, label).clicked() {
                            self.filter = f;
                        }
                    }
                });
            }
        }
        ui.add_space(6.0);

        let q = self.query.trim().to_lowercase();
        let mut items: Vec<MemoryId> = store
            .memories
            .iter()
            .filter(|m| (self.filter == Filter::Deleted) == (m.state == State::Archived))
            .filter(|m| match self.filter {
                Filter::Review => m.state == State::Pending || m.proposal.is_some(),
                Filter::All => m.state == State::Active && m.kind != Kind::Episode && m.kind != Kind::Rule,
                Filter::You => m.state == State::Active && m.scope == Scope::Global && m.kind != Kind::Rule,
                Filter::Project => m.state == State::Active && m.kind != Kind::Episode && m.kind != Kind::Rule && match (&m.scope, project) {
                    (Scope::Project(k), Some(p)) => *k == p.key,
                    (Scope::Project(_), None) => true,
                    _ => false,
                },
                Filter::Sessions => m.state == State::Active && m.kind == Kind::Episode,
                Filter::Rules => m.kind == Kind::Rule && m.state == State::Active,
                Filter::Deleted => true,
            })
            .filter(|m| q.is_empty() || m.text.to_lowercase().contains(&q) || m.sources.iter().any(|s| s.title.as_deref().is_some_and(|x| x.to_lowercase().contains(&q))))
            .map(|m| m.id)
            .collect();
        // Pinned first, then most recently changed.
        items.sort_by_key(|id| store.get(*id).map(|m| (std::cmp::Reverse(m.pinned), std::cmp::Reverse(m.updated))).unwrap_or((std::cmp::Reverse(false), std::cmp::Reverse(0))));

        let enabled = settings.enabled;
        egui::ScrollArea::vertical().id_salt("memory-scroll").auto_shrink([false; 2]).show(ui, |ui| {
            if !enabled {
                ui.multiply_opacity(0.55);
            }
            if items.is_empty() {
                empty_state(ui, theme, self.filter, !q.is_empty());
            }
            let mut act: Option<(MemoryId, CardAct)> = None;
            for id in items {
                let Some(m) = store.get(id).cloned() else { continue };
                let flash = self.highlight.filter(|(h, _)| *h == id).map(|(_, at)| (1.0 - (t - at) / 1.6).clamp(0.0, 1.0) as f32).unwrap_or(0.0);
                let editing = self.draft.as_ref().is_some_and(|d| d.id == Some(id));
                if editing {
                    let d = self.draft.as_mut().unwrap();
                    let mut done = None;
                    draft_ui(ui, theme, d, project, &mut done);
                    match done {
                        Some(true) => {
                            let scope = if d.global { Scope::Global } else { match &m.scope { Scope::Project(k) => Scope::Project(k.clone()), Scope::Global => project.map(|p| p.scope()).unwrap_or(Scope::Global) } };
                            store.edit(id, &d.text, d.kind, scope);
                            self.draft = None;
                        }
                        Some(false) => self.draft = None,
                        None => {}
                    }
                } else {
                    let resp = card(ui, theme, &m, project, flash);
                    if flash > 0.0 {
                        resp.0.scroll_to_me(Some(egui::Align::Center));
                        ui.ctx().request_repaint();
                    }
                    if let Some(a) = resp.1 {
                        act = Some((id, a));
                    }
                }
                ui.add_space(6.0);
            }
            if deleted > 0 && !matches!(self.filter, Filter::Deleted | Filter::Review) {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    if kit::mini_button(ui, &format!("Recently deleted ({deleted})"), theme).clicked() {
                        self.filter = Filter::Deleted;
                    }
                });
            }
            if let Some((id, a)) = act {
                match a {
                    CardAct::Accept => store.accept(id),
                    CardAct::Dismiss => store.dismiss(id),
                    CardAct::Delete => {
                        store.archive(id, "deleted");
                        out.push(MemoryAction::Toast("Deleted. You can restore it from Recently deleted.".into()));
                    }
                    CardAct::Restore => {
                        store.restore(id);
                        out.push(MemoryAction::Toast("Memory restored".into()));
                    }
                    CardAct::Purge => store.purge(id),
                    CardAct::Pin(p) => store.set_pinned(id, p),
                    CardAct::Edit => {
                        if let Some(m) = store.get(id) {
                            let text = match &m.proposal {
                                Some(Proposal::Update(t)) => t.clone(),
                                _ => m.text.clone(),
                            };
                            self.draft = Some(Draft {
                                id: Some(id),
                                text,
                                kind: m.kind,
                                global: m.scope == Scope::Global,
                                scope_touched: true,
                                focus: true,
                            });
                        }
                    }
                }
            }
        });
        if self.highlight.is_some_and(|(_, at)| t - at > 1.6) {
            self.highlight = None;
        }
        out
    }

    fn settings_ui(&mut self, ui: &mut Ui, theme: &Theme, store: &mut MemoryStore, settings: &mut MemorySettings, models: &[ModelInfo], out: &mut Vec<MemoryAction>) {
        Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut changed = false;
            changed |= kit::toggle_row(ui, &mut settings.learn, theme, "Learn from conversations").changed();
            changed |= kit::toggle_row(ui, &mut settings.inject, theme, "Share relevant memories with agents").changed();
            changed |= kit::toggle_row(ui, &mut settings.review_all, theme, "Ask before saving anything learned").changed();
            ui.add_space(4.0);
            ui.label(RichText::new("Learning model").font(ui_font(11.5)).color(theme.text_dim));
            let current = settings.model.clone().unwrap_or_else(|| "Automatic (lowest cost)".into());
            egui::ComboBox::from_id_salt("memory-model").selected_text(current).width(ui.available_width().min(260.0)).show_ui(ui, |ui| {
                if ui.selectable_label(settings.model.is_none(), "Automatic (lowest cost)").clicked() {
                    settings.model = None;
                    changed = true;
                }
                for m in models {
                    let label = match m.multiplier {
                        Some(x) => format!("{}  ·  {x}×", m.name),
                        None => m.name.clone(),
                    };
                    if ui.selectable_label(settings.model.as_deref() == Some(m.id.as_str()), label).clicked() {
                        settings.model = Some(m.id.clone());
                        changed = true;
                    }
                }
            });
            if changed {
                out.push(MemoryAction::SettingsChanged);
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                if kit::mini_button(ui, "Export JSON", theme).clicked() {
                    out.push(MemoryAction::ExportJson);
                }
                if kit::mini_button(ui, "Export Markdown", theme).clicked() {
                    out.push(MemoryAction::ExportMarkdown);
                }
                if kit::mini_button(ui, "Import…", theme).clicked() {
                    out.push(MemoryAction::Import);
                }
                if self.confirm_clear {
                    if kit::mini_button_tinted(ui, "Delete everything", theme, theme.err).clicked() {
                        store.clear_all();
                        self.confirm_clear = false;
                        out.push(MemoryAction::Toast("All memories deleted".into()));
                    }
                    if kit::mini_button(ui, "Cancel", theme).clicked() {
                        self.confirm_clear = false;
                    }
                } else if kit::mini_button_tinted(ui, "Clear all…", theme, theme.err).clicked() {
                    self.confirm_clear = true;
                }
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new("Stored only on this PC. Secrets are never saved. Learning runs in the background with a low-cost model and never uses tools.")
                    .font(ui_font(11.0))
                    .color(theme.text_faint),
            );
        });
    }
}

#[derive(Clone, Copy)]
enum CardAct {
    Accept,
    Dismiss,
    Delete,
    Restore,
    Purge,
    Edit,
    Pin(bool),
}

fn filter_chip(ui: &mut Ui, theme: &Theme, selected: bool, label: &str) -> egui::Response {
    let font = ui_font(12.0);
    let w = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), theme.text).size().x + 20.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, label));
    let p = ui.painter();
    let fill = if selected { theme.accent_soft() } else if resp.hovered() { theme.surface2 } else { theme.surface };
    p.rect(r, CornerRadius::same(13), fill, Stroke::new(1.0, if selected { mix(theme.border, theme.accent, 0.6) } else { theme.border }), StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, label, font, if selected { theme.text } else { theme.text_dim });
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// "3 suggestions to review" call-to-action above the list.
fn review_banner(ui: &mut Ui, theme: &Theme, n: usize) -> egui::Response {
    let text = if n == 1 { "1 suggestion to review".to_owned() } else { format!("{n} suggestions to review") };
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
    let p = ui.painter();
    let fill = mix(theme.surface2, theme.accent, if resp.hovered() { 0.2 } else { 0.12 });
    p.rect(r, CornerRadius::same(8), fill, Stroke::new(1.0, mix(theme.border, theme.accent, 0.5)), StrokeKind::Inside);
    p.text(pos2(r.left() + 14.0, r.center().y), Align2::CENTER_CENTER, icon::SPARKLE, icon_font(12.0), theme.accent);
    p.text(pos2(r.left() + 28.0, r.center().y), Align2::LEFT_CENTER, text, semibold(12.5), theme.text);
    p.text(pos2(r.right() - 14.0, r.center().y), Align2::CENTER_CENTER, icon::CHEVRON_RIGHT, icon_font(9.0), theme.text_dim);
    ui.add_space(6.0);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn empty_state(ui: &mut Ui, theme: &Theme, filter: Filter, searching: bool) {
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon::MEMORY).font(icon_font(22.0)).color(theme.text_faint));
        ui.add_space(4.0);
        let (title, body) = if searching {
            ("No matches", "Try other words.")
        } else {
            match filter {
                Filter::Review => ("All caught up", "When your agents learn something they're not sure about, it waits here for you."),
                Filter::Sessions => ("No sessions yet", "Short summaries of past work appear here and help new agents pick up where you left off."),
                Filter::Rules => ("No approval rules", "After you approve the same command a few times, Graysense offers to allow it automatically in that project."),
                Filter::Deleted => ("Nothing deleted", "Deleted memories stay here for 30 days."),
                _ => ("Nothing learned yet", "As you work, your agents learn your preferences and each project's conventions. You can also add memories yourself."),
            }
        };
        ui.label(RichText::new(title).font(semibold(13.0)).color(theme.text_dim));
        ui.add(egui::Label::new(RichText::new(body).font(ui_font(12.0)).color(theme.text_faint)).wrap());
    });
}

fn scope_label(m: &Memory, project: Option<&Project>) -> String {
    match &m.scope {
        Scope::Global => "You".into(),
        Scope::Project(k) => match project {
            Some(p) if &p.key == k => p.name.clone(),
            _ => std::path::Path::new(k).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into()),
        },
    }
}

/// One memory. Returns the card response (for scrolling) and any action.
fn card(ui: &mut Ui, theme: &Theme, m: &Memory, project: Option<&Project>, flash: f32) -> (egui::Response, Option<CardAct>) {
    let mut act = None;
    let pending = m.state == State::Pending;
    let archived = m.state == State::Archived;
    let review = !archived && (pending || m.proposal.is_some());
    let border = if review { mix(theme.border, theme.accent, 0.55) } else if flash > 0.0 { mix(theme.border, theme.accent, flash) } else { theme.border };
    let fill = if flash > 0.0 { mix(theme.surface2, theme.accent, 0.12 * flash) } else { theme.surface2 };
    // Hover is judged against the card's rect from the previous frame (the frame's max rect
    // runs to the bottom of the panel).
    let hover_id = Id::unique(("memory-card", m.id));
    let hovered = ui.ctx().data(|d| d.get_temp::<egui::Rect>(hover_id)).is_some_and(|r| ui.rect_contains_pointer(r));
    let inner = Frame::new().fill(fill).stroke(Stroke::new(1.0, border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());

        // Badge line for inbox items.
        if review {
            let (label, color) = match (&m.proposal, m.kind) {
                (Some(Proposal::Update(_)), _) => ("Update suggested".to_owned(), theme.accent),
                (Some(Proposal::Delete), _) => ("Looks outdated".to_owned(), theme.warn),
                (None, Kind::Rule) => ("Suggested rule".to_owned(), theme.accent),
                _ => (format!("Suggested · {:.0}% sure", m.confidence * 100.0), theme.accent),
            };
            ui.label(RichText::new(label).font(semibold(11.0)).color(color));
        }

        // Main text (with the proposed change, if any).
        ui.horizontal_top(|ui| {
            ui.label(RichText::new(kind_icon(m.kind)).font(icon_font(12.0)).color(if m.pinned { theme.accent } else { theme.text_faint }));
            ui.vertical(|ui| {
                match &m.proposal {
                    Some(Proposal::Update(new)) => {
                        ui.add(egui::Label::new(RichText::new(&m.text).font(ui_font(12.5)).color(theme.text_faint).strikethrough()).wrap());
                        ui.add(egui::Label::new(RichText::new(new).font(ui_font(13.0)).color(theme.text)).wrap());
                    }
                    _ => {
                        ui.add(egui::Label::new(RichText::new(&m.text).font(ui_font(13.0)).color(if archived { theme.text_dim } else { theme.text })).wrap());
                    }
                }
            });
        });

        // Evidence for inbox items: the words that prompted the suggestion.
        if review
            && let Some(q) = m.sources.last().and_then(|s| s.quote.as_deref()).map(str::trim).filter(|q| !q.is_empty()) {
                quote_line(ui, theme, q);
            }

        // Meta line.
        let mut meta = vec![m.kind.label().to_owned(), scope_label(m, project)];
        if archived {
            let (why, detail) = m.archived_reason();
            meta.push(format!("{why} {}", ago(m.updated)));
            let r = ui.label(RichText::new(meta.join("  ·  ")).font(ui_font(11.0)).color(theme.text_faint));
            if let Some(d) = detail {
                r.on_hover_text(d);
            }
        } else {
            if m.uses > 0 {
                meta.push(format!("used {}×", m.uses));
            }
            if m.pinned {
                meta.push("pinned".into());
            }
            if m.is_stale(now()) && !m.pinned {
                meta.push("not used lately".into());
            }
            meta.push(ago(m.updated));
            let text = meta.join("  ·  ");
            if review {
                ui.label(RichText::new(text).font(ui_font(11.0)).color(theme.text_faint));
            } else {
                // Actions live at the end of the meta row (space reserved, shown on hover) so
                // they never cover the memory itself.
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 76.0).max(60.0);
                    ui.allocate_ui_with_layout(vec2(w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(text).font(ui_font(11.0)).color(theme.text_faint)).truncate());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if !hovered {
                            ui.allocate_space(vec2(0.0, 22.0));
                            return;
                        }
                        let rule = m.kind == Kind::Rule;
                        if kit::icon_button_sized(ui, icon::DELETE, if rule { "Revoke" } else { "Delete" }, theme, 22.0, false).clicked() {
                            act = Some(CardAct::Delete);
                        }
                        if !rule && kit::icon_button_sized(ui, icon::EDIT, "Edit", theme, 22.0, false).clicked() {
                            act = Some(CardAct::Edit);
                        }
                        if !rule && kit::icon_button_sized(ui, icon::PIN, if m.pinned { "Unpin" } else { "Pin: always give agents this" }, theme, 22.0, m.pinned).clicked() {
                            act = Some(CardAct::Pin(!m.pinned));
                        }
                    });
                });
            }
        }

        // Provenance.
        if m.kind == Kind::Rule {
            // Rules say where they came from in their evidence ("You approved … 3 times here").
            if !review
                && let Some(q) = m.sources.last().and_then(|s| s.quote.as_deref()) {
                    ui.label(RichText::new(q).font(ui_font(11.0)).color(theme.text_faint));
                }
        } else if let Some(src) = m.sources.last() {
            let origin = match m.origin {
                Origin::User => "Added by you",
                Origin::Agent => "Saved by an agent",
                Origin::Learned => "Learned",
            };
            let from = src.title.as_deref().map(|t| format!("{origin} from “{}”", trunc(t, 48))).unwrap_or_else(|| origin.to_owned());
            let r = ui.add(egui::Label::new(RichText::new(from).font(ui_font(11.0)).color(theme.text_faint)).truncate());
            if let Some(q) = &src.quote {
                r.on_hover_text(format!("“{}”", trunc(q, 400)));
            }
        } else if m.origin == Origin::User {
            ui.label(RichText::new("Added by you").font(ui_font(11.0)).color(theme.text_faint));
        }

        // Actions.
        if archived {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if kit::mini_button_tinted(ui, "Restore", theme, theme.accent).clicked() {
                    act = Some(CardAct::Restore);
                }
                if kit::mini_button_tinted(ui, "Delete forever", theme, theme.err).on_hover_text("Learning may pick this up again later").clicked() {
                    act = Some(CardAct::Purge);
                }
            });
        } else if review {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (yes, no) = match (&m.proposal, m.kind) {
                    (Some(Proposal::Update(_)), _) => ("Accept", "Keep current"),
                    (Some(Proposal::Delete), _) => ("Remove", "Keep"),
                    (None, Kind::Rule) => ("Allow", "Not now"),
                    _ => ("Keep", "Dismiss"),
                };
                if kit::mini_button_tinted(ui, yes, theme, theme.accent).clicked() {
                    act = Some(CardAct::Accept);
                }
                if m.kind != Kind::Rule && kit::mini_button(ui, "Edit", theme).clicked() {
                    act = Some(CardAct::Edit);
                }
                if kit::mini_button(ui, no, theme).clicked() {
                    act = Some(CardAct::Dismiss);
                }
            });
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(hover_id, inner.response.rect));
    (inner.response, act)
}

fn draft_ui(ui: &mut Ui, theme: &Theme, d: &mut Draft, project: Option<&Project>, done: &mut Option<bool>) {
    Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.accent)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(if d.id.is_some() { "Edit memory" } else { "New memory" }).font(semibold(12.5)));
        let edit_id = Id::unique(("memory-draft", d.id));
        let resp = ui.add(TextEdit::multiline(&mut d.text).id(edit_id).hint_text("e.g. Use pnpm, not npm").desired_rows(2).desired_width(f32::INFINITY));
        if std::mem::take(&mut d.focus) {
            resp.request_focus();
        }
        ui.add_space(2.0);
        // Session summaries keep their kind and project; only their wording is editable.
        if Kind::EDITABLE.contains(&d.kind) {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for k in Kind::EDITABLE {
                    if kit::chip(ui, d.kind == k, k.label(), theme, false).clicked() {
                        d.kind = k;
                        // Preferences are about the person; facts and how-tos usually about a project.
                        if !d.scope_touched && project.is_some() {
                            d.global = k == Kind::Preference;
                        }
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                if kit::chip(ui, d.global, "About you (everywhere)", theme, false).clicked() {
                    d.global = true;
                    d.scope_touched = true;
                }
                let here = project.map(|p| format!("Only {}", p.name)).unwrap_or_else(|| "Only this project".into());
                if project.is_some() && kit::chip(ui, !d.global, &here, theme, false).clicked() {
                    d.global = false;
                    d.scope_touched = true;
                }
            });
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let can_save = d.text.trim().len() >= 4;
            ui.add_enabled_ui(can_save, |ui| {
                if kit::primary_button(ui, "Save", theme).clicked() {
                    *done = Some(true);
                }
            });
            if kit::ghost_button(ui, "Cancel", theme).clicked() {
                *done = Some(false);
            }
        });
    });
}

fn trunc(s: &str, n: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s }
}

/// A quoted line with a bar on the left (evidence on review cards).
fn quote_line(ui: &mut Ui, theme: &Theme, q: &str) {
    let q = q.strip_prefix("User input: ").unwrap_or(q).trim_matches('"');
    let resp = ui
        .horizontal(|ui| {
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(trunc(q, 180)).font(ui_font(11.5)).italics().color(theme.text_dim)).wrap());
        })
        .response;
    let r = resp.rect;
    ui.painter().rect_filled(egui::Rect::from_min_max(pos2(r.left() + 1.0, r.top() + 2.0), pos2(r.left() + 3.0, r.bottom() - 2.0)), CornerRadius::same(1), theme.border_strong);
}
