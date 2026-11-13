//! Agent bay UI: header, virtualized transcript, interactive cards, composer, dock.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Frame, Id, Key, KeyboardShortcut, Margin, Modifiers,
    Rect, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, pos2, vec2,
};
use serde_json::Value;

use super::backend::ModelInfo;
use super::markdown::{self, MdOut};
use super::{AgentView, Item, Level, Mode, PermDecision, Status, ToolState, tool_icon, tool_target};
use crate::canvas::CanvasUiAction;
use crate::kit::{self, icon};
use crate::theme::{Theme, icon_font, mix, mono, semibold, ui as ui_font, with_alpha};

pub enum AgentAction {
    Send(String),
    Abort,
    SetModel(String),
    SetMode(Mode),
    OpenUrl(String),
    Copy(String),
    Focus,
    Close,
    Maximize,
    CloseCanvas(String),
    /// Open the file picker to attach files to the next message.
    Attach,
    /// Undo memories saved from this conversation.
    ForgetMemories(Vec<crate::memory::MemoryId>),
    /// Show the Memory panel (optionally focused on one memory).
    OpenMemory(Option<crate::memory::MemoryId>),
    /// Folder actions from the header's path menu.
    Folder(FolderAction),
    /// Name the session (from the header title).
    Rename(String),
    /// Put an earlier message back in the composer to edit and resend.
    EditPrompt(String),
    /// From the Files tab.
    File(super::files::FileAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderAction {
    Reveal,
    VsCode,
    Terminal,
    NewAgent,
    CopyPath,
}

pub struct AgentCtx<'a> {
    pub theme: &'a Theme,
    pub t: f32,
    pub models: &'a [ModelInfo],
    pub show_reasoning: bool,
    pub focused: bool,
    pub maximized: bool,
    /// Where session folders live (paths inside it are shown as "Sessions › …").
    pub sessions_root: &'a std::path::Path,
}

pub fn status_color(status: &Status, theme: &Theme) -> Color32 {
    match status {
        Status::Idle => theme.ok,
        Status::Booting | Status::Thinking | Status::Streaming | Status::Tool(_) | Status::Compacting => theme.accent,
        Status::Waiting(_) => theme.warn,
        Status::Error(_) => theme.err,
        Status::Offline => theme.text_faint,
    }
}

pub const HEADER_H: f32 = 36.0;

/// Bay header: title, cwd, status, model & mode pickers, auto-approve, dock, close.
pub fn header(ui: &mut Ui, view: &mut AgentView, rect: Rect, cx: &AgentCtx, out: &mut Vec<AgentAction>) {
    let theme = cx.theme;
    let p = ui.painter_at(rect);
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme.border));
    let dot = pos2(rect.left() + 17.0, rect.center().y);
    let cy = rect.center().y;
    let color = status_color(&view.status, theme);
    if view.status.is_busy() {
        kit::spinner(&p, dot, 5.0, cx.t, color);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    } else {
        kit::dot(&p, dot, 4.0, color);
    }
    let x = rect.left() + 30.0;
    let status_text = match (&view.status, &view.intent) {
        (s, Some(intent)) if s.is_busy() => intent.clone(),
        (s, _) => s.label(),
    };
    let status_fg = if view.status.needs_attention() { theme.warn } else { theme.text_dim };

    let mut right = rect.right() - 6.0;
    let top = rect.top() + (rect.height() - 26.0) / 2.0;
    let slot = |right: &mut f32, w: f32| {
        *right -= w;
        Rect::from_min_size(pos2(*right, top), vec2(w, 26.0))
    };
    let btn = |ui: &mut Ui, r: Rect, glyph: &str, tip: &str, active: bool, idn: &str| -> egui::Response {
        let resp = ui.interact(r, ui.scope_id().with(idn), Sense::click()).on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand);
        kit::paint_icon_button(ui, r, &resp, glyph, theme, active);
        resp
    };
    if btn(ui, slot(&mut right, 26.0), icon::CANCEL, "Close agent", false, "close").clicked() {
        out.push(AgentAction::Close);
    }
    if btn(ui, slot(&mut right, 26.0), if cx.maximized { icon::BACK_TO_WINDOW } else { icon::FULLSCREEN }, "Maximize (Ctrl+Shift+Enter)", cx.maximized, "max").clicked() {
        out.push(AgentAction::Maximize);
    }
    let files_tab = view.canvases.len() + view.apps.len();
    let files_open = view.dock_open && view.dock_tab == files_tab;
    if !view.deferred && btn(ui, slot(&mut right, 26.0), icon::FOLDER, "Files in this session's folder", files_open, "files").clicked() {
        view.dock_open = !files_open;
        view.dock_tab = files_tab;
    }
    let canvas_open = view.dock_open && view.dock_tab < files_tab;
    if files_tab > 0 && btn(ui, slot(&mut right, 26.0), icon::DOCK, "Canvases and apps", canvas_open, "dock").clicked() {
        view.dock_open = !canvas_open;
        view.dock_tab = files_tab - 1;
    }
    let auto = view.auto_approve.load(std::sync::atomic::Ordering::Relaxed);
    if btn(ui, slot(&mut right, 26.0), icon::LIGHTNING, if auto { "Auto-approve is ON: tools run without asking" } else { "Auto-approve tool requests" }, auto, "auto").clicked() {
        view.auto_approve.store(!auto, std::sync::atomic::Ordering::Relaxed);
    }
    right -= 6.0;

    // Memory: what this agent started with (or that it's incognito). Incognito always stays
    // visible, collapsing to its icon when the bay is narrow.
    if view.incognito || !view.memory_used.is_empty() {
        let full = if view.incognito { "Incognito".to_owned() } else { format!("{}", view.memory_used.len()) };
        let full_w = kit::pill_width(&p, &full, ui_font(11.5)) + 18.0;
        let fit = if right - full_w > x + 200.0 {
            Some((full, full_w))
        } else if view.incognito && right - 24.0 > x + 90.0 {
            Some((String::new(), 24.0))
        } else {
            None
        };
        if let Some((label, w)) = fit {
            let r = slot(&mut right, w);
            let tip = if view.incognito { "Incognito: this agent doesn't use or add to memory" } else { "Memories this agent started with" };
            let resp = ui.interact(r, ui.scope_id().with("memory"), Sense::click()).on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if view.incognito { "Incognito".to_owned() } else { format!("{} memories in use", view.memory_used.len()) }));
            let fill = if resp.hovered() { theme.surface3 } else { theme.surface2 };
            p.rect_filled(Rect::from_min_size(pos2(r.left(), cy - 10.0), vec2(w, 20.0)), CornerRadius::same(10), fill);
            let icon_x = if label.is_empty() { r.center().x } else { r.left() + 11.0 };
            p.text(pos2(icon_x, cy), Align2::CENTER_CENTER, if view.incognito { icon::HIDE } else { icon::MEMORY }, icon_font(10.0), if view.incognito { theme.text_dim } else { theme.accent });
            if !label.is_empty() {
                p.text(pos2(r.left() + 20.0, cy), Align2::LEFT_CENTER, &label, ui_font(11.5), theme.text_dim);
            }
            if !view.incognito {
                egui::Popup::menu(&resp).width(320.0).show(|ui| {
                    ui.label(RichText::new("This agent started with").font(semibold(12.5)));
                    ui.add_space(2.0);
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for (_, text) in &view.memory_used {
                            ui.horizontal_top(|ui| {
                                ui.label(RichText::new("•").color(theme.text_faint));
                                ui.add(egui::Label::new(RichText::new(text).font(ui_font(12.0)).color(theme.text_dim)).wrap());
                            });
                        }
                    });
                    ui.add_space(4.0);
                    if kit::mini_button(ui, "Open Memory", theme).clicked() {
                        out.push(AgentAction::OpenMemory(None));
                        ui.close();
                    }
                });
            }
            right -= 6.0;
        }
    }

    // Left side: title, live status and working directory, fitted into what's left.
    let avail = (right - 10.0 - x).max(0.0);
    let status_g = kit::elided(&p, &status_text, ui_font(12.0), status_fg, (avail * 0.45).max(60.0));
    let show_status = avail > status_g.size().x + 120.0;
    let title_max = if show_status { avail - status_g.size().x - 10.0 } else { avail };
    let title = view.display_title();
    let title_g = kit::elided(&p, &title, semibold(13.0), theme.text, title_max);
    let mut lx = x;
    let tw = title_g.size().x;
    // Click the title to rename the session.
    let trect = Rect::from_min_size(pos2(lx - 4.0, cy - 11.0), vec2(tw + 8.0, 22.0));
    let tresp = ui.interact(trect, ui.scope_id().with("title"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Rename");
    tresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Rename “{title}”")));
    if tresp.hovered() {
        p.rect_filled(trect, CornerRadius::same(5), theme.surface2);
    }
    if tresp.clicked() {
        view.rename_draft = Some(title.clone());
    }
    egui::Popup::menu(&tresp).width(300.0).show(|ui| {
        ui.label(RichText::new("Rename session").font(semibold(12.5)));
        let draft = view.rename_draft.get_or_insert_with(|| title.clone());
        let r = ui.add(egui::TextEdit::singleline(draft).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 5)));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(4.0);
        let mut save = enter;
        ui.horizontal(|ui| {
            save |= kit::primary_button(ui, "Rename", theme).clicked();
            if kit::ghost_button(ui, "Cancel", theme).clicked() {
                view.rename_draft = None;
                ui.close();
            }
        });
        if save {
            if let Some(name) = view.rename_draft.take() {
                out.push(AgentAction::Rename(name.trim().to_owned()));
            }
            ui.close();
        }
    });
    p.galley(pos2(lx, cy - title_g.size().y / 2.0), title_g, theme.text);
    lx += tw + 10.0;
    if show_status {
        let sw = status_g.size().x;
        p.galley(pos2(lx, cy - status_g.size().y / 2.0), status_g, status_fg);
        lx += sw + 12.0;
    }
    let cwd_room = right - 10.0 - lx;
    if cwd_room > 70.0 {
        if view.deferred {
            let g = kit::elided(&p, "Gets its own folder when you send a message", ui_font(11.5), theme.text_faint, cwd_room);
            p.galley(pos2(lx, cy - g.size().y / 2.0), g, theme.text_faint);
        } else {
            // The folder is a menu: open it in Explorer or VS Code, start a terminal or agent there.
            let cwd = crate::sessions::display_path(&view.cwd, cx.sessions_root);
            let g = kit::elided(&p, &cwd, ui_font(11.5), theme.text_faint, cwd_room - 8.0);
            let prect = Rect::from_min_size(pos2(lx - 4.0, cy - 11.0), vec2(g.size().x + 8.0, 22.0));
            let presp = ui.interact(prect, ui.scope_id().with("path"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(view.cwd.display().to_string());
            presp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Folder {cwd}")));
            if presp.hovered() {
                p.rect_filled(prect, CornerRadius::same(5), theme.surface2);
            }
            p.galley(pos2(lx, cy - g.size().y / 2.0), g, if presp.hovered() { theme.text_dim } else { theme.text_faint });
            egui::Popup::menu(&presp).width(230.0).show(|ui| {
                let items = [
                    (icon::FOLDER, "Open in File Explorer", FolderAction::Reveal),
                    (icon::CODE, "Open in VS Code", FolderAction::VsCode),
                    (icon::TERMINAL, "New terminal here", FolderAction::Terminal),
                    (icon::BOT, "New session in this folder", FolderAction::NewAgent),
                    (icon::COPY, "Copy path", FolderAction::CopyPath),
                ];
                for (glyph, label, action) in items {
                    if ui.button(format!("{glyph}   {label}")).clicked() {
                        out.push(AgentAction::Folder(action));
                        ui.close();
                    }
                }
            });
        }
    }
}

/// Bay body: transcript + composer, with an optional dock. Returns the rect reserved for an
/// active MCP App view (so the app can position its webview), if any.
pub fn body(ui: &mut Ui, view: &mut AgentView, rect: Rect, cx: &AgentCtx, out: &mut Vec<AgentAction>) -> Option<(u64, Rect)> {
    let theme = cx.theme;
    // Tabs: canvases, then MCP apps, then the session's files (once it has a folder).
    let has_folder = !view.deferred;
    let files_tab = view.canvases.len() + view.apps.len();
    let dock_count = files_tab + usize::from(has_folder);
    if dock_count == 0 {
        view.dock_open = false;
    }
    let (main, dock) = if view.dock_open && rect.width() > 520.0 {
        let w = (rect.width() * 0.44).clamp(300.0, 720.0);
        (Rect::from_min_max(rect.min, pos2(rect.right() - w, rect.bottom())), Some(Rect::from_min_max(pos2(rect.right() - w, rect.top()), rect.max)))
    } else {
        (rect, None)
    };

    let col = column(main);
    if view.items.is_empty() {
        hero_ui(ui, view, main, col, cx, out);
    } else {
        let composer_h = composer_height(ui, view, col.width(), false);
        let crect = Rect::from_min_max(pos2(col.left(), main.bottom() - 12.0 - composer_h), pos2(col.right(), main.bottom() - 12.0));
        let transcript = Rect::from_min_max(main.min, pos2(main.right(), crect.top() - 6.0));
        ui.scope_builder(UiBuilder::new().max_rect(transcript.shrink2(vec2(0.0, 2.0))).id_salt(("transcript", view.bay)), |ui| {
            transcript_ui(ui, view, cx, out);
        });
        ui.scope_builder(UiBuilder::new().max_rect(crect).id_salt(("composer", view.bay)), |ui| {
            composer_ui(ui, view, cx, out, crect, false);
        });
    }

    let mut app_rect = None;
    if let Some(dock) = dock {
        let p = ui.painter();
        p.vline(dock.left() + 0.5, dock.y_range(), Stroke::new(1.0, theme.border));
        let inner = dock.shrink2(vec2(10.0, 8.0));
        ui.scope_builder(UiBuilder::new().max_rect(inner).id_salt(("dock", view.bay)), |ui| {
            // Tabs.
            ui.horizontal_wrapped(|ui| {
                let mut idx = 0;
                for c in &view.canvases {
                    if ui.selectable_label(view.dock_tab == idx, format!("{}  {}", c.icon(), c.title())).clicked() {
                        view.dock_tab = idx;
                    }
                    idx += 1;
                }
                for a in &view.apps {
                    if ui.selectable_label(view.dock_tab == idx, format!("{}  {}", icon::GLOBE, a.title)).clicked() {
                        view.dock_tab = idx;
                    }
                    idx += 1;
                }
                if has_folder && ui.selectable_label(view.dock_tab == files_tab, format!("{}  Files", icon::FOLDER)).clicked() {
                    view.dock_tab = files_tab;
                }
            });
            view.dock_tab = view.dock_tab.min(dock_count.saturating_sub(1));
            ui.add_space(4.0);
            if view.dock_tab < view.canvases.len() {
                let id = Id::unique(("canvas", view.bay, view.canvases[view.dock_tab].instance_id.clone()));
                let c = &mut view.canvases[view.dock_tab];
                if let Some(action) = c.show(ui, theme, id) {
                    match action {
                        CanvasUiAction::SendToAgent(text) => out.push(AgentAction::Send(text)),
                        CanvasUiAction::Copy(text) => out.push(AgentAction::Copy(text)),
                        CanvasUiAction::OpenUrl(u) => out.push(AgentAction::OpenUrl(u)),
                        CanvasUiAction::Close => out.push(AgentAction::CloseCanvas(c.instance_id.clone())),
                    }
                }
            } else if let Some(app) = view.apps.get(view.dock_tab - view.canvases.len()) {
                let r = ui.available_rect_before_wrap();
                ui.allocate_rect(r, Sense::hover());
                app_rect = Some((app.view, r));
            } else if has_folder && let Some(a) = super::files::files_ui(ui, &mut view.files_state, &view.cwd, cx.sessions_root, &view.files, theme) {
                out.push(AgentAction::File(a));
            }
        });
    }
    app_rect
}

/// Widest the conversation and composer get: a readable column, centered, like a chat app.
const COLUMN: f32 = 860.0;

/// The conversation column inside `area`.
fn column(area: Rect) -> Rect {
    let w = (area.width() - 24.0).clamp(0.0, COLUMN);
    Rect::from_min_max(pos2(area.center().x - w / 2.0, area.top()), pos2(area.center().x + w / 2.0, area.bottom()))
}

fn composer_height(ui: &Ui, view: &AgentView, width: f32, hero: bool) -> f32 {
    let font = ui_font(14.0);
    let (lines, row_h) = ui.fonts_mut(|f| {
        let galley = f.layout(view.composer.clone(), font.clone(), Color32::WHITE, (width - 32.0).max(100.0));
        (galley.rows.len().max(1), f.row_height(&font))
    });
    let rows = lines.clamp(if hero { 2 } else { 1 }, 10) as f32;
    let attach = if view.attachments.is_empty() { 0.0 } else { 36.0 };
    // Padding, attachments, the message, then the toolbar.
    12.0 + attach + rows * row_h + 6.0 + 42.0
}

fn transcript_ui(ui: &mut Ui, view: &mut AgentView, cx: &AgentCtx, out: &mut Vec<AgentAction>) {
    let theme = cx.theme;
    let width = ui.available_width();
    if (view.layout_width - width).abs() > 1.0 {
        view.heights.clear();
        view.layout_width = width;
    }
    view.heights.resize(view.items.len(), 48.0);

    let mut md = MdOut { open_url: None, copy: None };
    let mut area = egui::ScrollArea::vertical().id_salt(("scroll", view.bay)).stick_to_bottom(true).auto_shrink([false; 2]);
    if std::mem::take(&mut view.scroll_to_end) {
        area = area.vertical_scroll_offset(f32::MAX);
    }
    let output = area.show_viewport(ui, |ui, viewport| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let margin = 400.0;
            let mut y = 0.0;
            let n = view.items.len();
            let full = ui.available_width();
            let colw = (full - 24.0).clamp(0.0, COLUMN);
            let lead = (full - colw) / 2.0 + 2.0;
            let inner_w = colw - 4.0;
            for i in 0..n {
                let h = view.heights[i];
                let pending = view.items[i].pending();
                let visible = y + h >= viewport.min.y - margin && y <= viewport.max.y + margin;
                if !visible && !pending {
                    ui.allocate_space(vec2(inner_w, h));
                } else {
                    let top = ui.cursor().top();
                    ui.horizontal_top(|ui| {
                        ui.add_space(lead);
                        ui.vertical(|ui| {
                            ui.set_width(inner_w);
                            ui.add_space(6.0);
                            item_ui(ui, view, i, cx, out, &mut md);
                            ui.add_space(6.0);
                        });
                    });
                    view.heights[i] = (ui.cursor().top() - top).max(1.0);
                }
                y += view.heights[i];
            }
            if matches!(view.status, Status::Thinking | Status::Compacting) {
                thinking_row(ui, view, cx, lead);
            }
            ui.add_space(8.0);
        });
    // Scrolled up: a round "latest" button brings you back (and re-engages following).
    let r = output.inner_rect;
    if output.state.clamped_offset().y + r.height() < output.content_size.y - 32.0 {
        let b = Rect::from_center_size(pos2(r.center().x, r.bottom() - 26.0), vec2(34.0, 34.0));
        let resp = ui.interact(b, Id::unique(("to-latest", view.bay)), Sense::click()).on_hover_text("Scroll to the latest").on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Scroll to the latest"));
        let p = ui.painter();
        p.circle(b.center(), 17.0, if resp.hovered() { theme.surface3 } else { theme.surface2 }, Stroke::new(1.0, theme.border_strong));
        p.text(b.center(), Align2::CENTER_CENTER, icon::CHEVRON_DOWN, icon_font(11.0), theme.text);
        if resp.clicked() {
            view.scroll_to_end = true;
            ui.ctx().request_repaint();
        }
    }
    if let Some(u) = md.open_url {
        out.push(AgentAction::OpenUrl(u));
    }
    if let Some(c) = md.copy {
        out.push(AgentAction::Copy(c));
    }
    let _ = theme;
}

