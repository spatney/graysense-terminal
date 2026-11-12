//! Lightweight markdown → egui renderer for agent messages.

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, CornerRadius, CursorIcon, FontId, Label, Sense, Stroke, Ui, vec2};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::kit::{self, icon};
use crate::theme::{Theme, mix, mono, semibold, ui as ui_font};

#[derive(Clone, Default)]
struct Span {
    text: String,
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    link: Option<String>,
}

enum Block {
    Para { spans: Vec<Span>, indent: u8, bullet: Option<String>, quote: bool },
    Heading { level: u8, spans: Vec<Span> },
    Code { lang: String, text: String },
    Rule,
    Table { rows: Vec<Vec<Vec<Span>>>, header: usize },
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    in_para: bool,
    bold: u8,
    italic: u8,
    strike: u8,
    link: Option<String>,
    lists: Vec<Option<u64>>,
    pending_bullet: Option<String>,
    quote: u8,
    heading: Option<u8>,
    code: Option<(String, String)>,
    table: Option<(Vec<Vec<Vec<Span>>>, usize)>,
    row: Option<Vec<Vec<Span>>>,
    in_head: bool,
}

impl Builder {
    fn push_text(&mut self, text: &str, code: bool) {
        if self.code.is_some() {
            if let Some((_, t)) = self.code.as_mut() {
                t.push_str(text);
            }
            return;
        }
        if self.row.is_none() && self.heading.is_none() {
            self.in_para = true;
        }
        self.spans.push(Span {
            text: text.to_owned(),
            bold: self.bold > 0 || self.heading.is_some(),
            italic: self.italic > 0,
            code,
            strike: self.strike > 0,
            link: self.link.clone(),
        });
    }

    fn flush_para(&mut self) {
        if self.spans.is_empty() {
            self.in_para = false;
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        if let Some(row) = self.row.as_mut() {
            row.push(spans);
            return;
        }
        let indent = self.lists.len().saturating_sub(1) as u8 + u8::from(!self.lists.is_empty());
        let bullet = self.pending_bullet.take();
        self.blocks.push(Block::Para { spans, indent, bullet, quote: self.quote > 0 });
        self.in_para = false;
    }
}

fn parse(text: &str) -> Vec<Block> {
    let mut b = Builder::default();
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(text, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    b.flush_para();
                    b.heading = Some(match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        _ => 4,
                    });
                }
                Tag::BlockQuote(_) => {
                    b.flush_para();
                    b.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    b.flush_para();
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    b.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    b.flush_para();
                    b.lists.push(start);
                }
                Tag::Item => {
                    b.flush_para();
                    let bullet = match b.lists.last_mut() {
                        Some(Some(n)) => {
                            let s = format!("{n}.");
                            *n += 1;
                            s
                        }
                        _ => "•".to_owned(),
                    };
                    b.pending_bullet = Some(bullet);
                }
                Tag::Emphasis => b.italic += 1,
                Tag::Strong => b.bold += 1,
                Tag::Strikethrough => b.strike += 1,
                Tag::Link { dest_url, .. } => b.link = Some(dest_url.to_string()),
                Tag::Table(_) => {
                    b.flush_para();
                    b.table = Some((Vec::new(), 0));
                }
                Tag::TableHead => {
                    b.in_head = true;
                    b.row = Some(Vec::new());
                }
                Tag::TableRow => b.row = Some(Vec::new()),
                Tag::TableCell => b.spans.clear(),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => b.flush_para(),
                TagEnd::Heading(_) => {
                    let spans = std::mem::take(&mut b.spans);
                    let level = b.heading.take().unwrap_or(3);
                    b.blocks.push(Block::Heading { level, spans });
                }
                TagEnd::BlockQuote(_) => {
                    b.flush_para();
                    b.quote = b.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, text)) = b.code.take() {
                        b.blocks.push(Block::Code { lang, text: text.trim_end_matches('\n').to_owned() });
                    }
                }
                TagEnd::List(_) => {
                    b.flush_para();
                    b.lists.pop();
                }
                TagEnd::Item => b.flush_para(),
                TagEnd::Emphasis => b.italic = b.italic.saturating_sub(1),
                TagEnd::Strong => b.bold = b.bold.saturating_sub(1),
                TagEnd::Strikethrough => b.strike = b.strike.saturating_sub(1),
                TagEnd::Link => b.link = None,
                TagEnd::TableCell => {
                    let spans = std::mem::take(&mut b.spans);
                    if let Some(row) = b.row.as_mut() {
                        row.push(spans);
                    }
                }
                TagEnd::TableHead => {
                    if let (Some(row), Some((rows, header))) = (b.row.take(), b.table.as_mut()) {
                        rows.push(row);
                        *header = rows.len();
                    }
                    b.in_head = false;
                }
                TagEnd::TableRow => {
                    if let (Some(row), Some((rows, _))) = (b.row.take(), b.table.as_mut()) {
                        rows.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some((rows, header)) = b.table.take() {
                        b.blocks.push(Block::Table { rows, header });
                    }
                }
                _ => {}
            },
            Event::Text(t) => b.push_text(&t, false),
            Event::Code(t) => b.push_text(&t, true),
            Event::Html(t) | Event::InlineHtml(t) => b.push_text(&t, false),
            Event::SoftBreak => b.push_text(" ", false),
            Event::HardBreak => b.push_text("\n", false),
            Event::Rule => {
                b.flush_para();
                b.blocks.push(Block::Rule);
            }
            Event::TaskListMarker(done) => b.pending_bullet = Some(if done { "☑".into() } else { "☐".into() }),
            Event::FootnoteReference(t) => b.push_text(&format!("[{t}]"), false),
            _ => {}
        }
    }
    b.flush_para();
    b.blocks
}

