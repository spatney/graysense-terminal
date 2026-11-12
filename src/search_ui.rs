//! Search everything (Ctrl+K): session titles and conversations, and the files in session
//! folders, in one list with highlighted matches, like a chat app's search or an editor's
//! quick open.

use std::ops::Range;
use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Key, Rect, RichText, Sense, Stroke, StrokeKind, TextEdit, pos2, vec2};
use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};

use crate::kit::{self, icon};
use crate::search::{DocKind, Hit, Index};
use crate::theme::{Theme, icon_font, semibold, ui as ui_font, with_alpha};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    Sessions,
    Files,
}

pub enum SearchAction {
    OpenSession(String),
    OpenFile(PathBuf),
    RevealFile(PathBuf),
    AttachFile(PathBuf),
}

pub struct SearchUi {
    pub open: bool,
    query: String,
    scope: Scope,
    selected: usize,
    focus: bool,
    /// Results for (query, scope, index generation).
    cache: Option<(String, Scope, u64, Vec<Hit>)>,
}

impl Default for SearchUi {
    fn default() -> Self {
        Self { open: false, query: String::new(), scope: Scope::All, selected: 0, focus: false, cache: None }
    }
}

const ROW_H: f32 = 54.0;

impl SearchUi {
    pub fn open_with(&mut self, query: &str) {
        self.open = true;
        self.query = query.to_owned();
        self.selected = 0;
        self.focus = true;
        self.cache = None;
    }

    fn results(&mut self, index: &Index, generation: u64) -> &[Hit] {
        let q = self.query.trim().to_owned();
        let fresh = self.cache.as_ref().is_some_and(|(cq, cs, cg, _)| *cq == q && *cs == self.scope && *cg == generation);
        if !fresh {
            let want = |h: &Hit| matches!((self.scope, &index.doc(h).kind), (Scope::All, _) | (Scope::Sessions, DocKind::Session(_)) | (Scope::Files, DocKind::File(_)));
            let hits: Vec<Hit> = (if q.is_empty() { index.recent(200) } else { index.search(&q, 200) }).into_iter().filter(want).take(60).collect();
            self.cache = Some((q, self.scope, generation, hits));
        }
        &self.cache.as_ref().unwrap().3
    }

