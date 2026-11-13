//! Learning from conversations: once an agent has been idle for a while (or closes), a cheap
//! model reads what was said and files what's worth remembering.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{BayKind, Core};
use crate::agent::backend::{Cmd, ModelInfo};
use crate::agent::{BayId, Item, Level};
use crate::memory::{Project, Source};
use crate::settings;

/// Seconds an agent must sit idle after a turn before its conversation is read.
pub const LEARN_DELAY: Duration = Duration::from_secs(60);

/// What a curator run was reading, so its answer can be filed correctly.
#[derive(Clone, Serialize, Deserialize)]
pub struct LearnJob {
    project_root: PathBuf,
    session: Option<String>,
    title: String,
    /// The curator message, kept so unfinished work survives a restart.
    message: String,
}

fn learn_queue_path() -> PathBuf {
    settings::config_dir().join("memory-queue.json")
}

impl Core {
    /// Read a conversation in the background and file what's worth keeping.
    pub(super) fn start_learning(&mut self, bay: BayId) {
        let model = self.curator_model();
        if self.learn_jobs.contains_key(&bay) {
            return;
        }
        let memory = self.memory.clone();
        let Some(v) = self.agent_mut(bay) else { return };
        if v.incognito {
            return;
        }
        let from = if v.learn_cursor == usize::MAX { v.items.len() } else { v.learn_cursor };
        v.learn_cursor = v.items.len();
        v.learn_due = None;
        let Some(transcript) = v.transcript_since(from) else { return };
        let transcript = crate::memory::redact_secrets(&transcript);
        let message = {
            let store = memory.lock();
            let existing = store.curator_context(Some(&v.project), &transcript, 40);
            crate::memory::curator_message(&existing, Some(&v.project), &transcript)
        };
        let job = LearnJob { project_root: v.project.root.clone(), session: v.session_id.clone(), title: v.display_title(), message: message.clone() };
        self.learn_jobs.insert(bay, job);
        self.dirty.memory = true;
        self.backend.send(Cmd::Learn { bay, model, message });
    }

    /// Conversations not yet read when the app closes are queued and read on the next launch.
    pub(super) fn queue_unread_conversations(&mut self) {
        if !self.settings.memory.learning() {
            return;
        }
        let mut queue: Vec<LearnJob> = self.learn_jobs.drain().map(|(_, j)| j).collect();
        for b in &self.bays {
            let BayKind::Agent(v) = &b.kind else { continue };
            if v.incognito || v.learn_cursor == usize::MAX {
                continue;
            }
            if let Some(transcript) = v.transcript_since(v.learn_cursor) {
                let transcript = crate::memory::redact_secrets(&transcript);
                let store = self.memory.lock();
                let existing = store.curator_context(Some(&v.project), &transcript, 40);
                let message = crate::memory::curator_message(&existing, Some(&v.project), &transcript);
                queue.push(LearnJob { project_root: v.project.root.clone(), session: v.session_id.clone(), title: v.display_title(), message });
            }
        }
        if !queue.is_empty() {
            settings::write_json(&learn_queue_path(), &queue);
        }
    }

    /// Pick up conversations queued by the previous run (once models are known).
    pub(super) fn resume_learn_queue(&mut self) {
        let path = learn_queue_path();
        let Some(queue) = settings::read_json::<Vec<LearnJob>>(&path) else { return };
        let _ = std::fs::remove_file(&path);
        if !self.settings.memory.learning() {
            return;
        }
        let model = self.curator_model();
        for job in queue.into_iter().take(6) {
            // A bay id no bay uses, so the answer is filed without a transcript to show it in.
            let bay = self.alloc_bay();
            self.backend.send(Cmd::Learn { bay, model: model.clone(), message: job.message.clone() });
            self.learn_jobs.insert(bay, job);
        }
        self.dirty.memory = true;
    }