const SUGGESTIONS: [&str; 4] = ["Explain how this codebase is organized", "Find a bug and fix it", "Write tests for the recent changes", "Summarize the last 10 commits"];

/// Starters for a session in its own, empty folder.
const FRESH_SUGGESTIONS: [&str; 4] = [
    "Build a to-do app as a single HTML file",
    "Write a Python script that charts a CSV I attach",
    "Research what's new in Rust this year and summarize it",
    "Draft a project plan as a Markdown checklist",
];

/// A new conversation, like a chat app's home screen: a greeting, the composer in the middle, and
/// a few starters that fill it in.
fn hero_ui(ui: &mut Ui, view: &mut AgentView, main: Rect, col: Rect, cx: &AgentCtx, out: &mut Vec<AgentAction>) {
    let theme = cx.theme;
    let booting = view.status == Status::Booting;
    let composer_h = composer_height(ui, view, col.width(), true);
    let show_starters = !booting && main.height() > 420.0;
    let block = 60.0 + composer_h + if show_starters { 120.0 } else { 36.0 };
    // A little above center, where the eye lands.
    let top = (main.center().y - block / 2.0 - main.height() * 0.05).max(main.top() + 12.0);
    let p = ui.painter().clone();

    let text = if booting { "Starting Copilot…".to_owned() } else { greeting(view) };
    let g = kit::elided(&p, &text, semibold(if col.width() < 520.0 { 20.0 } else { 26.0 }), theme.text, (col.width() - 56.0).max(40.0));
    let logo = (g.size().y + 6.0).round();
    let cy = top + 22.0;
    let x0 = col.center().x - (logo + 12.0 + g.size().x) / 2.0;
    kit::mark(&p, pos2(x0 + logo / 2.0, cy), logo);
    p.galley(pos2(x0 + logo + 12.0, cy - g.size().y / 2.0), g, theme.text);

    let crect = Rect::from_min_size(pos2(col.left(), top + 60.0), vec2(col.width(), composer_h));
    ui.scope_builder(UiBuilder::new().max_rect(crect).id_salt(("composer", view.bay)), |ui| {
        composer_ui(ui, view, cx, out, crect, true);
    });

    let mut y = crect.bottom() + 16.0;
    if booting {
        kit::spinner(&p, pos2(col.center().x, y + 10.0), 7.0, cx.t, theme.accent);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        return;
    }
    if show_starters {
        y = starters(ui, view, col, y, theme) + 6.0;
    }
    let place = crate::sessions::display_path(&view.cwd, cx.sessions_root);
    let place = if view.deferred { format!("Gets its own folder in {place}") } else { format!("Working in {place}") };
    let notes = [format!("{place}  ·  / for commands  ·  drop files to attach"), place];
    if let Some(note) = notes.iter().find(|n| p.layout_no_wrap(n.to_string(), ui_font(11.5), theme.text_faint).size().x < col.width()) {
        p.text(pos2(col.center().x, y + 10.0), Align2::CENTER_CENTER, note, ui_font(11.5), theme.text_faint);
    }
}