    /// Draw the search dialog. `status` describes the index ("Indexing…", "277 sessions · 1,204 files").
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, index: &Index, generation: u64, status: &str, can_attach: bool) -> Option<SearchAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let screen = ctx.content_rect();
        // Backdrop.
        let backdrop = egui::Area::new(Id::unique("search-backdrop")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, with_alpha(Color32::BLACK, if theme.dark { 0.45 } else { 0.25 }));
            resp
        });
        if backdrop.inner.clicked() {
            self.open = false;
            return None;
        }

        let w = (screen.width() - 80.0).clamp(420.0, 760.0);
        let h = (screen.height() - 160.0).clamp(260.0, 600.0);
        let rect = Rect::from_min_size(pos2(screen.center().x - w / 2.0, screen.top() + 70.0), vec2(w, h));
        egui::Area::new(Id::unique("search-dialog")).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            let p = ui.painter();
            p.rect(rect, CornerRadius::same(12), theme.surface, Stroke::new(1.0, theme.border_strong), StrokeKind::Inside);
            let inner = rect.shrink2(vec2(14.0, 12.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                // Keyboard.
                let n = self.cache.as_ref().map(|c| c.3.len()).unwrap_or(0);
                let (down, up, enter, ctrl, alt, esc) = ui.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                        i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                        i.key_pressed(Key::Enter),
                        i.modifiers.command,
                        i.modifiers.alt,
                        i.consume_key(egui::Modifiers::NONE, Key::Escape),
                    )
                });
                if esc {
                    self.open = false;
                }
                if down && n > 0 {
                    self.selected = (self.selected + 1).min(n - 1);
                }
                if up {
                    self.selected = self.selected.saturating_sub(1);
                }

                // Query field.
                let field = ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SEARCH).font(icon_font(15.0)).color(theme.text_dim));
                    let r = ui.add(
                        TextEdit::singleline(&mut self.query)
                            .id(Id::unique("search-query"))
                            .hint_text("Search sessions and files")
                            .frame(egui::Frame::NONE)
                            .font(FontId::proportional(17.0))
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut self.focus) {
                        r.request_focus();
                    }
                    r
                });
                if field.inner.changed() {
                    self.selected = 0;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (s, label) in [(Scope::All, "All"), (Scope::Sessions, "Sessions"), (Scope::Files, "Files")] {
                        if kit::chip(ui, self.scope == s, label, theme, false).clicked() {
                            self.scope = s;
                            self.selected = 0;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(status).font(ui_font(11.5)).color(theme.text_faint));
                    });
                });
                ui.add_space(6.0);
                let p = ui.painter();
                p.hline(inner.x_range(), ui.cursor().top(), Stroke::new(1.0, theme.border));
                ui.add_space(4.0);

                let query_empty = self.query.trim().is_empty();
                let hits = self.results(index, generation).to_vec();
                self.selected = self.selected.min(hits.len().saturating_sub(1));
                if hits.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(icon::SEARCH).font(icon_font(22.0)).color(theme.text_faint));
                        let msg = if query_empty { "Nothing here yet".to_owned() } else { format!("No matches for “{}”", self.query.trim()) };
                        ui.label(RichText::new(msg).font(semibold(13.5)).color(theme.text_dim));
                        ui.label(RichText::new("Search covers session names and conversations, and every file in your sessions folder.").font(ui_font(12.0)).color(theme.text_faint));
                    });
                } else {
                    if query_empty {
                        ui.label(RichText::new("RECENT").font(semibold(10.5)).color(theme.text_faint));
                    }
                    let footer_h = 24.0;
                    let list_h = ui.available_height() - footer_h;
                    egui::ScrollArea::vertical().id_salt("search-results").max_height(list_h).auto_shrink([false; 2]).show_rows(ui, ROW_H, hits.len(), |ui, range| {
                        for i in range {
                            let hit = &hits[i];
                            let doc = index.doc(hit);
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &doc.title));
                            if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                                self.selected = i;
                            }
                            let sel = i == self.selected;
                            if sel && (down || up) {
                                resp.scroll_to_me(None);
                            }
                            let p = ui.painter();
                            if sel {
                                p.rect_filled(r, CornerRadius::same(8), theme.surface2);
                            }
                            let (glyph, color) = match &doc.kind {
                                DocKind::Session(_) => (icon::CHAT, theme.accent),
                                DocKind::File(_) => (icon::DOCUMENT, theme.text_dim),
                            };
                            p.text(pos2(r.left() + 18.0, r.top() + 18.0), Align2::CENTER_CENTER, glyph, icon_font(13.0), color);
                            // Right: when; on hover/selection, file actions.
                            let when = crate::app::short_age((chrono::Utc::now().timestamp() - doc.modified).max(0) as u64);
                            let mut right = r.right() - 10.0;
                            if let DocKind::File(path) = &doc.kind
                                && (sel || resp.hovered())
                            {
                                let acts: &[(&str, &str, u8)] = if can_attach { &[(icon::FOLDER, "Show in File Explorer (Ctrl+Enter)", 1), (icon::ATTACH, "Attach to the focused agent (Alt+Enter)", 2)] } else { &[(icon::FOLDER, "Show in File Explorer (Ctrl+Enter)", 1)] };
                                for (glyph, tip, kind) in acts {
                                    let b = Rect::from_center_size(pos2(right - 12.0, r.top() + 18.0), vec2(24.0, 24.0));
                                    let bresp = ui.interact(b, Id::unique(("search-act", i, *kind)), Sense::click()).on_hover_text(*tip);
                                    kit::paint_icon_button(ui, b, &bresp, glyph, theme, false);
                                    if bresp.clicked() {
                                        action = Some(if *kind == 1 { SearchAction::RevealFile(path.clone()) } else { SearchAction::AttachFile(path.clone()) });
                                    }
                                    right -= 26.0;
                                }
                            } else {
                                let wr = ui.painter().text(pos2(right, r.top() + 18.0), Align2::RIGHT_CENTER, when, ui_font(11.0), theme.text_faint);
                                right = wr.left() - 8.0;
                            }
                            let p = ui.painter();
                            let text_x = r.left() + 36.0;
                            let max_w = (right - text_x - 6.0).max(40.0);
                            // Title (with matches), then where it lives.
                            let mut title = highlight(&doc.title, &hit.title_ranges, semibold(13.0), theme.text, theme, max_w);
                            if !doc.location.is_empty() {
                                title.append(&format!("   {}", doc.location), 0.0, TextFormat::simple(ui_font(11.5), theme.text_faint));
                            }
                            title.wrap = TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                            let g = p.layout_job(title);
                            p.galley(pos2(text_x, r.top() + 18.0 - g.size().y / 2.0), g, theme.text);
                            let snippet = if hit.snippet.is_empty() {
                                let why = if matches!(doc.kind, DocKind::File(_)) { "Matched by file name" } else { "Matched by session name" };
                                LayoutJob::simple_singleline(why.into(), ui_font(12.0), theme.text_faint)
                            } else {
                                let mut j = highlight(&hit.snippet, &hit.snippet_ranges, ui_font(12.0), theme.text_dim, theme, max_w);
                                if let Some(line) = hit.line {
                                    j.append(&format!("   line {line}"), 0.0, TextFormat::simple(ui_font(11.0), theme.text_faint));
                                }
                                j
                            };
                            let mut snippet = snippet;
                            snippet.wrap = TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                            let g = p.layout_job(snippet);
                            p.galley(pos2(text_x, r.top() + 37.0 - g.size().y / 2.0), g, theme.text_dim);
                            if resp.clicked() {
                                action = Some(primary(&doc.kind));
                            }
                        }
                    });
                    if enter && let Some(hit) = hits.get(self.selected) {
                        let kind = &index.doc(hit).kind;
                        action = Some(match kind {
                            DocKind::File(p) if ctrl => SearchAction::RevealFile(p.clone()),
                            DocKind::File(p) if alt && can_attach => SearchAction::AttachFile(p.clone()),
                            _ => primary(kind),
                        });
                    }
                }
                // Footer.
                let f = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 18.0), inner.max);
                let tip = if can_attach { "↑↓ move  ·  Enter open  ·  Ctrl+Enter show in folder  ·  Alt+Enter attach  ·  Esc close" } else { "↑↓ move  ·  Enter open  ·  Ctrl+Enter show in folder  ·  Esc close" };
                let g = kit::elided(ui.painter(), tip, ui_font(11.0), theme.text_faint, f.width());
                ui.painter().galley(pos2(f.left(), f.center().y - g.size().y / 2.0), g, theme.text_faint);
            });
        });
        if action.is_some() {
            self.open = false;
        }
        action
    }
}

fn primary(kind: &DocKind) -> SearchAction {
    match kind {
        DocKind::Session(id) => SearchAction::OpenSession(id.clone()),
        DocKind::File(p) => SearchAction::OpenFile(p.clone()),
    }
}

/// `text` with `ranges` marked (accent wash), on one line.
fn highlight(text: &str, ranges: &[Range<usize>], font: FontId, color: Color32, theme: &Theme, max_w: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let normal = TextFormat::simple(font.clone(), color);
    let marked = TextFormat { background: theme.accent_soft(), color: theme.text, ..TextFormat::simple(font, theme.text) };
    let mut at = 0;
    for r in ranges {
        if r.start < at || r.end > text.len() || !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
            continue;
        }
        job.append(&text[at..r.start], 0.0, normal.clone());
        job.append(&text[r.start..r.end], 0.0, marked.clone());
        at = r.end;
    }
    job.append(&text[at..], 0.0, normal);
    job.wrap = TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job
}
