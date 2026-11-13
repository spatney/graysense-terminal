//! Native window polish on Windows 11: a title bar, frame and corners in the theme's colours.

/// `#rrggbb` → a Win32 COLORREF (0x00bbggrr).
fn colorref(hex: &str) -> Option<u32> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    let (r, g, b) = (v >> 16 & 0xff, v >> 8 & 0xff, v & 0xff);
    Some(b << 16 | g << 8 | r)
}

/// Paint the native caption, its text and the 1px frame in the theme's colours. Windows 10 ignores
/// the colours and keeps only the dark/light hint.
#[cfg(windows)]
pub fn apply_theme(window: &tauri::Window, caption: &str, text: &str, border: &str, dark: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    let set = |attr: i32, value: u32| {
        // SAFETY: `hwnd` is our live top-level window; every attribute here takes a 4-byte value.
        unsafe { DwmSetWindowAttribute(hwnd, attr as _, (&value as *const u32).cast(), 4) };
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE as _, dark as u32);
    set(DWMWA_WINDOW_CORNER_PREFERENCE as _, DWMWCP_ROUND as u32);
    for (attr, hex) in [(DWMWA_CAPTION_COLOR, caption), (DWMWA_TEXT_COLOR, text), (DWMWA_BORDER_COLOR, border)] {
        if let Some(c) = colorref(hex) {
            set(attr as _, c);
        }
    }
}

#[cfg(not(windows))]
pub fn apply_theme(_window: &tauri::Window, _caption: &str, _text: &str, _border: &str, _dark: bool) {}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_colours_become_colorrefs() {
        assert_eq!(super::colorref("#0b0e14"), Some(0x14_0e_0b));
        assert_eq!(super::colorref("#35a3ea"), Some(0xea_a3_35));
        assert_eq!(super::colorref("35a3ea"), None);
        assert_eq!(super::colorref("#fff"), None);
    }
}