fn greeting(view: &AgentView) -> String {
    use chrono::Timelike;
    let part = match chrono::Local::now().hour() {
        5..=11 => "Morning",
        12..=16 => "Afternoon",
        17..=21 => "Evening",
        _ => "Late night",
    };
    if view.deferred {
        format!("{part}, what are we building?")
    } else {
        format!("{part}, what's next for {}?", crate::settings::dir_name(&view.cwd))
    }
}

/// Starter prompts under the composer, in centered rows. Picking one fills in the composer, ready
/// to edit or send. Returns the bottom of the last row.
fn starters(ui: &mut Ui, view: &mut AgentView, col: Rect, mut y: f32, theme: &Theme) -> f32 {
    let p = ui.painter().clone();
    let font = ui_font(12.5);
    let items: Vec<(&str, f32)> = (if view.deferred { FRESH_SUGGESTIONS } else { SUGGESTIONS })
        .iter()
        .map(|s| (*s, p.layout_no_wrap(s.to_string(), font.clone(), theme.text).size().x + 28.0))
        .collect();
    let mut rows: Vec<Vec<(&str, f32)>> = vec![Vec::new()];
    let mut row_w = 0.0;
    for (s, w) in items {
        if row_w + w > col.width() && !rows.last().is_none_or(Vec::is_empty) {
            rows.push(Vec::new());
            row_w = 0.0;
        }
        row_w += w + 8.0;
        if let Some(row) = rows.last_mut() {
            row.push((s, w));
        }
    }
    for row in rows {
        let total: f32 = row.iter().map(|(_, w)| w + 8.0).sum::<f32>() - 8.0;
        let mut x = col.center().x - total / 2.0;
        for (s, w) in row {
            let chip = Rect::from_min_size(pos2(x, y), vec2(w, 30.0));
            let resp = ui.interact(chip, Id::unique(("suggest", view.bay, s)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, s));
            let hovered = resp.hovered();
            p.rect(chip, CornerRadius::same(15), if hovered { theme.surface3 } else { theme.surface }, Stroke::new(1.0, if hovered { theme.border_strong } else { theme.border }), StrokeKind::Inside);
            p.text(chip.center(), Align2::CENTER_CENTER, s, font.clone(), if hovered { theme.text } else { theme.text_dim });
            if resp.clicked() {
                view.composer = s.to_owned();
                view.focus_composer = true;
                set_cursor_end(ui.ctx(), Id::unique(("composer-edit", view.bay)), &view.composer);
            }
            x += w + 8.0;
        }
        y += 38.0;
    }
    y
}

/// Live "the agent is working" row shown until text or a tool call arrives.
fn thinking_row(ui: &mut Ui, view: &AgentView, cx: &AgentCtx, lead: f32) {
    let theme = cx.theme;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(lead);
        let (r, _) = ui.allocate_exact_size(vec2(30.0, 22.0), Sense::hover());
        for k in 0..3 {
            let phase = cx.t * 5.0 - k as f32 * 0.7;
            let a = 0.25 + 0.75 * (0.5 + 0.5 * phase.sin());
            ui.painter().circle_filled(pos2(r.left() + 5.0 + k as f32 * 8.0, r.center().y), 2.6, with_alpha(theme.accent, a));
        }
        let label = match (&view.status, &view.intent) {
            (Status::Compacting, _) => "Compacting the conversation".to_owned(),
            (_, Some(intent)) => intent.clone(),
            _ => "Thinking".to_owned(),
        };
        ui.label(RichText::new(label).font(ui_font(12.5)).color(theme.text_dim));
        if let Some(since) = view.busy_since {
            let secs = since.elapsed().as_secs();
            if secs >= 2 {
                ui.label(RichText::new(format!("{secs}s")).font(ui_font(11.5)).color(theme.text_faint));
            }
        }
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}

fn card(ui: &mut Ui, theme: &Theme, accent: Option<Color32>, add: impl FnOnce(&mut Ui)) {
    let frame = Frame::new()
        .fill(theme.surface2)
        .stroke(Stroke::new(1.0, accent.map(|a| mix(theme.border, a, 0.55)).unwrap_or(theme.border)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 10));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

fn json_preview(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Object(m) if m.is_empty() => String::new(),
        _ => serde_json::to_string_pretty(v).unwrap_or_default(),
    }
}

