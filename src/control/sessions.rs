//! Sessions sidebar, laid out the way chat apps list conversations: what's open now, pinned
//! sessions, then everything else grouped by date. Filter by title, rename inline, pin, open the
//! session's folder or delete it.

use chrono::{DateTime, Local};
use eframe::egui::{self, Align2, Color32, CornerRadius, CursorIcon, Id, Margin, Rect, RichText, Sense, TextEdit, Ui, pos2, vec2};

use crate::kit::{self, icon};
use crate::theme::{Theme, icon_font, semibold, ui as ui_font};

/// An agent bay that is open right now.
pub struct OpenRow {
    pub index: usize,
    pub title: String,
    pub status: String,
    pub color: Color32,
    pub busy: bool,
}

/// A past session (from Copilot's session list, named by Graysense where it knows better).
pub struct PastRow {
    pub id: String,
    pub title: String,
    pub modified: Option<DateTime<Local>>,
    /// Short "when" label ("4m", "Yesterday", "Nov 10").
    pub when: String,
    pub pinned: bool,
    /// Started outside Graysense (e.g. the Copilot CLI).
    pub cli: bool,
}

pub enum SessionsAction {
    New,
    Focus(usize),
    Resume(String),
    Rename(String, String),
    Pin(String, bool),
    Reveal(String),
    Delete(String),
    /// Search inside sessions and their files.
    Search(String),
}

#[derive(Default)]
pub struct SessionsPanel {
    filter: String,
    /// Session being renamed inline, with the draft name.
    renaming: Option<(String, String)>,
    /// Session waiting for delete confirmation.
    confirm_delete: Option<String>,
}

