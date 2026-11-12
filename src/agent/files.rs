//! Files tab in an agent's dock: the session's folder as a lazy tree, like an editor's explorer
//! or a chat app's artifacts. Open a file, reveal it, or attach it to the next message.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Align2, CornerRadius, CursorIcon, Id, Rect, RichText, Sense, Ui, pos2, vec2};

use crate::kit::{self, icon};
use crate::theme::{Theme, icon_font, ui as ui_font};

#[derive(Clone)]
struct FsEntry {
    path: PathBuf,
    name: String,
    dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

#[derive(Default)]
pub struct FilesState {
    expanded: HashSet<PathBuf>,
    listings: HashMap<PathBuf, (Instant, Vec<FsEntry>)>,
}

pub enum FileAction {
    Open(PathBuf),
    Reveal(PathBuf),
    Attach(PathBuf),
}

/// Folders that are rarely interesting and often huge: listed, but never opened automatically.
const HEAVY: &[&str] = &["node_modules", ".git", "target", ".venv", "venv", "__pycache__", ".next", "dist", "build"];
const ROW_H: f32 = 26.0;
const MAX_PER_DIR: usize = 400;

impl FilesState {
    /// Directory contents, re-read every couple of seconds while the tab is visible.
    fn list(&mut self, dir: &Path) -> &[FsEntry] {
        let stale = self.listings.get(dir).is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(2));
        if stale {
            let mut entries: Vec<FsEntry> = std::fs::read_dir(dir)
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let meta = e.metadata().ok()?;
                            Some(FsEntry { path: e.path(), name: e.file_name().to_string_lossy().into_owned(), dir: meta.is_dir(), size: meta.len(), modified: meta.modified().ok() })
                        })
                        .collect()
                })
                .unwrap_or_default();
            entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            entries.truncate(MAX_PER_DIR);
            self.listings.insert(dir.to_owned(), (Instant::now(), entries));
        }
        &self.listings[dir].1
    }

    /// Visible rows (entry, depth) in tree order.
    fn rows(&mut self, root: &Path) -> Vec<(FsEntry, usize)> {
        let mut out = Vec::new();
        let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_owned(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            let entries = self.list(&dir).to_vec();
            let mut nested = Vec::new();
            for e in entries {
                let open = e.dir && self.expanded.contains(&e.path);
                out.push((e.clone(), depth));
                if open {
                    nested.push((out.len(), e.path.clone(), depth + 1));
                }
            }
            // Expanded folders are listed right after their row: insert children in place.
            for (at, path, d) in nested.into_iter().rev() {
                let mut sub = FilesState { expanded: std::mem::take(&mut self.expanded), listings: std::mem::take(&mut self.listings) };
                let children = sub.subtree(&path, d);
                self.expanded = sub.expanded;
                self.listings = sub.listings;
                out.splice(at..at, children);
            }
        }
        out
    }

    fn subtree(&mut self, dir: &Path, depth: usize) -> Vec<(FsEntry, usize)> {
        let mut out = Vec::new();
        for e in self.list(dir).to_vec() {
            let open = e.dir && self.expanded.contains(&e.path);
            out.push((e.clone(), depth));
            if open && depth < 12 {
                out.extend(self.subtree(&e.path, depth + 1));
            }
        }
        out
    }
}

fn file_glyph(e: &FsEntry, open: bool) -> &'static str {
    if e.dir {
        return if open { icon::OPEN_FOLDER } else { icon::FOLDER };
    }
    match Path::new(&e.name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico") => icon::IMAGE,
        Some("csv" | "xlsx" | "tsv") => icon::TABLE,
        Some("md" | "txt" | "pdf" | "docx") => icon::DOCUMENT,
        _ => icon::CODE,
    }
}