fn mono_box(ui: &mut Ui, text: &str, theme: &Theme, max_h: f32, id: Id, follow: bool) {
    Frame::new().fill(theme.surface).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt(id).max_height(max_h).stick_to_bottom(follow).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).font(mono(12.0)).color(theme.text_dim)).wrap());
        });
    });
}

fn diff_box(ui: &mut Ui, diff: &str, theme: &Theme, id: Id) {
    Frame::new().fill(theme.surface).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt(id).max_height(280.0).show(ui, |ui| {
            let mut job = egui::text::LayoutJob::default();
            for line in diff.lines().take(800) {
                let (color, bg) = if line.starts_with('+') && !line.starts_with("+++") {
                    (theme.ok, with_alpha(theme.ok, 0.10))
                } else if line.starts_with('-') && !line.starts_with("---") {
                    (theme.err, with_alpha(theme.err, 0.10))
                } else if line.starts_with("@@") {
                    (theme.accent, Color32::TRANSPARENT)
                } else {
                    (theme.text_dim, Color32::TRANSPARENT)
                };
                let mut fmt = egui::TextFormat::simple(mono(12.0), color);
                fmt.background = bg;
                job.append(line, 0.0, fmt.clone());
                job.append("\n", 0.0, fmt);
            }
            ui.add(egui::Label::new(job).extend());
        });
    });
}

