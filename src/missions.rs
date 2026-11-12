//! Mission Board: a scratchpad of ideas you can delegate to Copilot agents.

use std::path::PathBuf;

use eframe::egui::{self, Align, CornerRadius, CursorIcon, Frame, Id, Key, Layout, Margin, RichText, Sense, Stroke, TextEdit, Ui};
use serde::{Deserialize, Serialize};

use crate::agent::BayId;
use crate::kit::{self, icon};
use crate::settings::{config_dir, read_json, write_json};
use crate::theme::{Theme, icon_font, semibold, ui as ui_font};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Idea,
    Active,
    Review,
    Done,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Idea => "Idea",
            Status::Active => "In progress",
            Status::Review => "Ready for review",
            Status::Done => "Done",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mission {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub dir: Option<String>,
    pub status: Status,
    pub created: i64,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub from_agent: bool,
    #[serde(skip)]
    pub bay: Option<BayId>,
}

impl Mission {
    pub fn brief(&self) -> String {
        if self.notes.trim().is_empty() { self.title.clone() } else { format!("{}\n\n{}", self.title, self.notes) }
    }
}

/// Drag payload for delegating a mission onto a bay.
#[derive(Clone, Copy)]
pub struct MissionDrag(pub u64);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Open,
    Active,
    Done,
}

pub enum BoardAction {
    Launch(u64),
    SendToFocused(u64),
    FocusBay(BayId),
    PickDir(u64),
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    missions: Vec<Mission>,
    next_id: u64,
}

pub struct MissionBoard {
    pub missions: Vec<Mission>,
    next_id: u64,
    dirty: bool,
    capture: String,
    editing: Option<(u64, String, String)>,
    filter: Filter,
}

fn path() -> PathBuf {
    config_dir().join("missions.json")
}

