//! Interactive terminal widget: input routing, selection, scrolling, painting.

use std::sync::atomic::Ordering;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use eframe::egui::{
    self, Align2, CornerRadius, Event, EventFilter, Id, ImeEvent, Key, PointerButton, Pos2, Rect, Sense, Ui, pos2, vec2,
};

use super::render::{self, Metrics, PaintOpts};
use super::{TermPane, input};
use crate::theme::{Theme, with_alpha};
#[derive(Default)]
pub struct ViewState {
    pub cursor_anim: Option<Rect>,
    pub glyphs: render::GlyphCache,
    pub scroll_accum: f32,
    pub last_rect: Option<Rect>,
    pub stable_frames: u32,
    pub mouse_down: Option<u8>,
}

pub enum TermAction {
    Focus,
    Copy(String),
    OpenUrl(String),
    Close,
    Restart,
}

pub const PAD: egui::Vec2 = egui::Vec2::new(8.0, 6.0);

#[allow(clippy::too_many_arguments)]
pub fn show(ui: &mut Ui, pane: &mut TermPane, rect: Rect, id: Id, m: &Metrics, theme: &Theme, smooth_cursor: bool, dt: f32) -> Vec<TermAction> {
    let mut actions = Vec::new();
    let inner = rect.shrink2(PAD);
    let cols = (inner.width() / m.cell.x).floor().max(10.0) as usize;
    let rows = (inner.height() / m.cell.y).floor().max(3.0) as usize;

    // Debounce resizes so animated layout changes don't spam the PTY.
    if pane.view.last_rect == Some(rect) {
        pane.view.stable_frames += 1;
    } else {
        pane.view.stable_frames = 0;
        pane.view.last_rect = Some(rect);
    }
    if cols != pane.cols || rows != pane.rows {
        if pane.view.stable_frames >= 2 {
            pane.resize(cols, rows, m.cell.x, m.cell.y);
        } else {
            ui.ctx().request_repaint();
        }
    }

    let resp = ui.interact(rect, id, Sense::click_and_drag());
    if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
        actions.push(TermAction::Focus);
        ui.memory_mut(|mem| mem.request_focus(id));
    }
    let has_focus = ui.memory(|mem| mem.has_focus(id));
    if has_focus {
        ui.memory_mut(|mem| {
            mem.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
        });
    }

    let mode = *pane.term.lock().mode();
    let origin = inner.min;
    let display_offset = pane.display_offset();
    let (pane_cols, pane_rows) = (pane.cols, pane.rows);
    let cell_at = move |pos: Pos2| -> (usize, usize, Side) {
        let x = ((pos.x - origin.x) / m.cell.x).max(0.0);
        let y = ((pos.y - origin.y) / m.cell.y).max(0.0);
        let col = (x.floor() as usize).min(pane_cols.saturating_sub(1));
        let row = (y.floor() as usize).min(pane_rows.saturating_sub(1));
        let side = if x.fract() > 0.5 { Side::Right } else { Side::Left };
        (row, col, side)
    };
    let to_point = |row: usize, col: usize| Point::new(Line(row as i32 - display_offset as i32), Column(col));

    // ------------------------------------------------------------ keyboard
    if has_focus {
        let events = ui.input(|i| i.events.clone());
        let mut suppress_text = false;
        let mut wrote = false;
        for ev in events {
            if pane.exited.is_some() {
                if let Event::Key { key: Key::Enter, pressed: true, .. } = ev {
                    actions.push(TermAction::Restart);
                }
                continue;
            }
            match ev {
                Event::Text(s) => {
                    if std::mem::take(&mut suppress_text) {
                        continue;
                    }
                    pane.write(s.into_bytes());
                    wrote = true;
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if modifiers.shift && !modifiers.ctrl && matches!(key, Key::PageUp | Key::PageDown) {
                        let page = pane.rows.saturating_sub(2).max(1) as i32;
                        pane.scroll(if key == Key::PageUp { page } else { -page });
                        continue;
                    }
                    if let Some(bytes) = input::encode_key(key, modifiers, mode) {
                        pane.write(bytes);
                        wrote = true;
                        if input::suppresses_text(key, modifiers) {
                            suppress_text = true;
                        }
                    }
                }
                Event::Paste(s) => {
                    pane.paste(&s);
                    wrote = true;
                }
                Event::Copy => {
                    if let Some(sel) = pane.selection_text() {
                        actions.push(TermAction::Copy(sel));
                        pane.clear_selection();
                    } else {
                        pane.write(vec![0x03]);
                        wrote = true;
                    }
                }
                Event::Cut => {
                    pane.write(vec![0x18]);
                    wrote = true;
                }
                Event::Ime(ImeEvent::Commit(s)) => {
                    pane.write(s.into_bytes());
                    wrote = true;
                }
                _ => {}
            }
        }
        if wrote {
            pane.scroll_to_bottom();
            pane.clear_selection();
        }
    }

    // ------------------------------------------------------------ mouse
    let shift = ui.input(|i| i.modifiers.shift);
    let mouse_mode = mode.intersects(TermMode::MOUSE_MODE) && !shift;

    if resp.hovered() {
        let (dy, mods) = ui.input(|i| {
            let mut lines = 0.0f32;
            for ev in &i.events {
                if let Event::MouseWheel { unit, delta, .. } = ev {
                    lines += match unit {
                        egui::MouseWheelUnit::Line => delta.y,
                        egui::MouseWheelUnit::Page => delta.y * pane.rows as f32,
                        egui::MouseWheelUnit::Point => delta.y / m.cell.y,
                    };
                }
            }
            (lines, i.modifiers)
        });
        if dy != 0.0 && !mods.ctrl {
            pane.view.scroll_accum += dy;
            let lines = pane.view.scroll_accum.trunc() as i32;
            if lines != 0 {
                pane.view.scroll_accum -= lines as f32;
                if mouse_mode {
                    if let Some(pos) = resp.hover_pos() {
                        let (row, col, _) = cell_at(pos);
                        let button = if lines > 0 { 64 } else { 65 };
                        for _ in 0..lines.unsigned_abs().min(8) {
                            if let Some(b) = input::mouse_report(button, col, row, true, mods, mode) {
                                pane.write(b);
                            }
                        }
                    }
                } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                    let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    for _ in 0..lines.unsigned_abs() {
                        pane.write(seq.to_vec());
                    }
                } else {
                    pane.scroll(lines * 3);
                }
            }
        }
    }

    if mouse_mode {
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            match ev {
                Event::PointerButton { pos, button, pressed, modifiers } if inner.contains(pos) || (!pressed && pane.view.mouse_down.is_some()) => {
                    let b = match button {
                        PointerButton::Primary => 0,
                        PointerButton::Middle => 1,
                        PointerButton::Secondary => 2,
                        _ => continue,
                    };
                    let (row, col, _) = cell_at(pos);
                    pane.view.mouse_down = if pressed { Some(b) } else { None };
                    if let Some(bytes) = input::mouse_report(b, col, row, pressed, modifiers, mode) {
                        pane.write(bytes);
                    }
                }
                Event::PointerMoved(pos) if inner.contains(pos) => {
                    let motion = mode.contains(TermMode::MOUSE_MOTION) || (mode.contains(TermMode::MOUSE_DRAG) && pane.view.mouse_down.is_some());
                    if motion {
                        let (row, col, _) = cell_at(pos);
                        let b = pane.view.mouse_down.unwrap_or(3) + 32;
                        if let Some(bytes) = input::mouse_report(b, col, row, true, egui::Modifiers::NONE, mode) {
                            pane.write(bytes);
                        }
                    }
                }
                _ => {}
            }
        }
    } else if let Some(pos) = resp.interact_pointer_pos() {
        let (row, col, side) = cell_at(pos);
        let point = to_point(row, col);
        if resp.triple_clicked() {
            set_selection(pane, Selection::new(SelectionType::Lines, point, side));
        } else if resp.double_clicked() {
            set_selection(pane, Selection::new(SelectionType::Semantic, point, side));
        } else if resp.drag_started_by(PointerButton::Primary) {
            let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let (r0, c0, s0) = cell_at(press);
            set_selection(pane, Selection::new(SelectionType::Simple, to_point(r0, c0), s0));
        } else if resp.clicked() && !ui.input(|i| i.modifiers.ctrl) {
            pane.clear_selection();
        }
        if resp.dragged_by(PointerButton::Primary) {
            if pos.y < inner.top() {
                pane.scroll(1);
            } else if pos.y > inner.bottom() {
                pane.scroll(-1);
            }
            let mut term = pane.term.lock();
            if let Some(sel) = term.selection.as_mut() {
                sel.update(point, side);
            }
            drop(term);
            pane.dirty.store(true, Ordering::Relaxed);
        }
        if resp.clicked() && ui.input(|i| i.modifiers.ctrl)
            && let Some(url) = link_at(pane, point) {
                actions.push(TermAction::OpenUrl(url));
            }
    }

    if ui.input(|i| i.modifiers.ctrl) && resp.hovered()
        && let Some(pos) = resp.hover_pos() {
            let (row, col, _) = cell_at(pos);
            if link_at(pane, to_point(row, col)).is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }

    resp.context_menu(|ui| {
        ui.set_min_width(180.0);
        if ui.button("Copy").clicked() {
            if let Some(sel) = pane.selection_text() {
                actions.push(TermAction::Copy(sel));
            }
            ui.close();
        }
        if ui.button("Paste").clicked() {
            if let Ok(mut cb) = arboard::Clipboard::new()
                && let Ok(text) = cb.get_text() {
                    pane.paste(&text);
                }
            ui.close();
        }
        if ui.button("Select all").clicked() {
            pane.select_all();
            ui.close();
        }
        if ui.button("Clear").clicked() {
            pane.clear_scrollback();
            ui.close();
        }
        ui.separator();
        if ui.button("Restart shell").clicked() {
            actions.push(TermAction::Restart);
            ui.close();
        }
        if ui.button("Close").clicked() {
            actions.push(TermAction::Close);
            ui.close();
        }
    });

    // ------------------------------------------------------------ paint
    let palette = pane.palette();
    if pane.dirty.swap(false, Ordering::Relaxed) || !pane.snapshot.valid {
        let term_arc = pane.term.clone();
        let term = term_arc.lock();
        render::snapshot(&term, &palette, &mut pane.snapshot);
    }
    let painter = ui.painter_at(rect);
    let opts = PaintOpts { theme, palette, focused: has_focus, smooth_cursor, dt };
    render::paint(ui.ctx(), &painter, origin, &pane.snapshot, m, &mut pane.view.glyphs, &mut pane.view.cursor_anim, &opts);

    if pane.snapshot.display_offset > 0 {
        let label = format!("{} lines up  ·  jump to bottom", pane.snapshot.display_offset);
        let w = crate::kit::pill_width(&painter, &label, crate::theme::ui(11.5));
        let chip = crate::kit::pill(&painter, pos2(rect.right() - w - 12.0, rect.bottom() - 18.0), &label, crate::theme::ui(11.5), theme.on_accent, theme.accent);
        let r = ui.interact(chip, id.with("sb"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            pane.scroll_to_bottom();
        }
    }

    if let Some(code) = pane.exited {
        painter.rect_filled(inner, CornerRadius::same(4), with_alpha(theme.surface, 0.85));
        let c = inner.center();
        painter.text(c - vec2(0.0, 12.0), Align2::CENTER_CENTER, "Process exited", crate::theme::semibold(15.0), theme.text);
        painter.text(c + vec2(0.0, 12.0), Align2::CENTER_CENTER, format!("Exit code {code}  ·  press Enter to restart"), crate::theme::ui(12.5), theme.text_dim);
    }
    actions
}

fn set_selection(pane: &mut TermPane, sel: Selection) {
    pane.term.lock().selection = Some(sel);
    pane.dirty.store(true, Ordering::Relaxed);
}

/// OSC 8 hyperlink at a point, or a plain-text URL under it.
fn link_at(pane: &TermPane, point: Point) -> Option<String> {
    {
        let term = pane.term.lock();
        if point.line < term.grid().topmost_line() || point.line > term.grid().bottommost_line() || point.column.0 >= term.grid().columns() {
            return None;
        }
        if let Some(link) = term.grid()[point].hyperlink() {
            return Some(link.uri().to_string());
        }
    }
    let line = pane.line_text(point.line);
    let chars: Vec<char> = line.chars().collect();
    let col = point.column.0.min(chars.len());
    let is_url_char = |c: char| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>' | '`' | '│' | '|');
    let mut start = col;
    while start > 0 && is_url_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < chars.len() && is_url_char(chars[end]) {
        end += 1;
    }
    let word: String = chars[start..end].iter().collect();
    let idx = word.find("https://").or_else(|| word.find("http://"))?;
    let url = word[idx..].trim_end_matches(['.', ',', ')', ']', ';', ':']).to_string();
    (url.len() > 8).then_some(url)
}