fn item_ui(ui: &mut Ui, view: &mut AgentView, i: usize, cx: &AgentCtx, out: &mut Vec<AgentAction>, md: &mut MdOut) {
    let theme = cx.theme;
    let bay = view.bay;
    let mut resolved = false;
    let mut approval: Option<(Vec<String>, bool)> = None;
    // Message actions (copy, edit, try again) show on hover, and always on the latest reply.
    let latest = !view.items[i + 1..].iter().any(|x| matches!(x, Item::Assistant { .. } | Item::User { .. }));
    let retry = (latest && !view.status.is_busy()).then(|| view.items[..i].iter().rev().find_map(|x| if let Item::User { text, .. } = x { Some(text.clone()) } else { None })).flatten();
    let hover_id = Id::unique(("message-hover", bay, i));
    let hovered = ui.ctx().data(|d| d.get_temp::<Rect>(hover_id)).is_some_and(|r| ui.rect_contains_pointer(r));
    let top = ui.cursor().top();
    match &mut view.items[i] {
        Item::User { text, queued, files } => {
            Frame::new().fill(theme.accent_soft()).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.set_height(18.0);
                    ui.label(RichText::new("You").font(semibold(11.5)).color(theme.text_dim));
                    if *queued {
                        ui.label(RichText::new("· queued").font(ui_font(11.5)).color(theme.text_faint));
                    }
                    if hovered {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if kit::icon_button_sized(ui, icon::COPY, "Copy", theme, 20.0, false).clicked() {
                                out.push(AgentAction::Copy(text.clone()));
                            }
                            if kit::icon_button_sized(ui, icon::EDIT, "Edit and resend", theme, 20.0, false).clicked() {
                                out.push(AgentAction::EditPrompt(text.clone()));
                            }
                        });
                    }
                });
                ui.add(egui::Label::new(RichText::new(text.as_str()).color(theme.text)).wrap());
                if !files.is_empty() {
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                        for f in files.iter() {
                            kit::file_pill(ui, f, theme, false);
                        }
                    });
                }
            });
        }
        Item::Assistant { text, done } => {
            markdown::render(ui, text, theme, md);
            if !*done {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
                let a = 0.4 + 0.6 * (cx.t * 6.0).sin().abs();
                ui.painter().rect_filled(Rect::from_min_size(r.min + vec2(0.0, 2.0), vec2(7.0, 12.0)), CornerRadius::same(1), with_alpha(theme.accent, a));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
            } else if !text.trim().is_empty() {
                // Space is reserved so the transcript doesn't jump when the actions appear.
                ui.horizontal(|ui| {
                    ui.set_height(24.0);
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if hovered || latest {
                        if kit::icon_button_sized(ui, icon::COPY, "Copy", theme, 24.0, false).clicked() {
                            out.push(AgentAction::Copy(text.clone()));
                        }
                        if let Some(prompt) = &retry
                            && kit::icon_button_sized(ui, icon::REFRESH, "Try again", theme, 24.0, false).clicked()
                        {
                            out.push(AgentAction::Send(prompt.clone()));
                        }
                    }
                });
            }
        }
        Item::Reasoning { text, done, open } => {
            if !cx.show_reasoning {
                return;
            }
            let label = if *done { "Thought process" } else { "Thinking…" };
            let resp = ui
                .horizontal(|ui| {
                    ui.label(RichText::new(if *open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT }).font(icon_font(9.0)).color(theme.text_faint));
                    ui.label(RichText::new(label).font(ui_font(12.5)).italics().color(theme.text_dim));
                })
                .response
                .interact(Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            if resp.clicked() {
                *open = !*open;
            }
            if *open {
                ui.add(egui::Label::new(RichText::new(text.trim()).font(ui_font(12.5)).color(theme.text_faint)).wrap());
            }
        }
        Item::Tool(card) => {
            let (glyph, color) = match card.state {
                ToolState::Running => (tool_icon(&card.name), theme.accent),
                ToolState::Ok => (icon::CHECK, theme.ok),
                ToolState::Failed => (icon::ERROR, theme.err),
            };
            let target = tool_target(&card.name, &card.args);
            let title = super::tool_label(card, target.as_deref());
            let target = target.filter(|t| !title.to_lowercase().contains(&t.to_lowercase()));
            let (row_rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &title));
            if resp.hovered() {
                ui.painter().rect_filled(row_rect.expand2(vec2(4.0, 1.0)), CornerRadius::same(4), with_alpha(theme.text, 0.03));
            }
            let p = ui.painter();
            let cy = row_rect.center().y;
            let icon_c = pos2(row_rect.left() + 8.0, cy);
            if card.state == ToolState::Running {
                kit::spinner(p, icon_c, 5.5, cx.t, theme.accent);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            } else if glyph.chars().next().is_some_and(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)) {
                p.text(icon_c, Align2::CENTER_CENTER, glyph, icon_font(11.0), color);
            } else {
                p.text(icon_c, Align2::CENTER_CENTER, glyph, ui_font(12.0), color);
            }
            // Right edge: elapsed time, then live progress while running.
            let secs = card.elapsed.unwrap_or_else(|| card.started.elapsed().as_secs_f32());
            let er = p.text(pos2(row_rect.right(), cy), Align2::RIGHT_CENTER, format!("{secs:.1}s"), ui_font(11.0), theme.text_faint);
            let mut right = er.left() - 10.0;
            let left = row_rect.left() + 22.0;
            if let (Some(prog), ToolState::Running) = (&card.progress, card.state) {
                let g = kit::elided(p, prog, ui_font(11.5), theme.text_dim, ((right - left) * 0.3).max(40.0));
                right -= g.size().x;
                p.galley(pos2(right, cy - g.size().y / 2.0), g, theme.text_dim);
                right -= 10.0;
            }
            // Left: title, then target, server and UI badge as space allows.
            let suffix: Vec<(String, egui::FontId, Color32)> = card
                .mcp_server
                .iter()
                .map(|s| (format!("· {s}"), ui_font(11.5), theme.text_faint))
                .chain(card.has_app.then(|| ("· UI".to_owned(), semibold(11.0), theme.accent)))
                .collect();
            let suffix_w: f32 = suffix.iter().map(|(t, f, c)| p.layout_no_wrap(t.clone(), f.clone(), *c).size().x + 6.0).sum();
            let avail = (right - left - suffix_w).max(40.0);
            let title_g = kit::elided(p, &title, ui_font(13.0), theme.text, if target.is_some() { (avail * 0.7).max(avail - 260.0) } else { avail });
            let mut x = left;
            let tw = title_g.size().x;
            p.galley(pos2(x, cy - title_g.size().y / 2.0), title_g, theme.text);
            x += tw + 8.0;
            if let Some(t) = &target {
                let room = right - suffix_w - x;
                if room > 30.0 {
                    let g = kit::elided(p, t, mono(12.0), theme.text_faint, room);
                    let gw = g.size().x;
                    p.galley(pos2(x, cy - g.size().y / 2.0), g, theme.text_faint);
                    x += gw + 6.0;
                }
            }
            for (t, f, c) in suffix {
                if x + 20.0 < right {
                    let r = p.text(pos2(x, cy), Align2::LEFT_CENTER, t, f, c);
                    x = r.right() + 6.0;
                }
            }
            if resp.clicked() {
                card.expanded = !card.expanded;
            }
            let show_live = card.state == ToolState::Running && !card.output.is_empty();
            if card.expanded || show_live {
                ui.add_space(4.0);
                let args = json_preview(&card.args);
                if card.expanded && !args.is_empty() {
                    mono_box(ui, &args, theme, 160.0, Id::unique(("args", bay, i)), false);
                    ui.add_space(4.0);
                }
                let body = if !card.output.is_empty() && (card.state == ToolState::Running || card.result.is_none()) {
                    Some(card.output.as_str())
                } else {
                    card.result.as_deref()
                };
                if let Some(text) = body.filter(|t| !t.trim().is_empty()) {
                    mono_box(ui, text, theme, if card.expanded { 260.0 } else { 120.0 }, Id::unique(("out", bay, i)), card.state == ToolState::Running);
                }
                if let Some(err) = &card.error {
                    ui.label(RichText::new(err).font(ui_font(12.0)).color(theme.err));
                }
            }
        }
        Item::Permission(p) => {
            let raw = p.raw.clone();
            let pr = raw.get("promptRequest").cloned().unwrap_or_else(|| raw.get("permissionRequest").cloned().unwrap_or(Value::Null));
            let kind = pr.get("kind").and_then(|v| v.as_str()).unwrap_or("tool").to_owned();
            let intention = pr.get("intention").and_then(|v| v.as_str()).map(str::to_owned);
            let graysense_tool = kind == "custom-tool" && pr.get("toolName").and_then(Value::as_str).is_some_and(|t| t.starts_with("graysense_"));
            let what = match kind.as_str() {
                _ if graysense_tool => "Change Graysense",
                "commands" => "Run a command",
                "write" => "Edit a file",
                "read" => "Read a file",
                "url" => "Fetch a URL",
                "mcp" => "Use an MCP tool",
                "custom-tool" => "Use a custom tool",
                "path" => "Access a path",
                "memory" => "Save a memory",
                _ => "Use a tool",
            };
            if let Some(outcome) = &p.outcome {
                let summary = intention.unwrap_or_else(|| match pr.get("toolName").and_then(Value::as_str) {
                    Some(tool) => {
                        let target = pr.get("args").and_then(|a| tool_target(tool, a));
                        format!("{}{}", super::tool_verb(tool), target.map(|t| format!(" · {t}")).unwrap_or_default())
                    }
                    None => what.to_owned(),
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SHIELD).font(icon_font(11.0)).color(theme.text_faint));
                    ui.label(RichText::new(format!("{summary} · {outcome}")).font(ui_font(12.5)).color(theme.text_dim));
                });
                return;
            }
            card(ui, theme, Some(theme.warn), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SHIELD).font(icon_font(13.0)).color(theme.warn));
                    ui.label(RichText::new(format!("Approval needed · {what}")).font(semibold(13.0)).color(theme.text));
                });
                if let Some(int) = &intention {
                    ui.label(RichText::new(int).color(theme.text_dim));
                }
                ui.add_space(4.0);
                let s = |k: &str| pr.get(k).and_then(|v| v.as_str()).map(str::to_owned);
                match kind.as_str() {
                    "commands" => {
                        if let Some(cmd) = s("fullCommandText") {
                            mono_box(ui, &cmd, theme, 140.0, Id::unique(("perm-cmd", bay, i)), false);
                        }
                        if let Some(w) = s("warning") {
                            ui.label(RichText::new(w).color(theme.warn).font(ui_font(12.0)));
                        }
                    }
                    "write" => {
                        if let Some(f) = s("fileName") {
                            ui.label(RichText::new(f).font(mono(12.0)).color(theme.text));
                        }
                        if let Some(d) = s("diff").filter(|d| !d.is_empty()) {
                            diff_box(ui, &d, theme, Id::unique(("perm-diff", bay, i)));
                        }
                    }
                    "read" | "path" => {
                        let paths: Vec<String> = s("path")
                            .map(|p| vec![p])
                            .or_else(|| pr.get("paths").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()))
                            .unwrap_or_default();
                        for p in paths {
                            ui.add(egui::Label::new(RichText::new(p).font(mono(12.0)).color(theme.text)).wrap());
                        }
                    }
                    "url" => {
                        ui.label(RichText::new(s("url").unwrap_or_default()).font(mono(12.0)).color(theme.accent));
                    }
                    "mcp" => {
                        ui.label(RichText::new(format!("{} › {}", s("serverName").unwrap_or_default(), s("toolTitle").or_else(|| s("toolName")).unwrap_or_default())).font(mono(12.0)));
                        let args = pr.get("args").map(json_preview).unwrap_or_default();
                        if !args.is_empty() {
                            mono_box(ui, &args, theme, 140.0, Id::unique(("perm-args", bay, i)), false);
                        }
                    }
                    "custom-tool" => {
                        let tool = s("toolName").unwrap_or_default();
                        let args = pr.get("args").cloned().unwrap_or(Value::Null);
                        let headline = match tool.as_str() {
                            "open_canvas" => {
                                let canvas = args.get("canvasId").and_then(Value::as_str).unwrap_or("canvas");
                                let title = args.get("input").and_then(|i| i.get("title")).and_then(Value::as_str);
                                format!("Open {canvas} canvas{}", title.map(|t| format!(" · {t}")).unwrap_or_default())
                            }
                            "invoke_canvas_action" => format!("Run canvas action · {}", args.get("actionName").and_then(Value::as_str).unwrap_or("?")),
                            "graysense_settings" => "Change Graysense settings".to_owned(),
                            "graysense_mcp_servers" | "graysense_plugins" => {
                                let action = args.get("action").and_then(Value::as_str).unwrap_or("change");
                                let what = if tool == "graysense_plugins" { "plugin" } else { "MCP server" };
                                let name = args.get("name").or_else(|| args.get("source")).and_then(Value::as_str).unwrap_or_default();
                                format!("{} {what} {name}", super::tool_verb(action))
                            }
                            _ => super::tool_verb(&tool),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(headline).font(semibold(12.5)).color(theme.text));
                            ui.label(RichText::new(&tool).font(mono(11.5)).color(theme.text_faint));
                        });
                        if let Some(d) = s("toolDescription").filter(|_| !tool.starts_with("graysense_")) {
                            ui.label(RichText::new(d).font(ui_font(12.0)).color(theme.text_dim));
                        }
                        let changes = args.get("changes").and_then(Value::as_object).filter(|_| tool == "graysense_settings");
                        let args = match changes {
                            Some(ch) => ch.iter().map(|(k, v)| format!("{k}  →  {}", if v.is_null() { "default".to_owned() } else { v.to_string() })).collect::<Vec<_>>().join("\n"),
                            None => json_preview(&args),
                        };
                        if !args.is_empty() {
                            mono_box(ui, &args, theme, 140.0, Id::unique(("perm-args", bay, i)), false);
                        }
                    }
                    _ => {
                        let preview = json_preview(&pr);
                        if !preview.is_empty() {
                            mono_box(ui, &preview, theme, 120.0, Id::unique(("perm-raw", bay, i)), false);
                        }
                    }
                }
                ui.add_space(6.0);
                let offer_session = super::backend::can_offer_session(&raw);
                let commands: Vec<String> = if kind == "commands" {
                    pr.get("commandIdentifiers").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
                } else {
                    Vec::new()
                };
                ui.horizontal(|ui| {
                    if kit::primary_button(ui, "Allow", theme).clicked() {
                        if let Some(tx) = p.reply.take() {
                            let _ = tx.send(PermDecision::Once);
                        }
                        p.outcome = Some("allowed".into());
                        resolved = true;
                        approval = Some((commands.clone(), true));
                    }
                    if offer_session && kit::ghost_button(ui, "Allow for session", theme).clicked() {
                        if let Some(tx) = p.reply.take() {
                            let _ = tx.send(PermDecision::Session);
                        }
                        p.outcome = Some("allowed for this session".into());
                        resolved = true;
                        approval = Some((commands.clone(), true));
                    }
                    if kit::tinted_button(ui, "Deny", theme, theme.err).clicked() {
                        let fb = (!p.feedback.trim().is_empty()).then(|| p.feedback.clone());
                        if let Some(tx) = p.reply.take() {
                            let _ = tx.send(PermDecision::Reject(fb));
                        }
                        p.outcome = Some("denied".into());
                        resolved = true;
                        approval = Some((commands.clone(), false));
                    }
                    ui.add(TextEdit::singleline(&mut p.feedback).hint_text("Tell Copilot what to do instead…").desired_width(f32::INFINITY));
                });
            });
        }
        Item::Elicit(e) => {
            if let Some(outcome) = &e.outcome {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::EDIT).font(icon_font(11.0)).color(theme.text_faint));
                    ui.label(RichText::new(format!("{} → {}", trunc_line(&e.message), outcome)).font(ui_font(12.5)).color(theme.text_dim));
                });
                return;
            }
            card(ui, theme, Some(theme.accent), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::EDIT).font(icon_font(12.0)).color(theme.accent));
                    let src = e.source.clone().map(|s| format!(" · {s}")).unwrap_or_default();
                    ui.label(RichText::new(format!("Input requested{src}")).font(semibold(13.0)));
                });
                markdown::render(ui, &e.message, theme, md);
                ui.add_space(4.0);
                if let Some(url) = e.url.clone() {
                    ui.label(RichText::new(&url).font(mono(12.0)).color(theme.accent));
                    ui.horizontal(|ui| {
                        if kit::primary_button(ui, "Open link", theme).clicked() {
                            out.push(AgentAction::OpenUrl(url.clone()));
                        }
                        if kit::ghost_button(ui, "Done", theme).clicked() {
                            if let Some(tx) = e.reply.take() {
                                let _ = tx.send(github_copilot_sdk::ElicitationResult { action: "accept".into(), content: None });
                            }
                            e.outcome = Some("completed".into());
                            resolved = true;
                        }
                        if kit::ghost_button(ui, "Cancel", theme).clicked() {
                            if let Some(tx) = e.reply.take() {
                                let _ = tx.send(github_copilot_sdk::ElicitationResult { action: "cancel".into(), content: None });
                            }
                            e.outcome = Some("cancelled".into());
                            resolved = true;
                        }
                    });
                    return;
                }
                let picked = e.form.show(ui, theme, Id::unique(("form", bay, i)));
                let quick = e.form.is_single_choice();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let submit = if quick { picked } else { kit::primary_button(ui, "Submit", theme).clicked() };
                    if submit
                        && let Some(content) = e.form.to_json() {
                            let summary = e.form.summary(&content);
                            if let Some(tx) = e.reply.take() {
                                let _ = tx.send(github_copilot_sdk::ElicitationResult { action: "accept".into(), content: Some(content) });
                            }
                            e.outcome = Some(summary);
                            resolved = true;
                        }
                    if kit::ghost_button(ui, "Decline", theme).clicked() {
                        if let Some(tx) = e.reply.take() {
                            let _ = tx.send(github_copilot_sdk::ElicitationResult { action: "decline".into(), content: None });
                        }
                        e.outcome = Some("declined".into());
                        resolved = true;
                    }
                });
            });
        }
        Item::Ask(a) => {
            if let Some(outcome) = &a.outcome {
                ui.label(RichText::new(format!("{} → {}", trunc_line(&a.question), outcome)).font(ui_font(12.5)).color(theme.text_dim));
                return;
            }
            card(ui, theme, Some(theme.accent), |ui| {
                ui.label(RichText::new("Copilot is asking").font(semibold(12.0)).color(theme.accent));
                markdown::render(ui, &a.question, theme, md);
                ui.add_space(4.0);
                let mut answer: Option<(String, bool)> = None;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for c in &a.choices {
                        if kit::chip(ui, false, c, theme, false).clicked() {
                            answer = Some((c.clone(), false));
                        }
                    }
                });
                if a.freeform {
                    ui.horizontal(|ui| {
                        let r = ui.add(TextEdit::singleline(&mut a.answer).hint_text("Type an answer…").desired_width(ui.available_width() - 70.0));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if (kit::primary_button(ui, "Send", theme).clicked() || enter) && !a.answer.trim().is_empty() {
                            answer = Some((a.answer.clone(), true));
                        }
                    });
                }
                if let Some((text, free)) = answer {
                    if let Some(tx) = a.reply.take() {
                        let _ = tx.send(Some((text.clone(), free)));
                    }
                    a.outcome = Some(text);
                    resolved = true;
                }
            });
        }
        Item::Plan(p) => {
            if let Some(outcome) = &p.outcome {
                ui.label(RichText::new(format!("Plan · {outcome}")).font(ui_font(12.5)).color(theme.text_dim));
                return;
            }
            card(ui, theme, Some(theme.accent), |ui| {
                ui.label(RichText::new("Plan ready for review").font(semibold(13.0)));
                markdown::render(ui, &p.summary, theme, md);
                if let Some(plan) = &p.plan {
                    egui::CollapsingHeader::new("Full plan").id_salt(("plan", bay, i)).show(ui, |ui| {
                        markdown::render(ui, plan, theme, md);
                    });
                }
                ui.add(TextEdit::multiline(&mut p.feedback).hint_text("Optional feedback…").desired_rows(1).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                let mut choice: Option<(bool, Option<String>)> = None;
                ui.horizontal_wrapped(|ui| {
                    let actions = if p.actions.is_empty() { vec!["interactive".to_owned(), "autopilot".to_owned()] } else { p.actions.clone() };
                    for a in actions {
                        let label = match a.as_str() {
                            "interactive" => "Approve · implement step by step".to_owned(),
                            "autopilot" => "Approve · run on autopilot".to_owned(),
                            "autopilot_fleet" => "Approve · autopilot fleet".to_owned(),
                            other => format!("Approve · {other}"),
                        };
                        let clicked = if a == p.recommended { kit::primary_button(ui, &label, theme).clicked() } else { kit::ghost_button(ui, &label, theme).clicked() };
                        if clicked {
                            choice = Some((true, Some(a)));
                        }
                    }
                    if kit::ghost_button(ui, "Keep planning", theme).clicked() {
                        choice = Some((false, None));
                    }
                });
                if let Some((approved, action)) = choice {
                    let feedback = (!p.feedback.trim().is_empty()).then(|| p.feedback.clone());
                    if let Some(tx) = p.reply.take() {
                        let _ = tx.send(github_copilot_sdk::handler::ExitPlanModeResult { approved, selected_action: action.clone(), feedback });
                    }
                    p.outcome = Some(if approved { format!("approved ({})", action.unwrap_or_default()) } else { "revising".into() });
                    resolved = true;
                }
            });
        }
        Item::Switch(s) => {
            if let Some(outcome) = &s.outcome {
                ui.label(RichText::new(format!("Model fallback · {outcome}")).font(ui_font(12.5)).color(theme.text_dim));
                return;
            }
            card(ui, theme, Some(theme.warn), |ui| {
                ui.label(RichText::new(&s.text).color(theme.text));
                ui.horizontal(|ui| {
                    use github_copilot_sdk::handler::AutoModeSwitchResponse as R;
                    let mut pick = None;
                    if kit::primary_button(ui, "Switch to Auto", theme).clicked() {
                        pick = Some((R::Yes, "switched"));
                    }
                    if kit::ghost_button(ui, "Always", theme).clicked() {
                        pick = Some((R::YesAlways, "always switch"));
                    }
                    if kit::ghost_button(ui, "No", theme).clicked() {
                        pick = Some((R::No, "declined"));
                    }
                    if let Some((r, label)) = pick {
                        if let Some(tx) = s.reply.take() {
                            let _ = tx.send(r);
                        }
                        s.outcome = Some(label.into());
                        resolved = true;
                    }
                });
            });
        }
        Item::Notice { level, text } => {
            let (glyph, color) = match level {
                Level::Info => (icon::INFO, theme.text_dim),
                Level::Ok => (icon::CHECK, theme.ok),
                Level::Warn => (icon::WARNING, theme.warn),
                Level::Error => (icon::ERROR, theme.err),
            };
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(glyph).font(icon_font(11.0)).color(color));
                ui.add(egui::Label::new(RichText::new(text.as_str()).font(ui_font(12.5)).color(if *level == Level::Info { theme.text_dim } else { color })).wrap());
            });
        }
        Item::Recall { entries, open } => {
            let n = entries.len();
            let row = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(icon::MEMORY).font(icon_font(11.0)).color(theme.text_faint));
                ui.label(RichText::new(format!("Recalled {n} memor{}", if n == 1 { "y" } else { "ies" })).font(ui_font(12.0)).color(theme.text_dim));
                ui.label(RichText::new(if *open { icon::CHEVRON_UP } else { icon::CHEVRON_DOWN }).font(icon_font(8.0)).color(theme.text_faint));
            });
            let resp = row.response.interact(Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Notes Graysense attached to this message");
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, *open, format!("Recalled {n} memories")));
            if resp.clicked() {
                *open = !*open;
            }
            if *open {
                Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (_, text) in entries.iter() {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new("•").color(theme.text_faint));
                            ui.add(egui::Label::new(RichText::new(text).font(ui_font(12.0)).color(theme.text_dim)).wrap());
                        });
                    }
                    ui.add_space(2.0);
                    if kit::mini_button(ui, "Open Memory", theme).clicked() {
                        out.push(AgentAction::OpenMemory(entries.first().map(|e| e.0)));
                    }
                });
            }
        }
        Item::Memory { ids, text, learned, undone } => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(icon::MEMORY).font(icon_font(11.0)).color(if *undone { theme.text_faint } else { theme.accent }));
                let verb = match (*undone, *learned) {
                    (true, _) => "Forgotten",
                    (false, true) => "Learned",
                    (false, false) => "Remembered",
                };
                ui.label(RichText::new(verb).font(semibold(12.0)).color(if *undone { theme.text_faint } else { theme.text_dim }));
                let room = (ui.available_width() - 130.0).max(60.0);
                let g = kit::elided(ui.painter(), text, ui_font(12.0), theme.text_dim, room);
                let (r, resp) = ui.allocate_exact_size(g.size(), Sense::hover());
                ui.painter().galley(r.min, g, theme.text_dim);
                resp.on_hover_text(text.as_str());
                if !*undone {
                    if !ids.is_empty() && kit::mini_button(ui, "Undo", theme).clicked() {
                        out.push(AgentAction::ForgetMemories(ids.clone()));
                        *undone = true;
                    }
                    if kit::mini_button(ui, "View", theme).clicked() {
                        out.push(AgentAction::OpenMemory(ids.first().copied()));
                    }
                }
            });
        }
        Item::Subagent(s) => {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                match s.done {
                    None => {
                        kit::spinner(ui.painter(), r.center(), 5.5, cx.t, theme.accent);
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                    }
                    Some(true) => {
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, icon::CHECK, icon_font(11.0), theme.ok);
                    }
                    Some(false) => {
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, icon::ERROR, icon_font(11.0), theme.err);
                    }
                }
                ui.label(RichText::new(icon::BOT).font(icon_font(12.0)).color(theme.text_dim));
                ui.label(RichText::new(&s.name).font(semibold(12.5)));
                ui.label(RichText::new(format!("· {} tool call{}", s.tools, if s.tools == 1 { "" } else { "s" })).font(ui_font(12.0)).color(theme.text_faint));
                if let Some(cur) = &s.current {
                    ui.label(RichText::new(cur).font(mono(11.5)).color(theme.text_faint));
                }
            });
            if !s.description.is_empty() && s.done.is_none() {
                ui.label(RichText::new(trunc_line(&s.description)).font(ui_font(12.0)).color(theme.text_dim));
            }
        }
    }
    if matches!(view.items[i], Item::User { .. } | Item::Assistant { .. }) {
        let r = Rect::from_min_max(pos2(ui.max_rect().left(), top), pos2(ui.max_rect().right(), ui.cursor().top().max(top + 1.0)));
        ui.ctx().data_mut(|d| d.insert_temp(hover_id, r));
    }
    if let Some((cmds, ok)) = approval.filter(|(c, _)| !c.is_empty()) {
        view.approval_log.push((cmds, ok));
    }
    if resolved {
        view.refresh_waiting();
        out.push(AgentAction::Focus);
    }
}