    /// The model the curator should use: the user's pick, else the cheapest available.
    fn curator_model(&self) -> Option<String> {
        if let Some(m) = &self.settings.memory.model {
            return Some(m.clone());
        }
        let cost = |m: &ModelInfo| {
            let small = ["mini", "flash", "haiku", "nano", "lite"].iter().any(|k| m.id.to_lowercase().contains(k));
            (m.multiplier.unwrap_or(1.0) * 1000.0) as i64 * 2 + if small { 0 } else { 1 }
        };
        self.models.iter().min_by_key(|m| cost(m)).map(|m| m.id.clone())
    }

    /// Curator results: apply them, then tell the agent's transcript and the user.
    pub(super) fn finish_learning(&mut self, bay: BayId, result: Result<String, String>) {
        let Some(job) = self.learn_jobs.remove(&bay) else { return };
        self.dirty.memory = true;
        self.memory_last_run = Some((crate::memory::now(), result.as_ref().err().cloned()));
        let Ok(content) = result else { return };
        let project = Project::for_dir(&job.project_root);
        let ops = crate::memory::parse_curator(&content, Some(&project));
        tracing::info!(ops = ops.len(), "memory curator finished");
        if ops.is_empty() {
            return;
        }
        let source = Source { session: job.session, title: Some(job.title), quote: None, at: crate::memory::now() };
        let outcome = self.memory.lock().apply(ops, source, self.settings.memory.auto_accept());
        let saved = outcome.saved.len();
        if saved == 0 && outcome.suggested == 0 && outcome.updated == 0 {
            return;
        }
        let text = match (saved, outcome.saved.first()) {
            (1, Some((_, t))) => t.clone(),
            (n, _) if n > 1 => format!("{n} things from this conversation"),
            _ => format!("{} update{} to existing memories", outcome.updated, if outcome.updated == 1 { "" } else { "s" }),
        };
        if saved > 0 || outcome.updated > 0 {
            match self.agent_mut(bay) {
                Some(v) => {
                    v.push(Item::Memory { ids: outcome.saved.iter().map(|(id, _)| *id).collect(), text, learned: true, undone: false });
                }
                // The conversation is closed (or was queued at exit): say it here instead.
                None => self.toast(Level::Ok, format!("Learned: {text}")),
            }
        }
        if outcome.suggested > 0 {
            let n = outcome.suggested;
            self.toast_with(
                Level::Info,
                format!("{n} new memor{} to review", if n == 1 { "y" } else { "ies" }),
                Some(("Review".into(), serde_json::json!({ "type": "setSide", "panel": "memory" }))),
            );
        }
    }

    /// Due curator runs and approval learning.
    pub(super) fn memory_tick(&mut self) {
        crate::memory::set_switches(self.settings.memory.enabled, self.settings.memory.inject);
        let learning = self.settings.memory.learning();
        let now = Instant::now();
        let mut due = Vec::new();
        let mut approvals = Vec::new();
        for b in self.bays.iter_mut() {
            let BayKind::Agent(v) = &mut b.kind else { continue };
            if !learning || v.incognito {
                v.learn_due = None;
                v.approval_log.clear();
                // Nothing said while learning is off is ever learned later.
                if v.learn_cursor != usize::MAX {
                    v.learn_cursor = v.items.len();
                }
                continue;
            }
            if v.learn_due.is_some_and(|t| now >= t) && !v.status.is_busy() && !v.status.needs_attention() {
                due.push(b.id);
            }
            for (cmds, ok) in v.approval_log.drain(..) {
                approvals.push((v.project.clone(), cmds, ok));
            }
        }
        for bay in due {
            self.start_learning(bay);
        }
        for (project, cmds, ok) in approvals {
            let suggestion = self.memory.lock().record_approval(&project, &cmds, ok);
            if let Some(s) = suggestion {
                self.toast_with(Level::Info, format!("Suggested rule: {s}"), Some(("Review".into(), serde_json::json!({ "type": "setSide", "panel": "memory" }))));
            }
        }
    }
}
