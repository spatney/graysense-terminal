//! Native window polish on Windows 11: rounded corners and a border that matches the theme.

use eframe::egui::Color32;

/// Raw HWND of the main window, captured at startup.
#[derive(Clone, Copy, Default)]
pub struct NativeWindow(isize);

impl NativeWindow {
    pub fn from_handle(handle: &impl raw_window_handle::HasWindowHandle) -> Self {
        match handle.window_handle().map(|h| h.as_raw()) {
            #[cfg(windows)]
            Ok(raw_window_handle::RawWindowHandle::Win32(w)) => NativeWindow(w.hwnd.get()),
            _ => NativeWindow(0),
        }
    }

    /// Ask DWM for rounded corners and paint its 1px frame in the theme's border colour.
    #[cfg(windows)]
    pub fn apply_theme(self, border: Color32, dark: bool) {
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
        if self.0 == 0 {
            return;
        }
        let hwnd = self.0 as windows_sys::Win32::Foundation::HWND;
        let corner: i32 = DWMWCP_ROUND;
        let colorref: u32 = (border.b() as u32) << 16 | (border.g() as u32) << 8 | border.r() as u32;
        let dark: i32 = dark as i32;
        // SAFETY: `hwnd` is our live top-level window; each attribute is a 4-byte value.
        unsafe {
            DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as _, (&corner as *const i32).cast(), 4);
            DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as _, (&colorref as *const u32).cast(), 4);
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as _, (&dark as *const i32).cast(), 4);
        }
    }

    #[cfg(not(windows))]
    pub fn apply_theme(self, _border: Color32, _dark: bool) {}

    /// Mouse position in egui points, read from the OS. Unlike egui's pointer, this
    /// stays accurate during an OLE drag-and-drop where the window gets no mouse moves.
    #[cfg(windows)]
    pub fn cursor_pos(self, pixels_per_point: f32) -> Option<eframe::egui::Pos2> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        if self.0 == 0 {
            return None;
        }
        let mut pt = POINT { x: 0, y: 0 };
        // SAFETY: valid out-pointer and live window handle.
        let ok = unsafe { GetCursorPos(&mut pt) != 0 && ScreenToClient(self.0 as _, &mut pt) != 0 };
        ok.then(|| eframe::egui::pos2(pt.x as f32 / pixels_per_point, pt.y as f32 / pixels_per_point))
    }

    #[cfg(not(windows))]
    pub fn cursor_pos(self, _pixels_per_point: f32) -> Option<eframe::egui::Pos2> {
        None
    }
}