fn job_for(spans: &[Span], theme: &Theme, size: f32, width: f32, color: Color32) -> (LayoutJob, Vec<(std::ops::Range<usize>, String)>) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let mut links = Vec::new();
    for s in spans {
        let font: FontId = if s.code {
            mono(size - 1.0)
        } else if s.bold {
            semibold(size)
        } else {
            ui_font(size)
        };
        let mut fmt = TextFormat::simple(font, color);
        fmt.italics = s.italic;
        if s.code {
            fmt.background = theme.surface3;
            fmt.color = mix(color, theme.accent, 0.25);
        }
        if s.strike {
            fmt.strikethrough = Stroke::new(1.0, color);
        }
        if let Some(url) = &s.link {
            fmt.color = theme.accent;
            fmt.underline = Stroke::new(1.0, crate::theme::with_alpha(theme.accent, 0.5));
            let start = job.text.len();
            job.append(&s.text, 0.0, fmt);
            links.push((start..job.text.len(), url.clone()));
        } else {
            job.append(&s.text, 0.0, fmt);
        }
    }
    (job, links)
}

pub struct MdOut {
    pub open_url: Option<String>,
    pub copy: Option<String>,
}

fn rich_label(ui: &mut Ui, mut job: LayoutJob, links: Vec<(std::ops::Range<usize>, String)>, out: &mut MdOut, wrap: bool) {
    if !wrap {
        job.wrap.max_width = f32::INFINITY;
    }
    let has_links = !links.is_empty();
    let sense = if has_links { Sense::click() } else { Sense::hover() };
    let text = job.text.clone();
    let label = Label::new(job.clone()).sense(sense);
    let resp = ui.add(if wrap { label.wrap() } else { label.extend() });
    if has_links
        && let Some(pos) = resp.hover_pos() {
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let cursor = galley.cursor_from_pos(pos - resp.rect.min);
            let byte = text.char_indices().nth(cursor.index.into()).map(|(b, _)| b).unwrap_or(text.len());
            if let Some((_, url)) = links.iter().find(|(r, _)| r.contains(&byte)) {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                if resp.clicked() {
                    out.open_url = Some(url.clone());
                }
            }
        }
}

