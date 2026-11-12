//! GPU terminal grid renderer built on egui's text pipeline.

use std::collections::HashMap;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};

use super::{Palette, Proxy, boxdraw, xterm_256};
use crate::theme::{Theme, bold_coords, mix, with_alpha};

#[derive(Clone, Copy)]
pub struct CellSnap {
    pub c: char,
    pub fg: Color32,
    pub bg: Color32,
    pub ul: Color32,
    pub flags: Flags,
    pub sel: bool,
    pub link: bool,
}

impl Default for CellSnap {
    fn default() -> Self {
        CellSnap { c: ' ', fg: Color32::WHITE, bg: Color32::TRANSPARENT, ul: Color32::WHITE, flags: Flags::empty(), sel: false, link: false }
    }
}

#[derive(Clone, Copy)]
pub struct CursorSnap {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub wide: bool,
}

#[derive(Default)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<CellSnap>,
    pub zerowidth: Vec<(usize, Vec<char>)>,
    pub cursor: Option<CursorSnap>,
    pub display_offset: usize,
    pub mode: Option<TermMode>,
    pub valid: bool,
}

fn rgb(c: alacritty_terminal::vte::ansi::Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// Approximate relative luminance (gamma ≈ 2), cheap enough to run per cell.
fn luminance(c: Color32) -> f32 {
    let l = |v: u8| {
        let s = v as f32 / 255.0;
        s * s
    };
    0.2126 * l(c.r()) + 0.7152 * l(c.g()) + 0.0722 * l(c.b())
}

fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Keep text on the default background readable when a program hard-codes a
/// colour meant for the opposite theme (e.g. a dark-theme prompt on Paper).
fn readable(fg: Color32, bg_lum: f32) -> Color32 {
    if contrast(luminance(fg), bg_lum) >= 1.7 {
        return fg;
    }
    let target = if bg_lum > 0.4 { Color32::BLACK } else { Color32::WHITE };
    let mut t = 0.25;
    while t < 0.95 {
        let c = mix(fg, target, t);
        if contrast(luminance(c), bg_lum) >= 3.0 {
            return c;
        }
        t += 0.1;
    }
    mix(fg, target, 0.85)
}

/// Resolve a cell colour; `None` means "default background" (transparent glass).
fn resolve(color: Color, colors: &Colors, p: &Palette) -> Option<Color32> {
    match color {
        Color::Spec(c) => Some(rgb(c)),
        Color::Indexed(i) => Some(colors[i as usize].map(rgb).unwrap_or_else(|| xterm_256(i, p))),
        Color::Named(n) => {
            let idx = n as usize;
            if let Some(c) = colors[idx] {
                return Some(rgb(c));
            }
            Some(match n {
                NamedColor::Foreground | NamedColor::BrightForeground => p.fg,
                NamedColor::Background => return None,
                NamedColor::Cursor => p.cursor,
                NamedColor::DimForeground => mix(p.fg, p.bg, 0.4),
                _ if idx < 16 => p.ansi[idx],
                _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&idx) => {
                    mix(p.ansi[idx - NamedColor::DimBlack as usize], p.bg, 0.35)
                }
                _ => p.fg,
            })
        }
    }
}

