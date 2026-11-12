//! Graysense design tokens: flat neutral palettes, typography, egui style.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle, Vec2,
};
use eframe::epaint::text::{FontTweak, VariationCoords};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeId {
    Graphite,
    Carbon,
    Slate,
    Paper,
}

impl ThemeId {
    pub const ALL: [ThemeId; 4] = [ThemeId::Graphite, ThemeId::Carbon, ThemeId::Slate, ThemeId::Paper];

    pub fn name(self) -> &'static str {
        match self {
            ThemeId::Graphite => "Graphite",
            ThemeId::Carbon => "Carbon",
            ThemeId::Slate => "Slate",
            ThemeId::Paper => "Paper (light)",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    /// Window background (gaps between bays).
    pub bg: Color32,
    /// Bays, side panel, title bar.
    pub surface: Color32,
    /// Headers, inputs, hover.
    pub surface2: Color32,
    /// Pressed / selected.
    pub surface3: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
    pub term_fg: Color32,
    pub term_bg: Color32,
    pub ansi: [Color32; 16],
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

const ANSI_DARK: [Color32; 16] = [
    hex(0x2D333B),
    hex(0xF47067),
    hex(0x57AB5A),
    hex(0xC69026),
    hex(0x539BF5),
    hex(0xB083F0),
    hex(0x39C5CF),
    hex(0xADBAC7),
    hex(0x636E7B),
    hex(0xFF938A),
    hex(0x6BC46D),
    hex(0xDAAA3F),
    hex(0x6CB6FF),
    hex(0xDCBDFB),
    hex(0x56D4DD),
    hex(0xCDD9E5),
];

const ANSI_LIGHT: [Color32; 16] = [
    hex(0x24292F),
    hex(0xCF222E),
    hex(0x116329),
    hex(0x7D4E00),
    hex(0x0969DA),
    hex(0x8250DF),
    hex(0x1B7C83),
    hex(0x6E7781),
    hex(0x57606A),
    hex(0xA40E26),
    hex(0x1A7F37),
    hex(0x633C01),
    hex(0x218BFF),
    hex(0xA475F9),
    hex(0x3192AA),
    hex(0x8C959F),
];

impl Theme {
    pub fn get(id: ThemeId) -> Theme {
        match id {
            ThemeId::Graphite => Theme {
                dark: true,
                bg: hex(0x0F1012),
                surface: hex(0x17181B),
                surface2: hex(0x1F2125),
                surface3: hex(0x2A2D32),
                border: hex(0x26292E),
                border_strong: hex(0x383C43),
                text: hex(0xE6E8EB),
                text_dim: hex(0x9BA1AA),
                text_faint: hex(0x666C76),
                accent: hex(0x5B8CFF),
                on_accent: hex(0xFFFFFF),
                ok: hex(0x3FB950),
                warn: hex(0xD29922),
                err: hex(0xF85149),
                term_fg: hex(0xD5DAE0),
                term_bg: hex(0x17181B),
                ansi: ANSI_DARK,
            },
            ThemeId::Carbon => Theme {
                dark: true,
                bg: hex(0x000000),
                surface: hex(0x0C0C0D),
                surface2: hex(0x161618),
                surface3: hex(0x222225),
                border: hex(0x1F1F22),
                border_strong: hex(0x323236),
                text: hex(0xEDEDEF),
                text_dim: hex(0x9C9CA3),
                text_faint: hex(0x5F5F66),
                accent: hex(0xA1A1FF),
                on_accent: hex(0x0B0B10),
                ok: hex(0x4ADE80),
                warn: hex(0xFBBF24),
                err: hex(0xF87171),
                term_fg: hex(0xDCDCE0),
                term_bg: hex(0x0C0C0D),
                ansi: ANSI_DARK,
            },
            ThemeId::Slate => Theme {
                dark: true,
                bg: hex(0x0D1117),
                surface: hex(0x141A21),
                surface2: hex(0x1B232C),
                surface3: hex(0x25303B),
                border: hex(0x222B35),
                border_strong: hex(0x34404D),
                text: hex(0xE1E7EE),
                text_dim: hex(0x93A1B0),
                text_faint: hex(0x5E6B78),
                accent: hex(0x2DD4BF),
                on_accent: hex(0x04211D),
                ok: hex(0x3FB950),
                warn: hex(0xD29922),
                err: hex(0xF85149),
                term_fg: hex(0xD3DCE6),
                term_bg: hex(0x141A21),
                ansi: ANSI_DARK,
            },
            ThemeId::Paper => Theme {
                dark: false,
                bg: hex(0xECEEF1),
                surface: hex(0xFFFFFF),
                surface2: hex(0xF5F6F8),
                surface3: hex(0xE8EAEE),
                border: hex(0xE1E4E8),
                border_strong: hex(0xC9CED5),
                text: hex(0x1B1F24),
                text_dim: hex(0x57606A),
                text_faint: hex(0x8C959F),
                accent: hex(0x2F6BFF),
                on_accent: hex(0xFFFFFF),
                ok: hex(0x1A7F37),
                warn: hex(0x9A6700),
                err: hex(0xCF222E),
                term_fg: hex(0x1F2328),
                term_bg: hex(0xFFFFFF),
                ansi: ANSI_LIGHT,
            },
        }
    }

    /// Accent tint layered over the surface (selected rows, soft badges).
    pub fn accent_soft(&self) -> Color32 {
        mix(self.surface, self.accent, if self.dark { 0.16 } else { 0.10 })
    }

    pub fn selection(&self) -> Color32 {
        with_alpha(self.accent, if self.dark { 0.32 } else { 0.22 })
    }
}

/// Linear mix in sRGB space (good enough for UI tints).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn with_alpha(c: Color32, alpha: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn semibold_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn icon_family() -> FontFamily {
    FontFamily::Name("icons".into())
}

pub fn ui(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, icon_family())
}

