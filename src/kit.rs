//! Graysense UI kit: flat, modern primitives shared across the app.


use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::theme::{Theme, icon_font, mix, semibold, ui as ui_font, with_alpha};

/// Segoe Fluent Icons code points.
pub mod icon {
    pub const MINIMIZE: &str = "\u{E921}";
    pub const MAXIMIZE: &str = "\u{E922}";
    pub const RESTORE: &str = "\u{E923}";
    pub const CLOSE: &str = "\u{E8BB}";
    pub const ADD: &str = "\u{E710}";
    pub const CANCEL: &str = "\u{E711}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const SEARCH: &str = "\u{E721}";
    pub const SEND: &str = "\u{E724}";
    pub const PLAY: &str = "\u{E768}";
    pub const STOP: &str = "\u{E71A}";
    pub const LIGHTBULB: &str = "\u{EA80}";
    pub const BOT: &str = "\u{E99A}";
    pub const CHAT: &str = "\u{E8BD}";
    pub const TERMINAL: &str = "\u{E756}";
    pub const PUZZLE: &str = "\u{EA86}";
    pub const CONNECT: &str = "\u{E703}";
    pub const FOLDER: &str = "\u{E8B7}";
    pub const OPEN_FOLDER: &str = "\u{E838}";
    pub const COPY: &str = "\u{E8C8}";
    pub const DELETE: &str = "\u{E74D}";
    pub const EDIT: &str = "\u{E70F}";
    pub const REFRESH: &str = "\u{E72C}";
    pub const LIGHTNING: &str = "\u{E945}";
    pub const SHIELD: &str = "\u{EA18}";
    pub const CHECK: &str = "\u{E73E}";
    pub const ERROR: &str = "\u{E783}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const INFO: &str = "\u{E946}";
    pub const GLOBE: &str = "\u{E774}";
    pub const FULLSCREEN: &str = "\u{E740}";
    pub const BACK_TO_WINDOW: &str = "\u{E73F}";
    pub const CHEVRON_DOWN: &str = "\u{E70D}";
    pub const CHEVRON_RIGHT: &str = "\u{E76C}";
    pub const CHEVRON_UP: &str = "\u{E70E}";
    pub const DOCUMENT: &str = "\u{E8A5}";
    pub const HISTORY: &str = "\u{E81C}";
    pub const DOCK: &str = "\u{E90C}";
    pub const TILES: &str = "\u{ECA5}";
    pub const BRAIN: &str = "\u{E7BE}";
    pub const BAR_CHART: &str = "\u{E9D2}";
    pub const TABLE: &str = "\u{F0E5}";
    pub const CHECKLIST: &str = "\u{E9D5}";
    pub const ATTACH: &str = "\u{E723}";
    pub const MEMORY: &str = "\u{E82D}";
    pub const HEART: &str = "\u{EB51}";
    pub const PIN: &str = "\u{E718}";
    pub const HIDE: &str = "\u{ED1A}";
    pub const REMOVE: &str = "\u{E738}";
    pub const BACK: &str = "\u{E72B}";
    pub const SPARKLE: &str = "\u{F4A5}";
    pub const CODE: &str = "\u{E943}";
    pub const MORE: &str = "\u{E712}";
    pub const IMAGE: &str = "\u{E8B9}";
}

/// Flat surface with a 1px border.
pub fn panel(painter: &Painter, rect: Rect, theme: &Theme, radius: u8, border: Color32) {
    painter.rect(rect, CornerRadius::same(radius), theme.surface, Stroke::new(1.0, border), StrokeKind::Inside);
}

/// Square, borderless icon button with a subtle hover fill.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str, theme: &Theme) -> Response {
    icon_button_sized(ui, glyph, tooltip, theme, 26.0, false)
}

pub fn icon_button_sized(ui: &mut Ui, glyph: &str, tooltip: &str, theme: &Theme, size: f32, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tooltip));
    paint_icon_button(ui, rect, &resp, glyph, theme, active);
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

