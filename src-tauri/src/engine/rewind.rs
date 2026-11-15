//! Rewinding a conversation to one of its messages: undo it (and everything after it), send it
//! again changed, or retry it (perhaps on another model). The runtime drops the turns and, when
//! the session tracked the agent's file changes, puts the files they changed back as they were.

use std::path::PathBuf;

use super::Core;
use super::actions::Action;
use super::dto::FillDto;
use crate::agent::backend::Cmd;
use crate::agent::{BayId, Item, Level, Rewind, RewindPreview, RewindReport, RewindThen};

/// The runtime's errors, in words the user can act on.
fn friendly(error: &str) -> String {
    if error.contains("not a current root user turn") {
        "Copilot can't rewind to this message (it was sent while the agent was working). Try an earlier one.".into()
    } else {
        format!("Couldn't rewind: {error}")
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// What a finished rewind did, for a toast (None: nothing worth saying).
fn summary(then: &RewindThen, messages: usize, report: &RewindReport) -> Option<(Level, String)> {
    let mut parts = Vec::new();
    if matches!(then, RewindThen::Undo) {
        parts.push(format!("Undid {}.", count(messages, "message")));
    }
    if !report.restored.is_empty() {
        parts.push(format!("Put back {}.", count(report.restored.len(), "file")));
    }
    let changed = report.skipped.iter().filter(|(_, why)| *why == "changed since").count();
    if changed > 0 {
        parts.push(format!("Left {} you changed since.", count(changed, "file")));
    }
    let other = report.skipped.len() - changed;
    if other > 0 {
        parts.push(format!("Couldn't put back {}.", count(other, "file")));
    }
    let level = if report.skipped.is_empty() { Level::Ok } else { Level::Warn };
    (!parts.is_empty()).then(|| (level, parts.join(" ")))
}

/// Attached files that are still there.
fn existing(files: &[String]) -> Vec<PathBuf> {
    files.iter().map(PathBuf::from).filter(|p| p.exists()).collect()
}

impl Core {
    /// Rewind the conversation to the message at `index` (`then`: "undo", "edit" or "retry"):
    /// first ask the runtime what it would do to files, for the user to confirm.
    pub(super) fn rewind_start(&mut self, bay: BayId, index: usize, then: &str, model: Option<String>) -> Result<(), String> {
        let then = match then {
            "undo" => RewindThen::Undo,
            "edit" => RewindThen::Edit { text: String::new() },
            "retry" => RewindThen::Retry { model: model.filter(|m| !m.trim().is_empty()) },
            other => return Err(format!("Unknown rewind: {other}")),
        };
        let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
        if v.rewind.as_ref().is_some_and(|r| r.running) {
            return Err("Still rewinding.".into());
        }
        if !v.can_rewind() {
            return Err("Wait for the agent to finish first.".into());
        }
        let (event_id, text, files) = v.rewind_point(index)?;
        v.rewind = Some(Rewind { index, event_id: event_id.clone(), text, files, then, preview: None, running: false });
        self.backend.send(Cmd::RewindPreview { bay, event_id });
        Ok(())
    }

    /// Go ahead with the rewind being confirmed (`text`: the changed message, when editing).
    pub(super) fn rewind_confirm(&mut self, bay: BayId, restore_files: bool, text: Option<String>) -> Result<(), String> {
        let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
        let ready = v.can_rewind();
        let r = v.rewind.as_mut().ok_or("Nothing to rewind.")?;
        if r.running {
            return Ok(());
        }
        let preview = r.preview.as_ref().ok_or("Still checking which files it would put back.")?;
        if !ready {
            return Err("Wait for the agent to finish first.".into());
        }
        if let RewindThen::Edit { text: edited } = &mut r.then {
            let text = text.unwrap_or_default().trim_end().to_owned();
            if text.trim().is_empty() {
                return Err("Write a message first.".into());
            }
            *edited = text;
        }
        let restore_files = restore_files && preview.can_restore && !preview.files.is_empty();
        r.running = true;
        let event_id = r.event_id.clone();
        self.backend.send(Cmd::Rewind { bay, event_id, restore_files });
        Ok(())
    }

    pub(super) fn rewind_cancel(&mut self, bay: BayId) {
        if let Some(v) = self.agent_mut(bay) {
            v.rewind.take_if(|r| !r.running);
        }
    }

    /// The runtime said what rewinding would do to files.
    pub(super) fn rewind_previewed(&mut self, bay: BayId, event_id: String, result: Result<RewindPreview, String>) {
        let Some(v) = self.agent_mut(bay) else { return };
        let Some(r) = v.rewind.as_mut().filter(|r| r.event_id == event_id && r.preview.is_none()) else { return };
        match result {
            Ok(preview) => {
                // Retrying a turn that changed no files needs no confirming.
                let quiet = matches!(r.then, RewindThen::Retry { .. }) && preview.files.is_empty();
                r.preview = Some(preview);
                if quiet && let Err(e) = self.rewind_confirm(bay, false, None) {
                    if let Some(v) = self.agent_mut(bay) {
                        v.rewind = None;
                    }
                    self.toast(Level::Error, e);
                }
            }
            Err(e) => {
                v.rewind = None;
                self.toast(Level::Error, friendly(&e));
            }
        }
    }

    /// The runtime rewound the conversation (or couldn't): the transcript follows, and the
    /// message goes back in the message box or is sent again.
    pub(super) fn rewound(&mut self, bay: BayId, event_id: String, result: Result<RewindReport, String>) {
        let Some(v) = self.agent_mut(bay) else { return };
        let Some(r) = v.rewind.take_if(|r| r.event_id == event_id) else { return };
        let report = match result {
            Ok(report) if report.rewound => report,
            Ok(report) => {
                let mut text = report.problem.unwrap_or_else(|| "Copilot couldn't rewind the conversation.".into());
                if !report.restored.is_empty() {
                    text.push_str(&format!(" {} put back anyway.", count(report.restored.len(), "file")));
                }
                self.toast(Level::Error, text);
                return;
            }
            Err(e) => {
                self.toast(Level::Error, friendly(&e));
                return;
            }
        };
        // The transcript may have moved on since (cleared…): find the message again.
        let at = v.items.iter().position(|i| matches!(i, Item::User { event_id: Some(id), .. } if *id == event_id));
        let messages = at.map_or(0, |at| v.items[at..].iter().filter(|i| matches!(i, Item::User { .. })).count());
        match at {
            Some(at) => v.truncate_at(at),
            None => v.truncate_at(v.items.len()),
        }
        if let Some((level, text)) = summary(&r.then, messages.max(1), &report) {
            self.toast(level, text);
        }
        match r.then {
            RewindThen::Undo => self.give_back(bay, r.text, &r.files),
            RewindThen::Edit { text } => self.resend(bay, text, &r.files),
            RewindThen::Retry { model } => {
                if let Some(model) = model.filter(|m| self.agent(bay).is_some_and(|v| v.model.as_deref() != Some(m.as_str()))) {
                    if let Err(e) = self.act(Action::SetModel { bay, model, effort: None, context: None }) {
                        self.toast(Level::Error, e);
                    }
                }
                self.resend(bay, r.text, &r.files);
            }
        }
    }

    /// Put a message back in the message box, with its files attached again.
    pub(super) fn give_back(&mut self, bay: BayId, text: String, files: &[String]) {
        let Some(v) = self.agent_mut(bay) else { return };
        for p in existing(files) {
            if !v.attachments.contains(&p) {
                v.attachments.push(p);
            }
        }
        self.fills_new.push(FillDto { bay, text });
    }

    /// Send a message again with the files it had (leaving what's attached for the next one).
    fn resend(&mut self, bay: BayId, text: String, files: &[String]) {
        let Some(v) = self.agent_mut(bay) else { return };
        let staged = std::mem::replace(&mut v.attachments, existing(files));
        let sent = self.send(bay, text.clone(), false);
        if let Some(v) = self.agent_mut(bay) {
            let unsent = std::mem::replace(&mut v.attachments, staged);
            for p in unsent {
                if !v.attachments.contains(&p) {
                    v.attachments.push(p);
                }
            }
        }
        if let Err(e) = sent {
            self.toast(Level::Error, e);
            self.fills_new.push(FillDto { bay, text });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_says_what_undo_did() {
        let report = RewindReport {
            rewound: true,
            restored: vec!["a.rs".into(), "b.rs".into()],
            skipped: vec![("c.rs".into(), "changed since")],
            problem: None,
        };
        let (level, text) = summary(&RewindThen::Undo, 3, &report).unwrap();
        assert_eq!(text, "Undid 3 messages. Put back 2 files. Left 1 file you changed since.");
        assert!(matches!(level, Level::Warn));
    }

    #[test]
    fn retry_without_files_says_nothing() {
        let report = RewindReport { rewound: true, ..Default::default() };
        assert!(summary(&RewindThen::Retry { model: None }, 1, &report).is_none());
        let (_, text) = summary(&RewindThen::Undo, 1, &report).unwrap();
        assert_eq!(text, "Undid 1 message.");
    }

    #[test]
    fn friendly_explains_steered_messages() {
        assert!(friendly("Rewind event 'x' is not a current root user turn").contains("earlier one"));
        assert_eq!(friendly("boom"), "Couldn't rewind: boom");
    }
}