pub fn snapshot(term: &Term<Proxy>, p: &Palette, out: &mut Snapshot) {
    let content = term.renderable_content();
    let cols = term.columns();
    let rows = term.screen_lines();
    out.cols = cols;
    out.rows = rows;
    out.cells.clear();
    out.cells.resize(cols * rows, CellSnap { fg: p.fg, ul: p.fg, ..Default::default() });
    out.zerowidth.clear();
    out.display_offset = content.display_offset;
    out.mode = Some(content.mode);
    let off = content.display_offset as i32;
    let sel = content.selection;
    let colors = content.colors;
    let bg_lum = luminance(p.bg);
    for indexed in content.display_iter {
        let row = indexed.point.line.0 + off;
        if row < 0 || row as usize >= rows {
            continue;
        }
        let col = indexed.point.column.0;
        if col >= cols {
            continue;
        }
        let cell = indexed.cell;
        let mut fg = resolve(cell.fg, colors, p).unwrap_or(p.fg);
        let mut bg = resolve(cell.bg, colors, p).unwrap_or(Color32::TRANSPARENT);
        let flags = cell.flags;
        if flags.contains(Flags::INVERSE) {
            let real_bg = if bg == Color32::TRANSPARENT { p.bg } else { bg };
            bg = fg;
            fg = real_bg;
        }
        if flags.contains(Flags::DIM) {
            fg = mix(fg, p.bg, 0.42);
        }
        // Powerline caps and block fills are shapes meant to blend with neighbours; leave them.
        let shape = matches!(cell.c, '\u{E0B0}'..='\u{E0D7}' | '\u{2580}'..='\u{259F}');
        if bg == Color32::TRANSPARENT && cell.c != ' ' && !shape {
            fg = readable(fg, bg_lum);
        }
        let mut c = cell.c;
        if flags.contains(Flags::HIDDEN) {
            c = ' ';
        }
        let ul = cell.underline_color().and_then(|u| resolve(u, colors, p)).unwrap_or(fg);
        let idx = row as usize * cols + col;
        out.cells[idx] = CellSnap {
            c,
            fg,
            bg,
            ul,
            flags,
            sel: sel.is_some_and(|s| s.contains(indexed.point)),
            link: cell.hyperlink().is_some(),
        };
        if let Some(zw) = cell.zerowidth() {
            out.zerowidth.push((idx, zw.to_vec()));
        }
    }
    let cur = content.cursor;
    out.cursor = None;
    if cur.shape != CursorShape::Hidden {
        let row = cur.point.line.0 + off;
        if row >= 0 && (row as usize) < rows && cur.point.column.0 < cols {
            let idx = row as usize * cols + cur.point.column.0;
            out.cursor = Some(CursorSnap {
                row: row as usize,
                col: cur.point.column.0,
                shape: cur.shape,
                wide: out.cells[idx].flags.contains(Flags::WIDE_CHAR),
            });
        }
    }
    out.valid = true;
}

#[derive(Clone)]
pub struct Metrics {
    pub font: FontId,
    pub cell: Vec2,
    pub glyph_h: f32,
}

pub fn metrics(ctx: &egui::Context, size: f32) -> Metrics {
    let font = FontId::monospace(size);
    let (w, h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let ppp = ctx.pixels_per_point();
    let cell_h = ((h * 1.06) * ppp).round() / ppp;
    Metrics { font, cell: vec2(w, cell_h), glyph_h: h }
}

/// Per-pane cache of "does this char render at exactly one cell width in the primary font".
#[derive(Default)]
pub struct GlyphCache {
    simple: HashMap<char, bool>,
    size: f32,
}

impl GlyphCache {
    fn is_simple(&mut self, ctx: &egui::Context, m: &Metrics, c: char) -> bool {
        if (' '..='~').contains(&c) {
            return true;
        }
        if boxdraw::is_procedural(c) {
            return false;
        }
        if self.size != m.font.size {
            self.simple.clear();
            self.size = m.font.size;
        }
        *self.simple.entry(c).or_insert_with(|| {
            let w = ctx.fonts_mut(|f| f.glyph_width(&m.font, c));
            (w - m.cell.x).abs() < 0.02 * m.cell.x
        })
    }
}

pub struct PaintOpts<'a> {
    pub theme: &'a Theme,
    pub palette: Palette,
    pub focused: bool,
    pub smooth_cursor: bool,
    pub dt: f32,
}

#[derive(Clone, Copy, PartialEq)]
struct StyleKey {
    fg: Color32,
    bold: bool,
    italic: bool,
}

