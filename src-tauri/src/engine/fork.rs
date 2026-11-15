//! Forking a conversation: copying a session (all of it, or up to one of its messages) into a new
//! one that goes its own way. The copy works in the same folder: files aren't copied.

use std::sync::atomic::Ordering;

use super::Core;
use crate::agent::backend::{Cmd, ForkJob};
use crate::agent::{BayId, Level};

/// The copy's name.
fn fork_name(title: &str) -> String {
    format!("Fork of {}", title.trim()).chars().take(100).collect()
}

impl Core {
    /// Fork the session in `bay` into a new one: up to the message at `index` (which goes in the
    /// new session's message box, to send or change), or all of it.
    pub(super) fn fork(&mut self, bay: BayId, index: Option<usize>) -> Result<(), String> {
        let v = self.agent(bay).ok_or("That agent is closed.")?;
        let session_id = v.session_id.clone().filter(|_| !v.deferred).ok_or("There's nothing to fork yet: send a message first.")?;
        let (before_event, text, files) = match index {
            Some(index) => {
                let (event_id, text, files) = v.rewind_point(index)?;
                (Some(event_id), Some(text), files)
            }
            // A turn half done would be copied half done.
            None if v.status.is_busy() => return Err("Wait for the agent to finish, or fork from one of your messages.".into()),
            None => (None, None, Vec::new()),
        };
        let name = fork_name(&v.display_title());
        self.backend.send(Cmd::Fork { job: ForkJob { bay, session_id, before_event, name, text, files } });
        Ok(())
    }

    /// The runtime copied the session: open the copy, set up like the original.
    pub(super) fn forked(&mut self, job: ForkJob, result: Result<String, String>) {
        let id = match result {
            Ok(id) => id,
            Err(e) => {
                self.toast(Level::Error, format!("Couldn't fork the session: {e}"));
                return;
            }
        };
        let bay = match self.agent(job.bay) {
            // The original closed meanwhile: open the copy like any past session.
            None => {
                self.session_index.entry(&id).name = Some(job.name.clone());
                self.resume_session(id)
            }
            Some(v) => {
                let (cwd, incognito) = (v.cwd.clone(), v.incognito);
                let (model, effort, tier) = (v.model.clone(), v.effort.clone(), v.context_tier.clone());
                let auto_approve = v.auto_approve.load(Ordering::Relaxed);
                let name = job.name.clone();
                let bay = self.open_agent(cwd.clone(), None, Some(id.clone()), None, incognito, move |v| {
                    v.model = model;
                    v.effort = effort;
                    v.context_tier = tier;
                    v.auto_approve.store(auto_approve, Ordering::Relaxed);
                    v.name = Some(name);
                });
                let meta = self.session_index.entry(&id);
                meta.name = Some(job.name.clone());
                meta.folder = Some(cwd.display().to_string());
                bay
            }
        };
        self.dirty.sessions = true;
        if let Some(text) = job.text {
            self.give_back(bay, text, &job.files);
        }
        self.toast(Level::Ok, "Forked into a new session. It works in the same folder as the original, so file changes show up in both.");
    }
}