fn trunc_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > 90 { format!("{}…", line.chars().take(90).collect::<String>()) } else { line.to_owned() }
}

const SLASH: [(&str, &str, &str); 5] = [
    ("model", "<name>", "Switch the model for this agent"),
    ("mode", "ask|plan|autopilot", "Change how much the agent does on its own"),
    ("attach", "<path>", "Attach a file or folder to the next message"),
    ("clear", "", "Clear this transcript"),
    ("stop", "", "Stop the current turn"),
];

fn set_cursor_end(ctx: &egui::Context, id: Id, text: &str) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    state.cursor.set_char_range(Some(egui::text_selection::CCursorRange::one(end)));
    state.store(ctx, id);
}

/// Command suggestions floating above the composer while typing `/…`.
fn slash_menu(ui: &mut Ui, view: &mut AgentView, composer: Rect, theme: &Theme, edit_id: Id, has_focus: bool) {
    let Some(query) = view.composer.strip_prefix('/') else { return };
    if query.contains(char::is_whitespace) {
        return;
    }
    let matches: Vec<&(&str, &str, &str)> = SLASH.iter().filter(|(name, _, _)| name.starts_with(query)).collect();
    if matches.is_empty() {
        return;
    }
    let mut pick: Option<&str> = None;
    if has_focus && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
        pick = Some(matches[0].0);
    }
    let w = composer.width().min(460.0);
    let pos = pos2(composer.left(), composer.top() - 6.0);
    egui::Area::new(Id::unique(("slash-menu", view.bay))).order(egui::Order::Foreground).pivot(Align2::LEFT_BOTTOM).fixed_pos(pos).show(ui.ctx(), |ui| {
        Frame::new()
            .fill(theme.surface)
            .stroke(Stroke::new(1.0, theme.border_strong))
            .corner_radius(CornerRadius::same(8))
            .shadow(ui.visuals().popup_shadow)
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(w - 12.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                for (k, (name, args, desc)) in matches.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(w - 12.0, 30.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("/{name}")));
                    let p = ui.painter();
                    if resp.hovered() || k == 0 {
                        p.rect_filled(r, CornerRadius::same(6), if resp.hovered() { theme.surface3 } else { theme.surface2 });
                    }
                    let nr = p.text(r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, format!("/{name}"), semibold(12.5), theme.text);
                    if !args.is_empty() {
                        p.text(pos2(nr.right() + 6.0, r.center().y), Align2::LEFT_CENTER, *args, mono(11.5), theme.text_faint);
                    }
                    p.text(r.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, *desc, ui_font(11.5), theme.text_dim);
                    if resp.clicked() {
                        pick = Some(name);
                    }
                }
            });
    });
    if let Some(name) = pick {
        let takes_arg = SLASH.iter().any(|(n, a, _)| *n == name && !a.is_empty());
        view.composer = if takes_arg { format!("/{name} ") } else { format!("/{name}") };
        set_cursor_end(ui.ctx(), edit_id, &view.composer);
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
}

