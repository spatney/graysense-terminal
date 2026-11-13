//! Mission Board: a scratchpad of ideas you can delegate to Copilot agents.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::BayId;
use crate::settings::{config_dir, read_json, write_json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Idea,
    Active,
    Review,
    Done,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mission {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub dir: Option<String>,
    pub status: Status,
    pub created: i64,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub from_agent: bool,
    #[serde(skip)]
    pub bay: Option<BayId>,
}

impl Mission {
    pub fn brief(&self) -> String {
        if self.notes.trim().is_empty() { self.title.clone() } else { format!("{}\n\n{}", self.title, self.notes) }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    missions: Vec<Mission>,
    next_id: u64,
}

pub struct MissionBoard {
    pub missions: Vec<Mission>,
    next_id: u64,
    dirty: bool,
}

fn path() -> PathBuf {
    config_dir().join("missions.json")
}

/// A captured idea: the first line is the title, the rest are notes (" -- " also splits them).
pub fn split_capture(text: &str) -> (String, String) {
    let text = text.trim();
    if let Some((title, notes)) = text.split_once('\n') {
        return (title.trim().to_owned(), notes.trim().to_owned());
    }
    match text.split_once(" -- ") {
        Some((t, n)) => (t.trim().to_owned(), n.trim().to_owned()),
        None => (text.to_owned(), String::new()),
    }
}

impl MissionBoard {
    pub fn load() -> Self {
        let stored: Stored = read_json(&path()).unwrap_or_default();
        let next_id = stored.next_id.max(stored.missions.iter().map(|m| m.id + 1).max().unwrap_or(1));
        MissionBoard { missions: stored.missions, next_id, dirty: false }
    }

    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            let stored = Stored { missions: self.missions.clone(), next_id: self.next_id };
            write_json(&path(), &stored);
            self.dirty = false;
        }
    }

    pub fn add(&mut self, title: &str, notes: &str, from_agent: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.missions.insert(0, Mission {
            id,
            title: title.trim().to_owned(),
            notes: notes.trim().to_owned(),
            dir: None,
            status: Status::Idea,
            created: chrono::Utc::now().timestamp(),
            session_id: None,
            from_agent,
            bay: None,
        });
        self.dirty = true;
        id
    }

    pub fn get(&self, id: u64) -> Option<&Mission> {
        self.missions.iter().find(|m| m.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Mission> {
        self.dirty = true;
        self.missions.iter_mut().find(|m| m.id == id)
    }

    pub fn edit(&mut self, id: u64, title: &str, notes: &str) {
        if let Some(m) = self.get_mut(id)
            && !title.trim().is_empty()
        {
            m.title = title.trim().to_owned();
            m.notes = notes.trim().to_owned();
        }
    }

    pub fn set_status(&mut self, id: u64, status: Status) {
        if let Some(m) = self.get_mut(id) {
            m.status = status;
        }
    }

    pub fn remove(&mut self, id: u64) -> Option<Mission> {
        let i = self.missions.iter().position(|m| m.id == id)?;
        self.dirty = true;
        Some(self.missions.remove(i))
    }

    /// Put a removed mission back where it was (Undo).
    pub fn restore(&mut self, mission: Mission, index: usize) {
        let i = index.min(self.missions.len());
        self.missions.insert(i, mission);
        self.dirty = true;
    }

    pub fn set_dir(&mut self, id: u64, dir: Option<PathBuf>) {
        if let Some(m) = self.get_mut(id) {
            m.dir = dir.map(|d| d.display().to_string());
        }
    }

    /// Link a mission to an agent bay and mark it in progress.
    pub fn link(&mut self, id: u64, bay: BayId) {
        if let Some(m) = self.get_mut(id) {
            m.bay = Some(bay);
            m.status = Status::Active;
        }
    }

    pub fn on_session(&mut self, bay: BayId, session_id: &str) {
        for m in self.missions.iter_mut().filter(|m| m.bay == Some(bay)) {
            m.session_id = Some(session_id.to_owned());
            self.dirty = true;
        }
    }

    /// The linked agent finished a turn.
    pub fn on_agent_finished(&mut self, bay: BayId) {
        for m in self.missions.iter_mut().filter(|m| m.bay == Some(bay) && m.status == Status::Active) {
            m.status = Status::Review;
            self.dirty = true;
        }
    }

    pub fn on_bay_closed(&mut self, bay: BayId) {
        for m in self.missions.iter_mut().filter(|m| m.bay == Some(bay)) {
            m.bay = None;
            self.dirty = true;
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_ideas_split_into_title_and_notes() {
        assert_eq!(split_capture("Fix the login bug\nIt fails on CI\nSee #12"), ("Fix the login bug".into(), "It fails on CI\nSee #12".into()));
        assert_eq!(split_capture("Draft release notes -- summarize this week"), ("Draft release notes".into(), "summarize this week".into()));
        assert_eq!(split_capture("  Just a title  "), ("Just a title".into(), String::new()));
    }
}
