//! Windows notifications: an agent finished or needs the user while Graysense is in the background.

use crate::agent::BayId;

/// What a notification says, and the bay a click shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub bay: BayId,
    /// The agent's name.
    pub title: String,
    pub body: String,
    /// The agent is waiting for the user (rather than done).
    pub urgent: bool,
}

/// One notification for whatever happened at once (Windows shows a few lines at most): the
/// latest news from each agent, the ones waiting for the user first.
pub fn combine(notes: Vec<Note>) -> Option<Note> {
    let mut latest: Vec<Note> = Vec::new();
    for n in notes {
        match latest.iter_mut().find(|l| l.bay == n.bay) {
            Some(l) if !(l.urgent && !n.urgent) => *l = n,
            Some(_) => {}
            None => latest.push(n),
        }
    }
    latest.sort_by_key(|n| !n.urgent);
    if latest.len() <= 1 {
        return latest.pop();
    }
    let waiting = latest.iter().filter(|n| n.urgent).count();
    let done = latest.len() - waiting;
    let title = match (waiting, done) {
        (0, d) => format!("{d} agents finished"),
        (w, 0) => format!("{w} agents need you"),
        (w, d) => format!("{w} need you, {d} finished"),
    };
    let mut lines: Vec<String> = latest.iter().take(3).map(|n| format!("{}: {}", n.title, n.body)).collect();
    if latest.len() > 3 {
        lines.push(format!("and {} more", latest.len() - 3));
    }
    Some(Note { bay: latest[0].bay, title, body: lines.join("\n"), urgent: waiting > 0 })
}

/// The first line of a reply, without markdown, short enough for a notification.
pub fn gist(text: &str) -> String {
    let line = text.lines().map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' ']).replace(['*', '`'], "")).find(|l| !l.trim().is_empty()).unwrap_or_default();
    let line = line.trim();
    if line.chars().count() > 120 { format!("{}…", line.chars().take(119).collect::<String>().trim_end()) } else { line.to_owned() }
}

#[cfg(windows)]
pub use windows_toast::{register, show};

#[cfg(windows)]
mod windows_toast {
    use super::Note;
    use crate::settings::config_dir;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};

    /// Who the notifications are from (the app's identifier).
    const AUMID: &str = "com.graysense.terminal";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn set(name: &str, value: &str) -> bool {
        let key = wide(&format!(r"Software\Classes\AppUserModelId\{AUMID}"));
        let (name, value) = (wide(name), wide(value));
        // Creates the key when it isn't there yet.
        unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32) == 0 }
    }

    /// Give Windows the name and icon notifications show. Unpackaged apps need this (or a Start
    /// menu shortcut with the same id) for their notifications to appear at all.
    pub fn register() {
        let icon = config_dir().join("notification-icon.png");
        const PNG: &[u8] = include_bytes!("../icons/128x128.png");
        if std::fs::metadata(&icon).map(|m| m.len()).ok() != Some(PNG.len() as u64) {
            let _ = std::fs::write(&icon, PNG);
        }
        if !(set("DisplayName", "Graysense") && set("IconUri", &icon.display().to_string())) {
            tracing::warn!("couldn't register Graysense for notifications");
        }
    }

    /// Show a notification; `on_click` runs when the user clicks it.
    pub fn show(note: &Note, on_click: impl Fn() + Send + 'static) {
        let shown = tauri_winrt_notification::Toast::new(AUMID)
            .title(&note.title)
            .text1(&note.body)
            .on_activated(move |_| {
                on_click();
                Ok(())
            })
            .show();
        if let Err(e) = shown {
            tracing::warn!("couldn't show a notification: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(bay: BayId, body: &str, urgent: bool) -> Note {
        Note { bay, title: format!("Agent {bay}"), body: body.into(), urgent }
    }

    #[test]
    fn combine_keeps_each_agents_latest_news_and_puts_waiting_first() {
        assert_eq!(combine(Vec::new()), None);
        assert_eq!(combine(vec![note(1, "Finished", false)]), Some(note(1, "Finished", false)));
        // A question isn't hidden by the turn ending after it.
        assert_eq!(combine(vec![note(1, "Needs approval", true), note(1, "Finished", false)]), Some(note(1, "Needs approval", true)));
        let all = combine(vec![note(1, "Finished", false), note(2, "Needs approval", true), note(3, "Finished", false)]).unwrap();
        assert_eq!((all.bay, all.title.as_str(), all.urgent), (2, "1 need you, 2 finished", true));
        assert_eq!(all.body, "Agent 2: Needs approval\nAgent 1: Finished\nAgent 3: Finished");
        let many = combine((1..=5).map(|b| note(b, "Finished", false)).collect()).unwrap();
        assert_eq!(many.title, "5 agents finished");
        assert!(many.body.ends_with("\nand 2 more"));
    }

    #[test]
    fn gist_takes_the_first_line_without_markdown() {
        assert_eq!(gist("\n## **Done.** All `42` tests pass.\nMore"), "Done. All 42 tests pass.");
        assert_eq!(gist("- item"), "item");
        let long = gist(&"word ".repeat(40));
        assert!(long.ends_with('…') && long.chars().count() <= 120);
    }
}
