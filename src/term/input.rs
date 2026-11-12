//! Key and mouse encoding for xterm-compatible PTYs.

use alacritty_terminal::term::TermMode;
use eframe::egui::{Key, Modifiers};

fn mod_param(m: Modifiers) -> u8 {
    1 + m.shift as u8 + (m.alt as u8) * 2 + (m.ctrl as u8) * 4
}

fn csi(s: String) -> Vec<u8> {
    let mut v = b"\x1b[".to_vec();
    v.extend_from_slice(s.as_bytes());
    v
}

/// Encode a key press. Returns `None` for keys that produce text via `Event::Text`.
pub fn encode_key(key: Key, m: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let mp = mod_param(m);
    let has_mods = mp > 1;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    let cursor = |c: char| -> Vec<u8> {
        if has_mods {
            csi(format!("1;{mp}{c}"))
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            csi(c.to_string())
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if has_mods { csi(format!("{n};{mp}~")) } else { csi(format!("{n}~")) } };
    let ss3 = |c: char| -> Vec<u8> { if has_mods { csi(format!("1;{mp}{c}")) } else { format!("\x1bO{c}").into_bytes() } };

    let bytes = match key {
        Key::Enter => {
            if m.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        Key::Tab => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        Key::Backspace => {
            if m.ctrl {
                vec![0x08]
            } else if m.alt {
                b"\x1b\x7f".to_vec()
            } else {
                vec![0x7f]
            }
        }
        Key::Escape => vec![0x1b],
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        Key::Space if m.ctrl => vec![0x00],
        _ => {
            let name = key.name();
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return punct(key, m);
            };
            if !c.is_ascii_alphabetic() {
                return punct(key, m);
            }
            let lower = c.to_ascii_lowercase();
            if m.ctrl && m.alt {
                // AltGr on Windows arrives as Ctrl+Alt: let the composed text through.
                return None;
            } else if m.ctrl {
                vec![(lower as u8) & 0x1f]
            } else if m.alt {
                let ch = if m.shift { c.to_ascii_uppercase() } else { lower };
                vec![0x1b, ch as u8]
            } else {
                return None;
            }
        }
    };
    Some(bytes)
}

fn punct(key: Key, m: Modifiers) -> Option<Vec<u8>> {
    if !m.ctrl {
        return None;
    }
    let b = match key {
        Key::OpenBracket => 0x1b,
        Key::Backslash => 0x1c,
        Key::CloseBracket => 0x1d,
        Key::Num6 => 0x1e,
        Key::Minus | Key::Slash => 0x1f,
        Key::Num2 => 0x00,
        _ => return None,
    };
    Some(vec![b])
}

/// Whether a key event with these modifiers was fully encoded by [`encode_key`]
/// such that a following `Event::Text` for the same keystroke must be dropped.
pub fn suppresses_text(key: Key, m: Modifiers) -> bool {
    (m.ctrl != m.alt) && !matches!(key, Key::Enter | Key::Tab | Key::Backspace | Key::Escape)
}

/// SGR (1006) or legacy X10 mouse report.
pub fn mouse_report(button: u8, col: usize, row: usize, pressed: bool, m: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let mut b = button;
    if m.shift {
        b += 4;
    }
    if m.alt {
        b += 8;
    }
    if m.ctrl {
        b += 16;
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{b};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    if col > 222 || row > 222 {
        return None;
    }
    let code = if pressed { b } else { 3 };
    Some(vec![0x1b, b'[', b'M', 32 + code, 33 + col as u8, 33 + row as u8])
}