enum Entry<'a> {
    Header(String),
    Open(&'a OpenRow),
    Past(&'a PastRow),
    SearchAll,
}

const ROW_H: f32 = 30.0;
const HEADER_H: f32 = 30.0;

fn group(t: Option<DateTime<Local>>, now: DateTime<Local>) -> String {
    let Some(t) = t else { return "Older".into() };
    match (now.date_naive() - t.date_naive()).num_days() {
        ..=0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => "Previous 7 days".into(),
        7..=29 => "Previous 30 days".into(),
        _ => t.format("%B %Y").to_string(),
    }
}

impl SessionsPanel {
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, open: &[OpenRow], past: &[PastRow], t: f32) -> Vec<SessionsAction> {
        let mut out = Vec::new();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sessions").font(semibold(15.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::icon_button_sized(ui, icon::ADD, "New session (Ctrl+Shift+N)", theme, 26.0, false).clicked() {
                    out.push(SessionsAction::New);
                }
            });
        });
        ui.add_space(4.0);
        let fresp = ui.add(TextEdit::singleline(&mut self.filter).hint_text(format!("{}  Filter by name", icon::SEARCH)).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6)));
        if fresp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.filter.trim().is_empty() {
            out.push(SessionsAction::Search(self.filter.trim().to_owned()));
        }
        ui.add_space(6.0);

        // Flatten into fixed-height entries so only the visible ones are laid out.
        let q = self.filter.trim().to_lowercase();
        let mut entries: Vec<Entry> = Vec::new();
        if q.is_empty() {
            if !open.is_empty() {
                entries.push(Entry::Header("Open".into()));
                entries.extend(open.iter().map(Entry::Open));
            }
            let pinned: Vec<&PastRow> = past.iter().filter(|p| p.pinned).collect();
            if !pinned.is_empty() {
                entries.push(Entry::Header("Pinned".into()));
                entries.extend(pinned.into_iter().map(Entry::Past));
            }
            let now = Local::now();
            let mut last = String::new();
            for p in past.iter().filter(|p| !p.pinned) {
                let g = group(p.modified, now);
                if g != last {
                    entries.push(Entry::Header(g.clone()));
                    last = g;
                }
                entries.push(Entry::Past(p));
            }
        } else {
            entries.extend(open.iter().filter(|o| o.title.to_lowercase().contains(&q)).map(Entry::Open));
            entries.extend(past.iter().filter(|p| p.title.to_lowercase().contains(&q)).map(Entry::Past));
            entries.push(Entry::SearchAll);
        }
        if entries.is_empty() {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::HISTORY).font(icon_font(22.0)).color(theme.text_faint));
                ui.label(RichText::new("No sessions yet").font(semibold(13.0)).color(theme.text_dim));
                ui.label(RichText::new("Start one with Ctrl+Shift+N.").font(ui_font(12.0)).color(theme.text_faint));
            });
            return out;
        }

        let heights: Vec<f32> = entries.iter().map(|e| if matches!(e, Entry::Header(_)) { HEADER_H } else { ROW_H }).collect();
        let total: f32 = heights.iter().sum();
        egui::ScrollArea::vertical().id_salt("sessions-list").auto_shrink([false; 2]).show_viewport(ui, |ui, viewport| {
            ui.set_height(total);
            let origin = ui.max_rect().min;
            let width = ui.available_width();
            let mut y = 0.0;
            for (e, h) in entries.iter().zip(&heights) {
                let top = y;
                y += h;
                if y < viewport.min.y || top > viewport.max.y {
                    continue;
                }
                let rect = Rect::from_min_size(origin + vec2(0.0, top), vec2(width, *h));
                match e {
                    Entry::Header(text) => {
                        ui.painter().text(pos2(rect.left() + 8.0, rect.bottom() - 9.0), Align2::LEFT_BOTTOM, text.to_uppercase(), semibold(10.5), theme.text_faint);
                    }
                    Entry::Open(o) => self.open_row(ui, theme, rect, o, t, &mut out),
                    Entry::Past(p) => self.past_row(ui, theme, rect, p, &mut out),
                    Entry::SearchAll => {
                        let resp = ui.interact(rect, Id::unique("sessions-search-all"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Search inside sessions"));
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(6), theme.surface2);
                        }
                        ui.painter().text(pos2(rect.left() + 12.0, rect.center().y), Align2::CENTER_CENTER, icon::SEARCH, icon_font(11.0), theme.accent);
                        let g = kit::elided(ui.painter(), &format!("Search inside sessions for “{}”", self.filter.trim()), ui_font(12.5), theme.accent, width - 40.0);
                        ui.painter().galley(pos2(rect.left() + 26.0, rect.center().y - g.size().y / 2.0), g, theme.accent);
                        if resp.clicked() {
                            out.push(SessionsAction::Search(self.filter.trim().to_owned()));
                        }
                    }
                }
            }
        });
        out
    }

    fn open_row(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, o: &OpenRow, t: f32, out: &mut Vec<SessionsAction>) {
        let resp = ui.interact(rect, Id::unique(("open-session", o.index)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} ({})", o.title, o.status)));
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, CornerRadius::same(6), theme.surface2);
        }
        let dot = pos2(rect.left() + 12.0, rect.center().y);
        if o.busy {
            kit::spinner(p, dot, 4.5, t, o.color);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        } else {
            kit::dot(p, dot, 3.5, o.color);
        }
        let sg = p.layout_no_wrap(o.status.clone(), ui_font(11.5), theme.text_faint);
        let sw = sg.size().x.min(rect.width() * 0.4);
        let tg = kit::elided(p, &o.title, ui_font(12.5), theme.text, rect.width() - sw - 44.0);
        p.galley(pos2(rect.left() + 26.0, rect.center().y - tg.size().y / 2.0), tg, theme.text);
        let sg = kit::elided(p, &o.status, ui_font(11.5), theme.text_faint, sw);
        p.galley(pos2(rect.right() - 8.0 - sg.size().x, rect.center().y - sg.size().y / 2.0), sg, theme.text_faint);
        if resp.clicked() {
            out.push(SessionsAction::Focus(o.index));
        }
    }

    fn past_row(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, row: &PastRow, out: &mut Vec<SessionsAction>) {
        // Inline rename.
        if let Some((id, draft)) = &mut self.renaming
            && *id == row.id
        {
            let mut done = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(2.0, 2.0))), |ui| {
                let r = ui.add(TextEdit::singleline(draft).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 4)));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if r.lost_focus() {
                    done = Some(ui.input(|i| i.key_pressed(egui::Key::Enter)));
                }
            });
            match done {
                Some(true) => {
                    out.push(SessionsAction::Rename(row.id.clone(), draft.trim().to_owned()));
                    self.renaming = None;
                }
                Some(false) => self.renaming = None,
                None => {}
            }
            return;
        }
        // Delete confirmation replaces the row.
        if self.confirm_delete.as_deref() == Some(row.id.as_str()) {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(4.0, 3.0))).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                ui.label(RichText::new("Delete?").font(semibold(12.0)).color(theme.err));
                if kit::mini_button_tinted(ui, "Delete", theme, theme.err).on_hover_text("Removes the conversation. Files in its folder stay.").clicked() {
                    out.push(SessionsAction::Delete(row.id.clone()));
                    self.confirm_delete = None;
                }
                if kit::mini_button(ui, "Cancel", theme).clicked() {
                    self.confirm_delete = None;
                }
            });
            return;
        }

        let resp = ui.interact(rect, Id::unique(("past-session", &row.id)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Resume {}", row.title)));
        let menu_rect = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(24.0, 22.0));
        let menu_id = Id::unique(("past-session-menu", &row.id));
        let menu_open = egui::Popup::is_id_open(ui.ctx(), menu_id.with("popup"));
        let hovered = resp.hovered() || menu_open || ui.rect_contains_pointer(menu_rect);
        let p = ui.painter();
        if hovered {
            p.rect_filled(rect, CornerRadius::same(6), theme.surface2);
        }
        let right_w = if hovered { 30.0 } else { p.layout_no_wrap(row.when.clone(), ui_font(11.0), theme.text_faint).size().x + 12.0 + if row.pinned { 16.0 } else { 0.0 } + if row.cli { 28.0 } else { 0.0 } };
        let tg = kit::elided(p, &row.title, ui_font(12.5), if hovered { theme.text } else { theme.text_dim }, rect.width() - right_w - 18.0);
        p.galley(pos2(rect.left() + 10.0, rect.center().y - tg.size().y / 2.0), tg, theme.text);
        if hovered {
            let mresp = ui.interact(menu_rect, menu_id, Sense::click()).on_hover_text("More");
            mresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("More actions for {}", row.title)));
            kit::paint_icon_button(ui, menu_rect, &mresp, icon::MORE, theme, menu_open);
            egui::Popup::menu(&mresp).width(180.0).show(|ui| {
                if ui.button(format!("{}   Rename", icon::EDIT)).clicked() {
                    self.renaming = Some((row.id.clone(), row.title.clone()));
                    ui.close();
                }
                if ui.button(format!("{}   {}", icon::PIN, if row.pinned { "Unpin" } else { "Pin" })).clicked() {
                    out.push(SessionsAction::Pin(row.id.clone(), !row.pinned));
                    ui.close();
                }
                if ui.button(format!("{}   Open folder", icon::FOLDER)).clicked() {
                    out.push(SessionsAction::Reveal(row.id.clone()));
                    ui.close();
                }
                if ui.button(RichText::new(format!("{}   Delete…", icon::DELETE)).color(theme.err)).clicked() {
                    self.confirm_delete = Some(row.id.clone());
                    ui.close();
                }
            });
        } else {
            let mut x = rect.right() - 8.0;
            let wr = p.text(pos2(x, rect.center().y), Align2::RIGHT_CENTER, &row.when, ui_font(11.0), theme.text_faint);
            x = wr.left() - 6.0;
            if row.pinned {
                let r = p.text(pos2(x, rect.center().y), Align2::RIGHT_CENTER, icon::PIN, icon_font(10.0), theme.accent);
                x = r.left() - 6.0;
            }
            if row.cli {
                p.text(pos2(x, rect.center().y), Align2::RIGHT_CENTER, "CLI", semibold(9.5), theme.text_faint);
            }
        }
        if resp.clicked() {
            out.push(SessionsAction::Resume(row.id.clone()));
        }
    }
}