pub fn paint(
    ctx: &egui::Context,
    painter: &Painter,
    origin: Pos2,
    snap: &Snapshot,
    m: &Metrics,
    cache: &mut GlyphCache,
    cursor_anim: &mut Option<Rect>,
    o: &PaintOpts,
) {
    if !snap.valid {
        return;
    }
    let ppp = ctx.pixels_per_point();
    let cw = m.cell.x;
    let ch = m.cell.y;
    let text_dy = ((ch - m.glyph_h) * 0.5 * ppp).round() / ppp;
    let sel_color = o.theme.selection();
    let cols = snap.cols;

    for row in 0..snap.rows {
        let y = origin.y + row as f32 * ch;
        let cells = &snap.cells[row * cols..(row + 1) * cols];

        // Backgrounds (merged runs).
        let mut run: Option<(usize, Color32)> = None;
        for col in 0..=cols {
            let color = if col < cols {
                let c = &cells[col];
                if c.sel { sel_color } else { c.bg }
            } else {
                Color32::TRANSPARENT
            };
            match run {
                Some((start, rc)) if rc == color => {
                    let _ = start;
                }
                _ => {
                    if let Some((start, rc)) = run.take()
                        && rc != Color32::TRANSPARENT {
                            let r = Rect::from_min_max(pos2(origin.x + start as f32 * cw, y), pos2(origin.x + col as f32 * cw, y + ch));
                            painter.rect_filled(r, 0.0, rc);
                        }
                    run = Some((col, color));
                }
            }
        }

        // Text segments.
        let mut job = LayoutJob::default();
        job.wrap = TextWrapping::no_max_width();
        let mut seg_col = 0usize;
        let mut cur_key: Option<StyleKey> = None;
        let mut cur_text = String::new();
        let flush_section = |job: &mut LayoutJob, key: &Option<StyleKey>, text: &mut String| {
            if let Some(k) = key
                && !text.is_empty() {
                    let mut fmt = TextFormat::simple(m.font.clone(), k.fg);
                    if k.bold {
                        fmt.coords = bold_coords();
                    }
                    fmt.italics = k.italic;
                    job.append(text, 0.0, fmt);
                    text.clear();
                }
        };
        let flush_job = |job: &mut LayoutJob, seg_col: usize| {
            if !job.text.is_empty() {
                let j = std::mem::take(job);
                let galley = painter.layout_job(j);
                painter.galley(pos2(origin.x + seg_col as f32 * cw, y + text_dy), galley, Color32::WHITE);
            }
            job.wrap = TextWrapping::no_max_width();
        };

        let mut col = 0;
        while col < cols {
            let cell = &cells[col];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                col += 1;
                continue;
            }
            let c = if cell.c == '\0' { ' ' } else { cell.c };
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let has_zw = !snap.zerowidth.is_empty() && snap.zerowidth.iter().any(|(i, _)| *i == row * cols + col);
            let simple = !wide && !has_zw && cache.is_simple(ctx, m, c);
            if simple {
                if job.text.is_empty() && cur_text.is_empty() {
                    if c == ' ' {
                        // Skip leading blanks entirely.
                        col += 1;
                        seg_col = col;
                        continue;
                    }
                    seg_col = col;
                }
                let key = StyleKey {
                    fg: cell.fg,
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                };
                if cur_key != Some(key) && c != ' ' {
                    flush_section(&mut job, &cur_key, &mut cur_text);
                    cur_key = Some(key);
                }
                cur_text.push(c);
                col += 1;
            } else {
                flush_section(&mut job, &cur_key, &mut cur_text);
                flush_job(&mut job, seg_col);
                cur_key = None;
                let span = if wide { 2 } else { 1 };
                let rect = Rect::from_min_size(pos2(origin.x + col as f32 * cw, y), vec2(cw * span as f32, ch));
                if !boxdraw::draw(painter, c, rect, cell.fg, ppp) {
                    let mut text = c.to_string();
                    if let Some((_, zw)) = snap.zerowidth.iter().find(|(i, _)| *i == row * cols + col) {
                        text.extend(zw.iter());
                    }
                    let font = m.font.clone();
                    let galley = painter.layout_no_wrap(text, font, cell.fg);
                    let gs = galley.size();
                    let pos = rect.center() - gs * 0.5;
                    painter.galley(pos2(pos.x, y + text_dy), galley, cell.fg);
                }
                col += span;
                seg_col = col;
            }
        }
        flush_section(&mut job, &cur_key, &mut cur_text);
        flush_job(&mut job, seg_col);

        // Decorations.
        for (col, cell) in cells.iter().enumerate() {
            let f = cell.flags;
            if !f.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT) && !cell.link {
                continue;
            }
            let x0 = origin.x + col as f32 * cw;
            let x1 = x0 + cw;
            let base = y + ch - (ch * 0.10).max(1.5);
            let s = Stroke::new(1.0, cell.ul);
            if f.contains(Flags::UNDERLINE) {
                painter.hline(x0..=x1, base, s);
            }
            if f.contains(Flags::DOUBLE_UNDERLINE) {
                painter.hline(x0..=x1, base, s);
                painter.hline(x0..=x1, base - 2.0, s);
            }
            if f.contains(Flags::UNDERCURL) {
                let pts: Vec<Pos2> = (0..=4).map(|k| pos2(x0 + cw * k as f32 / 4.0, base + if k % 2 == 0 { -1.2 } else { 1.2 })).collect();
                painter.add(egui::Shape::line(pts, s));
            }
            if f.intersects(Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE) || (cell.link && !f.intersects(Flags::ALL_UNDERLINES)) {
                painter.hline(x0 + cw * 0.15..=x1 - cw * 0.15, base, Stroke::new(1.0, with_alpha(cell.ul, 0.7)));
            }
            if f.contains(Flags::STRIKEOUT) {
                painter.hline(x0..=x1, y + ch * 0.55, Stroke::new(1.0, cell.fg));
            }
        }
    }

    // Cursor.
    match snap.cursor {
        Some(cur) => {
            let span = if cur.wide { 2.0 } else { 1.0 };
            let target = Rect::from_min_size(pos2(origin.x + cur.col as f32 * cw, origin.y + cur.row as f32 * ch), vec2(cw * span, ch));
            let rect = match cursor_anim {
                Some(r) if r.width() > 0.0 && o.smooth_cursor => {
                    let k = 1.0 - (-o.dt * 30.0).exp();
                    let nr = Rect::from_min_max(r.min + (target.min - r.min) * k, r.max + (target.max - r.max) * k);
                    if (nr.min - target.min).length() < 0.3 { target } else { nr }
                }
                _ => target,
            };
            *cursor_anim = Some(rect);
            if rect != target {
                ctx.request_repaint();
            }
            let color = o.palette.cursor;
            let shape = if o.focused { cur.shape } else { CursorShape::HollowBlock };
            match shape {
                CursorShape::Block => {
                    painter.rect_filled(rect, CornerRadius::same(1), color);
                    let cell = snap.cells[cur.row * snap.cols + cur.col];
                    if cell.c != ' ' && cell.c != '\0' && rect == target
                        && !boxdraw::draw(painter, cell.c, rect, o.palette.bg, ppp) {
                            painter.text(rect.center(), Align2::CENTER_CENTER, cell.c, m.font.clone(), o.palette.bg);
                        }
                }
                CursorShape::Beam => {
                    let bar = Rect::from_min_size(rect.min, vec2(2.0, ch));
                    painter.rect_filled(bar, 0.0, color);
                }
                CursorShape::Underline => {
                    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max);
                    painter.rect_filled(bar, 0.0, color);
                }
                _ => {
                    painter.rect_stroke(rect, CornerRadius::same(1), Stroke::new(1.0, with_alpha(color, 0.7)), StrokeKind::Inside);
                }
            }
        }
        _ => *cursor_anim = None,
    }
}