/// Variation coordinates for a bold weight on variable fonts (Cascadia Mono).
pub fn bold_coords() -> VariationCoords {
    VariationCoords::new([("wght", 700.0)])
}

fn fonts_dir() -> std::path::PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    std::path::Path::new(&windir).join("Fonts")
}

fn system_font(name: &str) -> Option<Vec<u8>> {
    std::fs::read(fonts_dir().join(name)).ok()
}

/// Find an installed Nerd Font (shell prompt icons live in the private-use area),
/// preferring the "Mono" variant whose glyphs fit a single terminal cell.
fn nerd_font() -> Option<Vec<u8>> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(std::path::Path::new(&local).join("Microsoft\\Windows\\Fonts"));
    }
    dirs.push(fonts_dir());
    let mut best: Option<(u8, std::path::PathBuf)> = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !(name.ends_with(".ttf") || name.ends_with(".otf")) || !name.contains("regular") {
                continue;
            }
            let score = if name.contains("nerdfontmono") {
                3
            } else if name.contains("nerdfont") && !name.contains("propo") {
                2
            } else if name.contains("nf") && name.contains("cascadia") {
                1
            } else {
                continue;
            };
            let score = score * 2 + u8::from(name.contains("meslolgm"));
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, e.path()));
            }
        }
    }
    best.and_then(|(_, p)| std::fs::read(p).ok())
}