fn mode_color(mode: Mode, theme: &Theme) -> Color32 {
    match mode {
        Mode::Interactive => theme.text,
        Mode::Plan => theme.accent,
        Mode::Autopilot => theme.warn,
    }
}

/// The agent's model by its display name ("Claude Sonnet 5.5").
fn model_name(view: &AgentView, models: &[ModelInfo]) -> String {
    match view.model.as_deref() {
        Some(id) => models.iter().find(|m| m.id == id).map_or_else(|| id.to_owned(), |m| m.name.clone()),
        None => "Default model".to_owned(),
    }
}

fn short_count(n: i64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// The message box: a rounded card with the message on top and a toolbar under it (attach, mode,
/// model, context used, send), like Claude's and ChatGPT's.
fn composer_ui(ui: &mut Ui, view: &mut AgentView, cx: &AgentCtx, out: &mut Vec<AgentAction>, rect: Rect, hero: bool) {
    let theme = cx.theme;
    let p = ui.painter().clone();
    let edit_id = Id::unique(("composer-edit", view.bay));
    let has_focus = ui.memory(|m| m.has_focus(edit_id));
    // Clicking the card's empty space puts the cursor in the message.
    if ui.interact(rect, Id::unique(("composer-card", view.bay)), Sense::click()).clicked() {
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
    let border = if has_focus { mix(theme.border_strong, theme.accent, 0.35) } else { theme.border_strong };
    p.rect(rect, CornerRadius::same(14), theme.surface2, Stroke::new(1.0, border), StrokeKind::Inside);

    let busy = view.status.is_busy();
    let can_send = !view.composer.trim().is_empty();
    let stop_mode = busy && !can_send;
    let bar = Rect::from_min_max(pos2(rect.left() + 8.0, rect.bottom() - 42.0), pos2(rect.right() - 8.0, rect.bottom() - 6.0));
    let cy = bar.center().y;

    // Send, or stop while the agent works.
    let send_r = Rect::from_center_size(pos2(bar.right() - 16.0, cy), vec2(32.0, 32.0));
    let send = ui.interact(send_r, Id::unique(("send", view.bay)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    send.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if stop_mode { "Stop" } else { "Send" }));
    let (fill, fg, glyph) = if stop_mode {
        (theme.text, theme.surface, icon::STOP)
    } else if can_send {
        (theme.accent, theme.on_accent, icon::ARROW_UP)
    } else {
        (theme.surface3, theme.text_faint, icon::ARROW_UP)
    };
    let lit = send.hovered() && (can_send || stop_mode);
    p.rect_filled(send_r, CornerRadius::same(9), if lit { mix(fill, Color32::WHITE, 0.1) } else { fill });
    p.text(send_r.center(), Align2::CENTER_CENTER, glyph, icon_font(if stop_mode { 10.0 } else { 13.0 }), fg);
    let send = send.on_hover_text(if stop_mode { "Stop (Esc)" } else { "Send (Enter)" });

    let plus_r = Rect::from_center_size(pos2(bar.left() + 15.0, cy), vec2(30.0, 30.0));
    let plus = ui.interact(plus_r, Id::unique(("attach", view.bay)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Attach files (or drop them here)");
    plus.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Attach files"));
    kit::paint_icon_button(ui, plus_r, &plus, icon::ADD, theme, false);
    if plus.clicked() {
        out.push(AgentAction::Attach);
    }

    // How much the agent does on its own: a segmented control, or just the current mode when narrow.
    let model = model_name(view, cx.models);
    let model_w = p.layout_no_wrap(model.clone(), ui_font(12.0), theme.text).size().x + 28.0;
    let ring_w = if view.context.is_some() { 26.0 } else { 0.0 };
    let modes = [
        (Mode::Interactive, "Ask", "Asks before it edits files or runs commands"),
        (Mode::Plan, "Plan", "Writes a plan for you to approve, then carries it out"),
        (Mode::Autopilot, "Autopilot", "Works on its own until the task is done"),
    ];
    let seg_font = ui_font(12.0);
    let seg_w: Vec<f32> = modes.iter().map(|(_, label, _)| p.layout_no_wrap(label.to_string(), seg_font.clone(), theme.text).size().x + 22.0).collect();
    let track_w = seg_w.iter().sum::<f32>() + 4.0;
    let mut x = plus_r.right() + 6.0;
    if x + track_w + 12.0 + model_w.min(150.0) + ring_w < send_r.left() - 8.0 {
        let track = Rect::from_min_size(pos2(x, cy - 14.0), vec2(track_w, 28.0));
        p.rect_filled(track, CornerRadius::same(8), theme.surface);
        let mut sx = track.left() + 2.0;
        for ((mode, label, tip), w) in modes.iter().zip(&seg_w) {
            let r = Rect::from_min_size(pos2(sx, track.top() + 2.0), vec2(*w, 24.0));
            let selected = view.mode == *mode;
            let resp = ui.interact(r, Id::unique(("mode", view.bay, *label)), Sense::click()).on_hover_text(*tip).on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, *label));
            if selected {
                p.rect(r, CornerRadius::same(6), theme.surface3, Stroke::new(1.0, theme.border_strong), StrokeKind::Inside);
            } else if resp.hovered() {
                p.rect_filled(r, CornerRadius::same(6), theme.surface2);
            }
            let color = if selected { mode_color(*mode, theme) } else if resp.hovered() { theme.text } else { theme.text_dim };
            p.text(r.center(), Align2::CENTER_CENTER, *label, seg_font.clone(), color);
            if resp.clicked() && !selected {
                out.push(AgentAction::SetMode(*mode));
            }
            sx += w;
        }
        x = track.right() + 8.0;
    } else {
        let label = view.mode.label();
        let w = p.layout_no_wrap(label.to_owned(), seg_font.clone(), theme.text).size().x + 20.0;
        let r = Rect::from_min_size(pos2(x, cy - 13.0), vec2(w, 26.0));
        let resp = ui.interact(r, Id::unique(("mode", view.bay)), Sense::click()).on_hover_text("Click for the next mode: Ask → Plan → Autopilot").on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Mode: {label}")));
        p.rect(r, CornerRadius::same(7), if resp.hovered() { theme.surface3 } else { theme.surface }, Stroke::new(1.0, theme.border), StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, label, seg_font, mode_color(view.mode, theme));
        if resp.clicked() {
            out.push(AgentAction::SetMode(view.mode.next()));
        }
        x = r.right() + 8.0;
    }

    // Context used, then the model, right-aligned before the send button.
    let mut right = send_r.left() - 6.0;
    if let Some((cur, lim)) = view.context {
        let frac = (cur as f32 / lim.max(1) as f32).clamp(0.0, 1.0);
        let r = Rect::from_center_size(pos2(right - 11.0, cy), vec2(22.0, 22.0));
        let tip = format!("{:.0}% of the context window used ({} of {} tokens)", frac * 100.0, short_count(cur), short_count(lim));
        ui.interact(r, Id::unique(("context", view.bay)), Sense::hover()).on_hover_text(tip);
        p.circle_stroke(r.center(), 7.0, Stroke::new(2.0, theme.surface3));
        if frac > 0.0 {
            let arc: Vec<egui::Pos2> = (0..=32)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frac * k as f32 / 32.0;
                    r.center() + vec2(a.cos(), a.sin()) * 7.0
                })
                .collect();
            p.add(egui::Shape::line(arc, Stroke::new(2.0, if frac > 0.8 { theme.warn } else { theme.text_dim })));
        }
        right = r.left() - 2.0;
    }
    if right - x > 56.0 {
        let w = model_w.min(right - x);
        let r = Rect::from_min_size(pos2(right - w, cy - 13.0), vec2(w, 26.0));
        let resp = ui.interact(r, Id::unique(("model", view.bay)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Model: {model}")));
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        if resp.hovered() || open {
            p.rect_filled(r, CornerRadius::same(7), theme.surface3);
        }
        let color = if resp.hovered() || open { theme.text } else { theme.text_dim };
        let g = kit::elided(&p, &model, ui_font(12.0), color, w - 26.0);
        p.galley(pos2(r.left() + 9.0, cy - g.size().y / 2.0), g, color);
        p.text(pos2(r.right() - 11.0, cy + 1.0), Align2::CENTER_CENTER, icon::CHEVRON_DOWN, icon_font(8.0), theme.text_faint);
        egui::Popup::menu(&resp).align(egui::RectAlign::TOP_END).width(280.0).show(|ui| {
            ui.set_max_height(380.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                if cx.models.is_empty() {
                    ui.label(RichText::new("Loading models…").color(theme.text_dim));
                }
                for m in cx.models {
                    let current = view.model.as_deref() == Some(m.id.as_str());
                    let label = match m.multiplier {
                        Some(x) if x > 0.0 => format!("{}   ·  {x}×", m.name),
                        _ => m.name.clone(),
                    };
                    if ui.selectable_label(current, label).clicked() {
                        out.push(AgentAction::SetModel(m.id.clone()));
                        ui.close();
                    }
                }
            });
        });
    }

    // Files going with the next message.
    let mut top = rect.top() + 12.0;
    if !view.attachments.is_empty() {
        let row = Rect::from_min_max(pos2(rect.left() + 12.0, top), pos2(rect.right() - 12.0, top + 26.0));
        let mut remove = None;
        ui.scope_builder(UiBuilder::new().max_rect(row).id_salt(("attachments", view.bay)), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (k, path) in view.attachments.iter().enumerate() {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
                    if kit::file_pill(ui, &name, theme, true) {
                        remove = Some(k);
                    }
                }
            });
        });
        if let Some(k) = remove {
            view.attachments.remove(k);
        }
        top += 36.0;
    }

    // The message itself.
    let edit_rect = Rect::from_min_max(pos2(rect.left() + 16.0, top), pos2(rect.right() - 16.0, bar.top() - 2.0));
    slash_menu(ui, view, rect, theme, edit_id, has_focus);
    let mut submitted = false;
    let mut stop = false;
    ui.scope_builder(UiBuilder::new().max_rect(edit_rect), |ui| {
        let enter_pressed = has_focus && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift && !i.modifiers.ctrl);
        if has_focus && ui.input(|i| i.key_pressed(Key::Escape)) && busy {
            stop = true;
        }
        if has_focus && view.composer.is_empty() && ui.input(|i| i.key_pressed(Key::ArrowUp)) && !view.sent_history.is_empty() {
            let idx = view.history_idx.map(|i| i.saturating_sub(1)).unwrap_or(view.sent_history.len() - 1);
            view.history_idx = Some(idx);
            view.composer = view.sent_history[idx].clone();
        }
        let hint = match (busy, edit_rect.width() < 420.0) {
            (true, false) => "Steer Copilot, or queue a follow-up…",
            (true, true) => "Steer or queue…",
            (false, false) => "Ask Copilot to build, fix or explain anything…",
            (false, true) => "Ask Copilot…",
        };
        egui::ScrollArea::vertical().id_salt(("composer-scroll", view.bay)).max_height(edit_rect.height()).auto_shrink([false, true]).show(ui, |ui| {
            let resp = ui.add(
                TextEdit::multiline(&mut view.composer)
                    .id(edit_id)
                    .frame(Frame::NONE)
                    .margin(Margin::ZERO)
                    .font(ui_font(14.0))
                    .hint_text(hint)
                    .desired_width(f32::INFINITY)
                    .desired_rows(if hero { 2 } else { 1 })
                    .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter)),
            );
            if view.focus_composer && cx.focused {
                resp.request_focus();
                view.focus_composer = false;
            }
        });
        if enter_pressed {
            submitted = true;
            // Strip the newline egui may have inserted on plain Enter.
            while view.composer.ends_with('\n') {
                view.composer.pop();
            }
        }
    });
    if send.clicked() {
        if stop_mode {
            stop = true;
        } else {
            submitted = true;
        }
    }
    if stop {
        out.push(AgentAction::Abort);
    }
    if submitted {
        let text = view.composer.trim().to_owned();
        if !text.is_empty() {
            view.composer.clear();
            if let Some(cmd) = text.strip_prefix('/') {
                slash(view, cmd, out);
            } else {
                out.push(AgentAction::Send(text));
            }
        }
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
}

fn slash(view: &mut AgentView, cmd: &str, out: &mut Vec<AgentAction>) {
    let mut parts = cmd.splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match name {
        "model" if !arg.is_empty() => out.push(AgentAction::SetModel(arg.to_owned())),
        "mode" => match Mode::parse(if arg == "ask" { "interactive" } else { arg }) {
            Some(m) => out.push(AgentAction::SetMode(m)),
            None => view.notice(Level::Warn, "Usage: /mode ask|plan|autopilot"),
        },
        "clear" => {
            view.items.clear();
            view.heights.clear();
        }
        "stop" | "abort" => out.push(AgentAction::Abort),
        "attach" if !arg.is_empty() => {
            let raw = arg.trim_matches('"');
            let path = std::path::PathBuf::from(raw);
            let path = if path.is_absolute() { path } else { view.cwd.join(path) };
            if path.exists() {
                if !view.attachments.contains(&path) {
                    view.attachments.push(path);
                }
            } else {
                view.notice(Level::Warn, format!("No such file or folder: {raw}"));
            }
        }
        "attach" => out.push(AgentAction::Attach),
        _ => out.push(AgentAction::Send(format!("/{cmd}"))),
    }
}