pub fn paint_icon_button(ui: &Ui, rect: Rect, resp: &Response, glyph: &str, theme: &Theme, active: bool) {
    let painter = ui.painter();
    if resp.hovered() || active {
        let fill = if resp.is_pointer_button_down_on() { theme.surface3 } else if active { theme.accent_soft() } else { theme.surface2 };
        painter.rect_filled(rect, CornerRadius::same(6), fill);
    }
    let color = if active { theme.accent } else if resp.hovered() { theme.text } else { theme.text_dim };
    painter.text(rect.center(), Align2::CENTER_CENTER, glyph, icon_font((rect.height() * 0.5).round()), color);
}

/// Rounded pill with text. Returns its rect.
pub fn pill(painter: &Painter, left_center: Pos2, text: &str, font: FontId, fg: Color32, bg: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), font, fg);
    let size = galley.size() + vec2(14.0, 4.0);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - size.y * 0.5), size);
    painter.rect_filled(rect, CornerRadius::same((size.y * 0.5) as u8), bg);
    painter.galley(rect.min + vec2(7.0, 2.0), galley, fg);
    rect
}

/// Measure a pill without painting.
pub fn pill_width(painter: &Painter, text: &str, font: FontId) -> f32 {
    painter.layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x + 14.0
}

/// Lay out `text` on one line, eliding with "…" so it never exceeds `max_w`.
pub fn elided(painter: &Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

pub fn dot(painter: &Painter, center: Pos2, r: f32, color: Color32) {
    painter.circle_filled(center, r, color);
}

/// Thin, flat spinner arc.
pub fn spinner(painter: &Painter, center: Pos2, r: f32, t: f32, color: Color32) {
    let start = t * 6.0;
    let span = 1.9 + 0.6 * (t * 2.2).sin();
    let pts: Vec<Pos2> = (0..=16)
        .map(|k| {
            let a = start + span * k as f32 / 16.0;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    painter.circle_stroke(center, r, Stroke::new(1.5, with_alpha(color, 0.18)));
    painter.add(Shape::line(pts, Stroke::new(1.6, color)));
}

pub fn sparkline(painter: &Painter, rect: Rect, values: &[f32], max: f32, color: Color32) {
    if values.len() < 2 {
        return;
    }
    let max = max.max(1e-3);
    let n = values.len();
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(rect.left() + rect.width() * i as f32 / (n - 1) as f32, rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0)))
        .collect();
    painter.add(Shape::line(pts, Stroke::new(1.2, color)));
}

pub fn progress(painter: &Painter, rect: Rect, frac: f32, color: Color32, track: Color32) {
    let r = CornerRadius::same((rect.height() * 0.5) as u8);
    painter.rect_filled(rect, r, track);
    let mut fill = rect;
    fill.set_width((rect.width() * frac.clamp(0.0, 1.0)).max(rect.height()));
    if frac > 0.0 {
        painter.rect_filled(fill, r, color);
    }
}

/// Switch with an accessible name (what it turns on or off).
pub fn toggle_named(ui: &mut Ui, on: &mut bool, theme: &Theme, name: &str) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(30.0, 17.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let state = *on;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), state, name));
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let painter = ui.painter();
    let track = mix(theme.surface3, theme.accent, t);
    painter.rect_filled(rect, CornerRadius::same(9), track);
    let x = rect.left() + 8.5 + (rect.width() - 17.0) * t;
    painter.circle_filled(pos2(x, rect.center().y), 6.0, if *on { theme.on_accent } else { theme.text_dim });
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Switch followed by a label; clicking either flips it.
pub fn toggle_row(ui: &mut Ui, on: &mut bool, theme: &Theme, label: &str) -> Response {
    ui.horizontal(|ui| {
        let mut resp = toggle_named(ui, on, theme, label);
        ui.add_space(4.0);
        let l = ui.add(egui::Label::new(label).sense(Sense::click()).selectable(false)).on_hover_cursor(CursorIcon::PointingHand);
        if l.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        resp
    })
    .inner
}