/// Install the Graysense font stack: Segoe UI Variable for UI, Segoe Fluent Icons,
/// Cascadia Mono for terminals/code, with Nerd Font / symbol / emoji fallbacks.
pub fn install_fonts(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let add = |defs: &mut FontDefinitions, key: &str, data: Option<FontData>| -> bool {
        match data {
            Some(d) => {
                defs.font_data.insert(key.to_owned(), Arc::new(d));
                true
            }
            None => false,
        }
    };

    let ui_var = system_font("SegUIVar.ttf");
    let has_ui = match &ui_var {
        Some(bytes) => {
            add(&mut defs, "ui", Some(FontData::from_owned(bytes.clone())));
            let tweak = FontTweak { coords: VariationCoords::new([("wght", 600.0)]), ..Default::default() };
            add(&mut defs, "ui-semibold", Some(FontData::from_owned(bytes.clone()).tweak(tweak)))
        }
        None => {
            let a = add(&mut defs, "ui", system_font("segoeui.ttf").map(FontData::from_owned));
            add(&mut defs, "ui-semibold", system_font("seguisb.ttf").map(FontData::from_owned));
            a
        }
    };
    let has_icons = add(&mut defs, "icons", system_font("SegoeIcons.ttf").or_else(|| system_font("segmdl2.ttf")).map(FontData::from_owned));
    let has_mono = add(&mut defs, "cascadia", system_font("CascadiaMono.ttf").or_else(|| system_font("consola.ttf")).map(FontData::from_owned));
    let has_nerd = add(&mut defs, "nerd", nerd_font().map(FontData::from_owned));
    let has_symbol = add(&mut defs, "symbol", system_font("seguisym.ttf").map(FontData::from_owned));
    let has_emoji = add(&mut defs, "emoji", system_font("seguiemj.ttf").map(FontData::from_owned));

    let defaults_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let defaults_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let push_if = |v: &mut Vec<String>, cond: bool, key: &str| {
        if cond {
            v.push(key.to_owned());
        }
    };

    let mut prop = Vec::new();
    push_if(&mut prop, has_ui, "ui");
    push_if(&mut prop, has_icons, "icons");
    push_if(&mut prop, has_symbol, "symbol");
    push_if(&mut prop, has_emoji, "emoji");
    prop.extend(defaults_prop.iter().cloned());

    let mut semi = Vec::new();
    push_if(&mut semi, defs.font_data.contains_key("ui-semibold"), "ui-semibold");
    semi.extend(prop.iter().cloned());

    let mut monof = Vec::new();
    push_if(&mut monof, has_mono, "cascadia");
    push_if(&mut monof, has_nerd, "nerd");
    push_if(&mut monof, has_symbol, "symbol");
    push_if(&mut monof, has_emoji, "emoji");
    push_if(&mut monof, has_icons, "icons");
    monof.extend(defaults_mono.iter().cloned());

    let mut icons = Vec::new();
    push_if(&mut icons, has_icons, "icons");
    icons.extend(prop.iter().cloned());

    defs.families.insert(FontFamily::Proportional, prop);
    defs.families.insert(FontFamily::Monospace, monof);
    defs.families.insert(semibold_family(), semi);
    defs.families.insert(icon_family(), icons);
    ctx.set_fonts(defs);
}

/// Apply the theme to egui's global style.
pub fn apply_style(ctx: &egui::Context, theme: &Theme) {
    ctx.global_style_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, semibold(15.0)),
            (TextStyle::Body, ui(13.5)),
            (TextStyle::Button, ui(13.0)),
            (TextStyle::Small, ui(11.5)),
            (TextStyle::Monospace, mono(12.5)),
        ]
        .into();

        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
        style.spacing.interact_size.y = 24.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        style.interaction.selectable_labels = true;

        let v = &mut style.visuals;
        v.dark_mode = theme.dark;
        v.override_text_color = Some(theme.text);
        v.weak_text_color = Some(theme.text_dim);
        v.panel_fill = theme.surface;
        v.window_fill = theme.surface;
        v.window_stroke = Stroke::new(1.0, theme.border_strong);
        v.window_corner_radius = CornerRadius::same(10);
        v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if theme.dark { 120 } else { 40 }) };
        v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(if theme.dark { 110 } else { 35 }) };
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = theme.surface2;
        v.text_edit_bg_color = Some(theme.surface2);
        v.faint_bg_color = theme.surface2;
        v.code_bg_color = theme.surface2;
        v.hyperlink_color = theme.accent;
        v.warn_fg_color = theme.warn;
        v.error_fg_color = theme.err;
        v.selection.bg_fill = theme.selection();
        v.selection.stroke = Stroke::new(1.0, theme.accent);
        v.text_cursor.stroke = Stroke::new(2.0, theme.accent);
        v.striped = false;

        let r = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = theme.surface;
        w.noninteractive.weak_bg_fill = theme.surface;
        w.noninteractive.bg_stroke = Stroke::new(1.0, theme.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, theme.text);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = theme.surface2;
        w.inactive.weak_bg_fill = theme.surface2;
        w.inactive.bg_stroke = Stroke::new(1.0, theme.border);
        w.inactive.fg_stroke = Stroke::new(1.0, theme.text);
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = theme.surface3;
        w.hovered.weak_bg_fill = theme.surface3;
        w.hovered.bg_stroke = Stroke::new(1.0, theme.border_strong);
        w.hovered.fg_stroke = Stroke::new(1.0, theme.text);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = theme.surface3;
        w.active.weak_bg_fill = theme.surface3;
        w.active.bg_stroke = Stroke::new(1.0, theme.accent);
        w.active.fg_stroke = Stroke::new(1.0, theme.text);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = theme.surface3;
        w.open.weak_bg_fill = theme.surface3;
        w.open.bg_stroke = Stroke::new(1.0, theme.border_strong);
        w.open.fg_stroke = Stroke::new(1.0, theme.text);
        w.open.corner_radius = r;
    });
}