impl MissionBoard {
    pub fn load() -> Self {
        let stored: Stored = read_json(&path()).unwrap_or_default();
        let next_id = stored.next_id.max(stored.missions.iter().map(|m| m.id + 1).max().unwrap_or(1));
        MissionBoard { missions: stored.missions, next_id, dirty: false, capture: String::new(), editing: None, filter: Filter::Open }
    }

    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            let stored = Stored { missions: self.missions.clone(), next_id: self.next_id };
            write_json(&path(), &stored);
            self.dirty = false;
        }
    }

    pub fn add(&mut self, title: &str, notes: &str, from_agent: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.missions.insert(0, Mission {
            id,
            title: title.trim().to_owned(),
            notes: notes.trim().to_owned(),
            dir: None,
            status: Status::Idea,
            created: chrono::Utc::now().timestamp(),
            session_id: None,
            from_agent,
            bay: None,
        });
        self.dirty = true;
        id
    }

    pub fn get(&self, id: u64) -> Option<&Mission> {
        self.missions.iter().find(|m| m.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Mission> {
        self.dirty = true;
        self.missions.iter_mut().find(|m| m.id == id)
    }

    pub fn set_dir(&mut self, id: u64, dir: PathBuf) {
        if let Some(m) = self.get_mut(id) {
            m.dir = Some(dir.display().to_string());
        }
    }

    /// Link a mission to an agent bay and mark it in progress.
    pub fn link(&mut self, id: u64, bay: BayId) {
        if let Some(m) = self.get_mut(id) {
            m.bay = Some(bay);
            m.status = Status::Active;
        }
    }

    pub fn on_session(&mut self, bay: BayId, session_id: &str) {
        for m in self.missions.iter_mut().filter(|m| m.bay == Some(bay)) {
            m.session_id = Some(session_id.to_owned());
            self.dirty = true;
        }
    }

    /// The linked agent finished a turn.
    pub fn on_agent_finished(&mut self, bay: BayId) {
        for m in self.missions.iter_mut().filter(|m| m.bay == Some(bay) && m.status == Status::Active) {
            m.status = Status::Review;
            self.dirty = true;
        }
    }

    pub fn on_bay_closed(&mut self, bay: BayId) {
        for m in self.missions.iter_mut().filter(|m| m.bay == Some(bay)) {
            m.bay = None;
        }
    }

    pub fn open_count(&self) -> usize {
        self.missions.iter().filter(|m| m.status != Status::Done).count()
    }

    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, agent_status: &dyn Fn(BayId) -> Option<(String, egui::Color32)>) -> Vec<BoardAction> {
        let mut actions = Vec::new();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Missions").font(semibold(15.0)));
            ui.label(RichText::new(self.open_count().to_string()).font(ui_font(12.0)).color(theme.text_faint));
        });
        ui.label(RichText::new("Capture ideas, then launch them as Copilot agents or drag one onto a running agent.").font(ui_font(12.0)).color(theme.text_dim));
        ui.add_space(6.0);

        let capture_id = Id::unique("mission-capture");
        let mut submit = false;
        let mut refocus = false;
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(80.0);
            let resp = ui.add(TextEdit::singleline(&mut self.capture).id(capture_id).hint_text("Capture an idea…  (Enter)").desired_width(w).margin(Margin::symmetric(8, 6)));
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
                refocus = true;
            }
            if kit::icon_button_sized(ui, icon::ADD, "Add mission", theme, 28.0, false).clicked() {
                submit = true;
            }
        });
        if submit && !self.capture.trim().is_empty() {
            let text = std::mem::take(&mut self.capture);
            let (title, notes) = match text.split_once(" -- ") {
                Some((t, n)) => (t.to_owned(), n.to_owned()),
                None => (text, String::new()),
            };
            self.add(&title, &notes, false);
            if refocus {
                ui.memory_mut(|m| m.request_focus(capture_id));
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (f, label) in [(Filter::Open, "Open"), (Filter::Active, "Active"), (Filter::Done, "Done")] {
                if ui.selectable_label(self.filter == f, label).clicked() {
                    self.filter = f;
                }
            }
        });
        ui.add_space(4.0);

        let mut delete = None;
        egui::ScrollArea::vertical().id_salt("missions-scroll").auto_shrink([false; 2]).show(ui, |ui| {
            let visible: Vec<usize> = self
                .missions
                .iter()
                .enumerate()
                .filter(|(_, m)| match self.filter {
                    Filter::Open => m.status != Status::Done,
                    Filter::Active => matches!(m.status, Status::Active | Status::Review),
                    Filter::Done => m.status == Status::Done,
                })
                .map(|(i, _)| i)
                .collect();
            if visible.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icon::LIGHTBULB).font(icon_font(22.0)).color(theme.text_faint));
                    ui.label(RichText::new("Nothing here yet").color(theme.text_dim));
                });
            }
            for i in visible {
                let editing = self.editing.as_ref().is_some_and(|(id, _, _)| *id == self.missions[i].id);
                if editing {
                    self.edit_card(ui, i, theme, &mut actions);
                } else if let Some(a) = self.card(ui, i, theme, agent_status) {
                    match a {
                        CardAction::Delete => delete = Some(i),
                        CardAction::Edit => {
                            let m = &self.missions[i];
                            self.editing = Some((m.id, m.title.clone(), m.notes.clone()));
                        }
                        CardAction::Done => {
                            self.missions[i].status = if self.missions[i].status == Status::Done { Status::Idea } else { Status::Done };
                            self.dirty = true;
                        }
                        CardAction::Board(b) => actions.push(b),
                    }
                }
                ui.add_space(6.0);
            }
        });
        if let Some(i) = delete {
            self.missions.remove(i);
            self.dirty = true;
        }
        actions
    }

    fn card(&mut self, ui: &mut Ui, i: usize, theme: &Theme, agent_status: &dyn Fn(BayId) -> Option<(String, egui::Color32)>) -> Option<CardAction> {
        let m = self.missions[i].clone();
        let mut action = None;
        // The card is its own drag handle. Its sense sits *below* the card's buttons (unlike
        // `dnd_drag_source`, which covers them), so the buttons stay clickable and a drag only
        // starts once the pointer moves.
        let inner = ui.scope_builder(egui::UiBuilder::new().id_salt(("mission-card", m.id)).sense(Sense::click_and_drag()), |ui| {
            Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let (color, done) = match m.status {
                        Status::Idea => (theme.text_faint, false),
                        Status::Active => (theme.accent, false),
                        Status::Review => (theme.ok, false),
                        Status::Done => (theme.text_faint, true),
                    };
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), Sense::hover());
                    kit::dot(ui.painter(), r.center(), 3.5, color);
                    let title = if done { RichText::new(&m.title).strikethrough().color(theme.text_faint) } else { RichText::new(&m.title).color(theme.text) };
                    ui.add(egui::Label::new(title.font(semibold(13.0))).wrap());
                });
                if !m.notes.is_empty() {
                    let preview: String = m.notes.lines().take(2).collect::<Vec<_>>().join(" ");
                    ui.add(egui::Label::new(RichText::new(preview).font(ui_font(12.0)).color(theme.text_dim)).truncate());
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let live = m.bay.and_then(agent_status);
                    let (status_text, color) = live.unwrap_or_else(|| (m.status.label().to_owned(), theme.text_faint));
                    ui.label(RichText::new(status_text).font(ui_font(11.5)).color(color));
                    if let Some(d) = &m.dir {
                        ui.label(RichText::new(format!("{}  {}", icon::FOLDER, crate::settings::dir_name(std::path::Path::new(d)))).font(ui_font(11.5)).color(theme.text_faint));
                    }
                    if m.from_agent {
                        ui.label(RichText::new(format!("{} from agent", icon::BOT)).font(ui_font(11.5)).color(theme.text_faint));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if kit::icon_button_sized(ui, icon::DELETE, "Delete", theme, 22.0, false).clicked() {
                            action = Some(CardAction::Delete);
                        }
                        if kit::icon_button_sized(ui, icon::CHECK, if m.status == Status::Done { "Reopen" } else { "Mark done" }, theme, 22.0, false).clicked() {
                            action = Some(CardAction::Done);
                        }
                        if kit::icon_button_sized(ui, icon::EDIT, "Edit", theme, 22.0, false).clicked() {
                            action = Some(CardAction::Edit);
                        }
                        if let Some(bay) = m.bay
                            && kit::icon_button_sized(ui, icon::CHAT, "Go to agent", theme, 22.0, false).clicked() {
                                action = Some(CardAction::Board(BoardAction::FocusBay(bay)));
                            }
                        if kit::icon_button_sized(ui, icon::SEND, "Send to the focused agent", theme, 22.0, false).clicked() {
                            action = Some(CardAction::Board(BoardAction::SendToFocused(m.id)));
                        }
                        if kit::icon_button_sized(ui, icon::PLAY, "Launch a new agent for this", theme, 22.0, true).clicked() {
                            action = Some(CardAction::Board(BoardAction::Launch(m.id)));
                        }
                    });
                });
            });
        });
        let resp = inner.response;
        resp.dnd_set_drag_payload(MissionDrag(m.id));
        if resp.dragged() {
            // A small ghost of the mission follows the pointer to the agent it's dropped on.
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique(("mission-ghost", m.id))));
                let g = kit::elided(&painter, &m.title, semibold(12.5), theme.text, 260.0);
                let r = egui::Rect::from_min_size(p + egui::vec2(14.0, 10.0), g.size() + egui::vec2(22.0, 14.0));
                painter.rect(r, CornerRadius::same(8), theme.surface3, Stroke::new(1.0, theme.accent), egui::StrokeKind::Inside);
                painter.galley(r.min + egui::vec2(11.0, 7.0), g, theme.text);
            }
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if resp.double_clicked() {
            action = Some(CardAction::Edit);
        }
        action
    }

    fn edit_card(&mut self, ui: &mut Ui, i: usize, theme: &Theme, actions: &mut Vec<BoardAction>) {
        let Some((id, title, notes)) = self.editing.as_mut() else { return };
        let id = *id;
        let mut save = false;
        let mut cancel = false;
        Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.accent)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(TextEdit::singleline(title).hint_text("Title").desired_width(f32::INFINITY).font(semibold(13.0)));
            ui.add(TextEdit::multiline(notes).hint_text("Notes, context, acceptance criteria…").desired_rows(4).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                let dir = self.missions[i].dir.clone().unwrap_or_else(|| "Default folder".into());
                ui.label(RichText::new(format!("{}  {}", icon::FOLDER, dir)).font(ui_font(11.5)).color(theme.text_dim));
                if kit::mini_button(ui, "Change…", theme).clicked() {
                    actions.push(BoardAction::PickDir(id));
                }
            });
            ui.horizontal(|ui| {
                if kit::primary_button(ui, "Save", theme).clicked() {
                    save = true;
                }
                if kit::ghost_button(ui, "Cancel", theme).clicked() {
                    cancel = true;
                }
            });
        });
        if save {
            if let Some((_, t, n)) = self.editing.take() {
                let m = &mut self.missions[i];
                m.title = t;
                m.notes = n;
                self.dirty = true;
            }
        } else if cancel {
            self.editing = None;
        }
    }
}

enum CardAction {
    Delete,
    Edit,
    Done,
    Board(BoardAction),
}