/// Render markdown into the current ui at full available width.
pub fn render(ui: &mut Ui, text: &str, theme: &Theme, out: &mut MdOut) {
    let width = ui.available_width();
    let size = 13.5;
    let blocks = parse(text);
    let n = blocks.len();
    for (i, block) in blocks.into_iter().enumerate() {
        match block {
            Block::Para { spans, indent, bullet, quote } => {
                let pad = indent as f32 * 16.0;
                let color = if quote { theme.text_dim } else { theme.text };
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if pad > 0.0 {
                        ui.add_space(pad - if bullet.is_some() { 12.0 } else { 0.0 });
                    }
                    if quote {
                        let (r, _) = ui.allocate_exact_size(vec2(3.0, 18.0), Sense::hover());
                        ui.painter().rect_filled(r, CornerRadius::same(1), theme.border_strong);
                    }
                    if let Some(b) = &bullet {
                        let c = if b == "☑" { theme.ok } else { theme.text_dim };
                        ui.label(egui::RichText::new(b).color(c).font(ui_font(size)));
                    }
                    let w = ui.available_width();
                    let (job, links) = job_for(&spans, theme, size, w, color);
                    rich_label(ui, job, links, out, true);
                });
            }
            Block::Heading { level, spans } => {
                if i > 0 {
                    ui.add_space(4.0);
                }
                let s = match level {
                    1 => 19.0,
                    2 => 16.5,
                    3 => 14.5,
                    _ => 13.5,
                };
                let (job, links) = job_for(&spans, theme, s, width, theme.text);
                rich_label(ui, job, links, out, true);
            }
            Block::Code { lang, text } => code_block(ui, &lang, &text, theme, out),
            Block::Rule => {
                ui.add_space(2.0);
                let (r, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
                ui.painter().rect_filled(r, 0.0, theme.border);
                ui.add_space(2.0);
            }
            Block::Table { rows, header } => {
                egui::Frame::new().stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                    egui::Grid::new(ui.next_auto_id()).striped(false).spacing(vec2(18.0, 4.0)).show(ui, |ui| {
                        for (ri, row) in rows.iter().enumerate() {
                            for cell in row {
                                let mut spans = cell.clone();
                                if ri < header {
                                    for s in spans.iter_mut() {
                                        s.bold = true;
                                    }
                                }
                                let (job, links) = job_for(&spans, theme, 13.0, 360.0, if ri < header { theme.text } else { theme.text_dim });
                                rich_label(ui, job, links, out, false);
                            }
                            ui.end_row();
                        }
                    });
                });
            }
        }
        if i + 1 < n {
            ui.add_space(3.0);
        }
    }
}

fn code_block(ui: &mut Ui, lang: &str, text: &str, theme: &Theme, out: &mut MdOut) {
    egui::Frame::new()
        .fill(theme.surface2)
        .stroke(Stroke::new(1.0, theme.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(if lang.is_empty() { "code" } else { lang }).font(ui_font(11.0)).color(theme.text_faint));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::icon_button_sized(ui, icon::COPY, "Copy", theme, 20.0, false).clicked() {
                        out.copy = Some(text.to_owned());
                    }
                });
            });
            let job = highlight(text, lang, theme);
            egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
                ui.add(Label::new(job).extend());
            });
        });
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "const", "pub", "use", "mod", "struct", "enum", "impl", "trait", "match", "if", "else", "for", "while",
    "loop", "return", "async", "await", "move", "where", "self", "Self", "crate", "super", "def", "class", "import", "from", "as",
    "function", "var", "export", "default", "new", "try", "catch", "finally", "throw", "interface", "type", "public", "private",
    "static", "void", "int", "string", "bool", "true", "false", "null", "None", "True", "False", "nil", "in", "of", "with", "yield",
    "lambda", "package", "func", "go", "defer", "select", "case", "switch", "break", "continue",
];

/// Minimal, language-agnostic syntax tinting (strings, comments, numbers, keywords).
pub fn highlight(text: &str, lang: &str, theme: &Theme) -> LayoutJob {
    let font = mono(12.5);
    let base = theme.text;
    let kw = theme.accent;
    let string = theme.ok;
    let comment = theme.text_faint;
    let number = theme.warn;
    let mut job = LayoutJob::default();
    let hash_comments = matches!(lang, "python" | "py" | "sh" | "bash" | "ps1" | "powershell" | "pwsh" | "yaml" | "yml" | "toml" | "rb" | "ruby");
    let push = |job: &mut LayoutJob, s: &str, c: Color32| job.append(s, 0.0, TextFormat::simple(font.clone(), c));
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut buf = String::new();
    while i < chars.len() {
        let c = chars[i];
        let at = |k: usize| chars.get(i + k).copied().unwrap_or('\0');
        if (c == '/' && at(1) == '/') || (hash_comments && c == '#') || (c == '-' && at(1) == '-' && matches!(lang, "sql" | "lua")) {
            push(&mut job, &buf, base);
            buf.clear();
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            push(&mut job, &chars[start..i].iter().collect::<String>(), comment);
            continue;
        }
        if c == '"' || c == '\'' || c == '`' {
            push(&mut job, &buf, base);
            buf.clear();
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != c && chars[i] != '\n' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            push(&mut job, &chars[start..i].iter().collect::<String>(), string);
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if KEYWORDS.contains(&word.as_str()) {
                push(&mut job, &buf, base);
                buf.clear();
                push(&mut job, &word, kw);
            } else {
                buf.push_str(&word);
            }
            continue;
        }
        if c.is_ascii_digit() {
            push(&mut job, &buf, base);
            buf.clear();
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                i += 1;
            }
            push(&mut job, &chars[start..i].iter().collect::<String>(), number);
            continue;
        }
        buf.push(c);
        i += 1;
    }
    push(&mut job, &buf, base);
    job
}