/// Small dim uppercase section label.
pub fn section(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text.to_uppercase()).font(semibold(10.5)).color(theme.text_faint));
}

/// Primary (accent-filled) button.
pub fn primary_button(ui: &mut Ui, text: &str, theme: &Theme) -> Response {
    let font = semibold(12.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, theme.on_accent);
    let size = galley.size() + vec2(24.0, 10.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), text));
    let fill = if resp.is_pointer_button_down_on() {
        mix(theme.accent, Color32::BLACK, 0.15)
    } else if resp.hovered() {
        mix(theme.accent, Color32::WHITE, 0.08)
    } else {
        theme.accent
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    ui.painter().galley(rect.center() - galley.size() * 0.5, galley, theme.on_accent);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Secondary (outlined) button.
pub fn ghost_button(ui: &mut Ui, text: &str, theme: &Theme) -> Response {
    tinted_button(ui, text, theme, theme.text)
}

/// Compact attachment pill (paperclip + name). Returns true when its remove button is clicked.
pub fn file_pill(ui: &mut Ui, name: &str, theme: &Theme, removable: bool) -> bool {
    let font = ui_font(11.5);
    let tw = ui.painter().layout_no_wrap(name.to_owned(), font.clone(), theme.text).size().x.min(220.0);
    let w = 26.0 + tw + if removable { 26.0 } else { 10.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    resp.on_hover_text(name);
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(6), theme.surface, Stroke::new(1.0, theme.border), StrokeKind::Inside);
    p.text(pos2(rect.left() + 14.0, rect.center().y), Align2::CENTER_CENTER, icon::ATTACH, icon_font(11.0), theme.text_dim);
    let g = elided(p, name, font, theme.text, tw);
    p.galley(pos2(rect.left() + 26.0, rect.center().y - g.size().y / 2.0), g, theme.text);
    if !removable {
        return false;
    }
    let xr = Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(18.0, 18.0));
    let x = ui.interact(xr, ui.scope_id().with(("remove-attachment", name)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Remove {name}")));
    if x.hovered() {
        ui.painter().rect_filled(xr, CornerRadius::same(4), theme.surface3);
    }
    ui.painter().text(xr.center(), Align2::CENTER_CENTER, icon::CANCEL, icon_font(8.0), theme.text_dim);
    x.clicked()
}

/// Pill-shaped option for single (`multi == false`) or multiple choice.
pub fn chip(ui: &mut Ui, selected: bool, label: &str, theme: &Theme, multi: bool) -> Response {
    let font = ui_font(12.5);
    let (rect, resp) = {
        let w = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), theme.text).size().x;
        let check = if selected { 16.0 } else { 0.0 };
        ui.allocate_exact_size(vec2(w + 26.0 + check, 28.0), Sense::click())
    };
    let kind = if multi { egui::Role::CheckBox } else { egui::Role::RadioButton };
    resp.widget_info(|| egui::WidgetInfo::selected(kind, ui.is_enabled(), selected, label));
    let (fill, stroke, fg) = if selected {
        (theme.accent_soft(), theme.accent, theme.text)
    } else if resp.hovered() {
        (theme.surface3, theme.border_strong, theme.text)
    } else {
        (theme.surface, theme.border, theme.text_dim)
    };
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(14), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let mut x = rect.left() + 13.0;
    if selected {
        p.text(pos2(x + 5.0, rect.center().y), Align2::CENTER_CENTER, icon::CHECK, icon_font(10.0), theme.accent);
        x += 16.0;
    }
    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn tinted_button(ui: &mut Ui, text: &str, theme: &Theme, color: Color32) -> Response {
    let font = ui_font(12.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = galley.size() + vec2(22.0, 10.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), text));
    let fill = if resp.hovered() { theme.surface3 } else { theme.surface2 };
    ui.painter().rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0, theme.border_strong), StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() * 0.5, galley, color);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Compact flat button for secondary actions inside lists and cards.