fn human_size(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.0} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// The Files tab. `changed` holds paths the agent created or edited (marked with a dot).
pub fn files_ui(ui: &mut Ui, state: &mut FilesState, root: &Path, changed: &[String], theme: &Theme) -> Option<FileAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        let g = kit::elided(ui.painter(), &crate::settings::short_path(root), ui_font(11.5), theme.text_faint, ui.available_width() - 70.0);
        let (r, resp) = ui.allocate_exact_size(vec2(g.size().x, 20.0), Sense::hover());
        ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, theme.text_faint);
        resp.on_hover_text(root.display().to_string());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if kit::icon_button_sized(ui, icon::FOLDER, "Open in File Explorer", theme, 24.0, false).clicked() {
                action = Some(FileAction::Reveal(root.to_owned()));
            }
        });
    });
    ui.add_space(4.0);
    let rows = state.rows(root);
    if rows.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(icon::FOLDER).font(icon_font(22.0)).color(theme.text_faint));
            ui.label(RichText::new("No files yet").color(theme.text_dim));
            ui.add(egui::Label::new(RichText::new("Anything Copilot creates in this session's folder shows up here.").font(ui_font(12.0)).color(theme.text_faint)).wrap());
        });
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        return action;
    }
    let changed: HashSet<String> = changed.iter().map(|p| p.replace('/', "\\").to_lowercase()).collect();
    egui::ScrollArea::vertical().id_salt("files-tab").auto_shrink([false; 2]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
        for (e, depth) in &rows[range] {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &e.name));
            let open = e.dir && state.expanded.contains(&e.path);
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(r, CornerRadius::same(5), theme.surface2);
            }
            let x0 = r.left() + 6.0 + *depth as f32 * 14.0;
            if e.dir {
                p.text(pos2(x0 + 5.0, r.center().y), Align2::CENTER_CENTER, if open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT }, icon_font(8.0), theme.text_faint);
            }
            p.text(pos2(x0 + 20.0, r.center().y), Align2::CENTER_CENTER, file_glyph(e, open), icon_font(11.0), if e.dir { theme.accent } else { theme.text_dim });
            let edited = !e.dir && changed.contains(&e.path.display().to_string().to_lowercase());
            // Right side: size and age, or row actions on hover.
            let mut right = r.right() - 6.0;
            if resp.hovered() || ui.rect_contains_pointer(r) {
                for (glyph, tip, kind) in [(icon::ATTACH, "Attach to message", 0u8), (icon::FOLDER, "Show in File Explorer", 1u8)] {
                    if e.dir && kind == 0 {
                        continue;
                    }
                    let b = Rect::from_center_size(pos2(right - 11.0, r.center().y), vec2(22.0, 22.0));
                    let bresp = ui.interact(b, Id::unique(("file-act", &e.path, kind)), Sense::click()).on_hover_text(tip);
                    kit::paint_icon_button(ui, b, &bresp, glyph, theme, false);
                    if bresp.clicked() {
                        action = Some(if kind == 0 { FileAction::Attach(e.path.clone()) } else { FileAction::Reveal(e.path.clone()) });
                    }
                    right -= 24.0;
                }
            } else if !e.dir {
                let age = e.modified.and_then(|m| m.elapsed().ok()).map(|d| crate::app::short_age(d.as_secs())).unwrap_or_default();
                let wr = ui.painter().text(pos2(right, r.center().y), Align2::RIGHT_CENTER, format!("{}  ·  {age}", human_size(e.size)), ui_font(11.0), theme.text_faint);
                right = wr.left() - 6.0;
            }
            let p = ui.painter();
            let name_x = x0 + 32.0;
            let g = kit::elided(p, &e.name, ui_font(12.5), theme.text, (right - name_x - 14.0).max(20.0));
            let gw = g.size().x;
            p.galley(pos2(name_x, r.center().y - g.size().y / 2.0), g, theme.text);
            if edited {
                p.circle_filled(pos2(name_x + gw + 8.0, r.center().y), 3.0, theme.accent);
            }
            if resp.clicked() {
                if e.dir {
                    if !state.expanded.remove(&e.path) {
                        state.expanded.insert(e.path.clone());
                    }
                } else {
                    action = Some(FileAction::Open(e.path.clone()));
                }
            }
            if e.dir && HEAVY.contains(&e.name.as_str()) && resp.hovered() {
                resp.on_hover_text("Large folder; click to expand");
            }
        }
    });
    ui.ctx().request_repaint_after(Duration::from_secs(2));
    action
}