pub fn mini_button(ui: &mut Ui, text: &str, theme: &Theme) -> Response {
    mini_button_tinted(ui, text, theme, theme.text_dim)
}

pub fn mini_button_tinted(ui: &mut Ui, text: &str, theme: &Theme, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), ui_font(11.5), color);
    let size = vec2(galley.size().x + 16.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), text));
    let (fill, stroke) = if resp.hovered() { (theme.surface3, theme.border_strong) } else { (theme.surface, theme.border) };
    ui.painter().rect(rect, CornerRadius::same(5), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let fg = if resp.hovered() && color == theme.text_dim { theme.text } else { color };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, ui_font(11.5), fg);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// The Graysense mark: a rounded square with three "sense" bars.
pub fn mark(painter: &Painter, center: Pos2, size: f32, theme: &Theme) {
    let rect = Rect::from_center_size(center, Vec2::splat(size));
    painter.rect_filled(rect, CornerRadius::same((size * 0.28) as u8), theme.accent);
    let bw = size * 0.12;
    let gap = size * 0.08;
    let heights = [0.34, 0.56, 0.42];
    let total = 3.0 * bw + 2.0 * gap;
    for (i, h) in heights.iter().enumerate() {
        let x = center.x - total * 0.5 + i as f32 * (bw + gap);
        let bar = Rect::from_min_size(pos2(x, center.y - size * h * 0.5), vec2(bw, size * h));
        painter.rect_filled(bar, CornerRadius::same((bw * 0.5) as u8), theme.on_accent);
    }
}

pub fn app_icon() -> egui::IconData {
    let size = 64usize;
    let mut rgba = vec![0u8; size * size * 4];
    let accent = [0x5Bu8, 0x8C, 0xFF];
    let radius = 18.0f32;
    let inside = |x: f32, y: f32| -> f32 {
        // Signed distance to a rounded square of half-size 30.
        let hx = 30.0 - radius;
        let qx = (x - 31.5).abs() - hx;
        let qy = (y - 31.5).abs() - hx;
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) - radius
    };
    let bars = [(20.0, 0.34), (29.0, 0.56), (38.0, 0.42)];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32, y as f32);
            let d = inside(fx, fy);
            let a = (0.5 - d).clamp(0.0, 1.0);
            let mut c = [accent[0] as f32, accent[1] as f32, accent[2] as f32];
            for (bx, h) in bars {
                let half = 32.0 * h;
                if fx >= bx && fx < bx + 7.0 && (fy - 31.5).abs() < half {
                    c = [255.0, 255.0, 255.0];
                }
            }
            let i = (y * size + x) * 4;
            rgba[i] = c[0] as u8;
            rgba[i + 1] = c[1] as u8;
            rgba[i + 2] = c[2] as u8;
            rgba[i + 3] = (a * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: size as u32, height: size as u32 }
}

/// Keyboard shortcut hint (e.g. "Ctrl K") drawn as small keycaps.
pub fn kbd(painter: &Painter, right_center: Pos2, keys: &[&str], theme: &Theme) -> f32 {
    let mut x = right_center.x;
    for key in keys.iter().rev() {
        let galley = painter.layout_no_wrap((*key).to_owned(), ui_font(10.5), theme.text_dim);
        let w = galley.size().x + 8.0;
        let rect = Rect::from_min_size(pos2(x - w, right_center.y - 8.5), vec2(w, 17.0));
        painter.rect(rect, CornerRadius::same(4), theme.surface, Stroke::new(1.0, theme.border_strong), StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() * 0.5, galley, theme.text_dim);
        x -= w + 3.0;
    }
    right_center.x - x
}

