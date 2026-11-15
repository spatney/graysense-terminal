//! The three-way merge: what the repository held at the last sync (`base`), what this PC has now
//! (`local`) and what the repository has now (`remote`), item by item.
//!
//! Nothing here touches the disk or the network. The result is the state both sides should end
//! up in, and the changes that bring this PC there. The rules, in the order they matter:
//!
//! - What only one side changed is taken. A change made on both sides to the same thing is settled
//!   by who owns it (what the user wrote, pinned or saved as a note beats what was learned) and then
//!   by which is newer; the loser's words are kept in the winner's history.
//! - Deleting beats an untouched copy, but never the user's own words: something they wrote that
//!   the other PC changed after it was deleted here comes back.
//! - Memories new on both PCs that say the same thing (the same words, or nearly) become one.
//! - What the user deleted here doesn't come back from the other PC under another id.
//! - A repository file that couldn't be read is not a deletion: this PC's copy stays.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::items::{Base, Item, ItemAttachment, MissionItem, Snapshot};
use super::{Options, Report, hash_value};
use crate::memory::{self, Kind, Revision};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Combine this PC with the repository.
    Merge,
    /// Make this PC match the repository.
    UseRemote,
}

/// A change to make to this PC's memories.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Add(Item),
    Update(Item),
    /// `merged`: it was combined into another memory, not removed on another PC.
    Remove { uid: String, merged: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum MissionChange {
    Add(MissionItem),
    Update(MissionItem),
    Remove(String),
}

pub struct Inputs<'a> {
    pub base: &'a Base,
    pub local: &'a Snapshot,
    pub remote: &'a Snapshot,
    /// uids of repository files that couldn't be read: left alone, and this PC's copy kept.
    pub unreadable: &'a HashSet<String>,
    /// Texts the user deleted recently (lower-case), which must not come back from another PC.
    pub rejected: &'a [String],
    /// What a fresh install has for each synced preference.
    pub defaults: &'a BTreeMap<String, Value>,
    pub options: Options,
    pub strategy: Strategy,
}

pub struct Merged {
    /// The state both sides should end up in.
    pub snapshot: Snapshot,
    pub changes: Vec<Change>,
    pub mission_changes: Vec<MissionChange>,
    pub setting_changes: BTreeMap<String, Value>,
    pub report: Report,
    /// Knowledge new on both PCs and not alike enough to combine by text: the AI's to look at.
    pub candidates: Vec<Candidate>,
}

/// A memory waiting for the AI to compare it with what the other PC learned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub uid: String,
    /// It came from the repository (another PC) rather than this one.
    pub elsewhere: bool,
}

pub fn merge(inp: &Inputs) -> Merged {
    let mut report = Report::default();
    let mut notes = Vec::new();
    let (memories, merged_away, candidates) = merge_memories(inp, &mut report, &mut notes);
    let missions = if inp.options.missions { merge_missions(inp, &mut report) } else { inp.remote.missions.clone() };
    let (settings, setting_changes) = if inp.options.settings { merge_settings(inp) } else { (inp.remote.settings.clone(), BTreeMap::new()) };

    let changes = diff_memories(&inp.local.memories, &memories, &merged_away);
    let mission_changes = if inp.options.missions { diff_missions(&inp.local.missions, &missions) } else { Vec::new() };

    report.pulled = changes.iter().filter(|c| !matches!(c, Change::Remove { .. })).count() + mission_changes.iter().filter(|c| !matches!(c, MissionChange::Remove(_))).count() + setting_changes.len();
    report.removed = changes.iter().filter(|c| matches!(c, Change::Remove { .. })).count() + mission_changes.iter().filter(|c| matches!(c, MissionChange::Remove(_))).count();
    report.pushed = changed_files(inp.remote, &Snapshot { memories: memories.clone(), missions: missions.clone(), settings: settings.clone() }, inp.options);
    report.notes = notes;
    Merged { snapshot: Snapshot { memories, missions, settings }, changes, mission_changes, setting_changes, report, candidates }
}

/// How many files of the repository differ between two states: added, changed or removed.
pub fn changed_files(before: &Snapshot, after: &Snapshot, options: Options) -> usize {
    count_differences(&before.memories, &after.memories, |i| i.hash())
        + if options.missions { count_differences(&before.missions, &after.missions, |m| m.hash()) } else { 0 }
        + usize::from(options.settings && before.settings != after.settings)
}

/// A sync that removes at least this many items, and at least half of what was synced, is
/// asked about first.
const MASS_REMOVAL: usize = 5;

/// What a merge would remove of what the last sync knew: the check that keeps one PC's failure
/// from becoming everyone's loss. A repository deleted and made again, a memory file that
/// wouldn't load, "forget everything": each looks to a merge like everything was deleted
/// somewhere, and the merge would pass that on to the other side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removals {
    /// Gone from the repository since the last sync (this PC would remove them too).
    pub here: BTreeSet<String>,
    /// Gone from this PC since the last sync (the repository would lose them).
    pub there: BTreeSet<String>,
    /// What the last sync knew of.
    pub known: usize,
}

impl Removals {
    /// Most of what was synced is going at once: that's a wipe, not an edit.
    pub fn mass(&self) -> bool {
        let n = self.here.len() + self.there.len();
        n >= MASS_REMOVAL && n * 2 >= self.known
    }

    pub fn uids(&self) -> impl Iterator<Item = &String> {
        self.here.iter().chain(&self.there)
    }
}

/// The items the last sync knew that `merged` drops because only one side still has them.
pub fn removals(inp: &Inputs, merged: &Merged) -> Removals {
    if inp.strategy == Strategy::UseRemote {
        return Removals::default();
    }
    let mut out = Removals { known: inp.base.memories.len() + if inp.options.missions { inp.base.missions.len() } else { 0 }, ..Removals::default() };
    let mut note = |uid: &String, local: bool, remote: bool| match (local, remote) {
        (true, false) => {
            out.here.insert(uid.clone());
        }
        (false, true) => {
            out.there.insert(uid.clone());
        }
        _ => {}
    };
    for uid in inp.base.memories.keys().filter(|u| !merged.snapshot.memories.contains_key(*u)) {
        note(uid, inp.local.memories.contains_key(uid), inp.remote.memories.contains_key(uid));
    }
    if inp.options.missions {
        for uid in inp.base.missions.keys().filter(|u| !merged.snapshot.missions.contains_key(*u)) {
            note(uid, inp.local.missions.contains_key(uid), inp.remote.missions.contains_key(uid));
        }
    }
    out
}

// ------------------------------------------------------------------ memories

fn merge_memories(inp: &Inputs, report: &mut Report, notes: &mut Vec<String>) -> (BTreeMap<String, Item>, HashSet<String>, Vec<Candidate>) {
    if inp.strategy == Strategy::UseRemote {
        return (inp.remote.memories.clone(), HashSet::new(), Vec::new());
    }
    let (base, local, remote) = (&inp.base.memories, &inp.local.memories, &inp.remote.memories);
    let mut result: BTreeMap<String, Item> = BTreeMap::new();
    let mut new_local: BTreeSet<String> = BTreeSet::new();
    let mut new_remote: BTreeSet<String> = BTreeSet::new();
    let mut left_out = 0;

    let uids: BTreeSet<&String> = base.keys().chain(local.keys()).chain(remote.keys()).collect();
    for uid in uids {
        let was = base.get(uid);
        match (local.get(uid), remote.get(uid)) {
            (Some(l), Some(r)) => {
                let (lh, rh) = (l.hash(), r.hash());
                if lh == rh {
                    // The repository's copy, so its file stays as it is.
                    result.insert(uid.clone(), r.clone());
                } else if was == Some(&lh) {
                    result.insert(uid.clone(), r.clone());
                } else if was == Some(&rh) {
                    result.insert(uid.clone(), l.clone());
                } else {
                    result.insert(uid.clone(), settle(l, r));
                    report.conflicts += 1;
                }
            }
            (Some(l), None) => {
                if inp.unreadable.contains(uid) {
                    result.insert(uid.clone(), l.clone());
                    continue;
                }
                match was {
                    None => {
                        result.insert(uid.clone(), l.clone());
                        new_local.insert(uid.clone());
                    }
                    // Deleted elsewhere and untouched here: it goes.
                    Some(h) if *h == l.hash() => {}
                    // Changed here after it was deleted elsewhere: the user's words stay.
                    Some(_) => {
                        report.conflicts += 1;
                        if l.protected() {
                            result.insert(uid.clone(), l.clone());
                        }
                    }
                }
            }
            (None, Some(r)) => match was {
                None => {
                    if !r.protected() && r.kind().is_knowledge() && is_rejected(&r.text, inp.rejected) {
                        left_out += 1;
                        continue;
                    }
                    result.insert(uid.clone(), r.clone());
                    new_remote.insert(uid.clone());
                }
                // Deleted here and untouched there: it goes from the repository too.
                Some(h) if *h == r.hash() => {}
                Some(_) => {
                    report.conflicts += 1;
                    if r.protected() {
                        result.insert(uid.clone(), r.clone());
                    }
                }
            },
            (None, None) => {}
        }
    }
    if left_out > 0 {
        notes.push(format!("{left_out} {} from the repository {} left out because you deleted {} here.", plural(left_out, "memory", "memories"), if left_out == 1 { "was" } else { "were" }, if left_out == 1 { "it" } else { "them" }));
    }

    // The same thing said on both PCs since the last sync becomes one memory.
    let mut merged_away = HashSet::new();
    for (l_uid, r_uid) in duplicate_pairs(&result, &new_local, &new_remote) {
        let (Some(l), Some(r)) = (result.get(&l_uid).cloned(), result.get(&r_uid).cloned()) else { continue };
        result.remove(&l_uid);
        result.remove(&r_uid);
        let l_better = (l.protected(), l.updated) > (r.protected(), r.updated);
        let survivor = if l_better { combine(&l, &r) } else { combine(&r, &l) };
        new_local.remove(&l_uid);
        new_remote.remove(&r_uid);
        merged_away.insert(if l_better { r_uid } else { l_uid });
        result.insert(survivor.uid.clone(), survivor);
        report.combined += 1;
    }

    let candidates = ai_candidates(&result, &new_local, &new_remote);
    (result, merged_away, candidates)
}

/// Both PCs changed the same memory: the one that is the user's, then the newer, wins.
fn settle(l: &Item, r: &Item) -> Item {
    if (l.protected(), l.updated) > (r.protected(), r.updated) { combine(l, r) } else { combine(r, l) }
}

/// `winner` with what `loser` adds: its words kept in the history, its pin, votes, files and
/// sources, and the earlier creation date.
fn combine(winner: &Item, loser: &Item) -> Item {
    let mut out = winner.clone();
    out.pinned |= loser.pinned;
    out.votes = out.votes.max(loser.votes);
    out.confidence = out.confidence.max(loser.confidence);
    out.created = out.created.min(loser.created);
    out.updated = out.updated.max(loser.updated);
    if loser.origin == "user" {
        out.origin = "user".into();
    }
    if norm(&loser.text) != norm(&winner.text) && !out.history.iter().any(|r| r.text == loser.text) {
        out.history.push(Revision { text: loser.text.clone(), at: loser.updated, why: "merged".into() });
        let keep = out.history.len().saturating_sub(8);
        out.history.drain(..keep);
    }
    for s in &loser.sources {
        if !out.sources.contains(s) {
            out.sources.push(s.clone());
        }
    }
    out.sources.sort_by_key(|s| s.at);
    let keep = out.sources.len().saturating_sub(5);
    out.sources.drain(..keep);
    let have: HashSet<String> = out.attachments.iter().map(ItemAttachment::ident).collect();
    out.attachments.extend(loser.attachments.iter().filter(|a| !have.contains(&a.ident())).cloned());
    out
}

fn is_rejected(text: &str, rejected: &[String]) -> bool {
    let lower = text.to_lowercase();
    rejected.iter().any(|r| *r == lower || memory::jaccard(r, text) >= 0.8)
}

/// Text compared loosely: case, spacing and closing punctuation don't count.
fn norm(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase().trim_end_matches(['.', ',', ';', ':', '!', '?']).to_owned()
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// Pairs (this PC's uid, the repository's uid) of memories that are the same thing, best first,
/// each memory in at most one pair.
fn duplicate_pairs(result: &BTreeMap<String, Item>, new_local: &BTreeSet<String>, new_remote: &BTreeSet<String>) -> Vec<(String, String)> {
    struct Words {
        scope: String,
        kind: Kind,
        norm: String,
        tokens: HashSet<String>,
    }
    let words = |uid: &String| -> Option<Words> {
        let i = result.get(uid)?;
        Some(Words { scope: i.scope.id(), kind: i.kind(), norm: norm(&i.text), tokens: memory::tokens(&i.text).into_iter().collect() })
    };
    let remote: Vec<(&String, Words)> = new_remote.iter().filter_map(|u| words(u).map(|w| (u, w))).collect();
    // Only memories in the same scope can be the same thing: look each up by scope.
    let mut remote_in: HashMap<&str, Vec<&(&String, Words)>> = HashMap::new();
    for entry in &remote {
        remote_in.entry(entry.1.scope.as_str()).or_default().push(entry);
    }
    let mut scored: Vec<(f32, &String, &String)> = Vec::new();
    for l_uid in new_local {
        let Some(l) = words(l_uid) else { continue };
        for (r_uid, r) in remote_in.get(l.scope.as_str()).into_iter().flatten().map(|e| (e.0, &e.1)) {
            let score = match (l.kind, r.kind) {
                (Kind::Note, Kind::Note) | (Kind::Episode, Kind::Episode) => (l.norm == r.norm).then_some(1.0),
                (Kind::Note, _) | (_, Kind::Note) | (Kind::Episode, _) | (_, Kind::Episode) => None,
                _ if l.norm == r.norm => Some(1.0),
                _ => {
                    let both = l.tokens.intersection(&r.tokens).count();
                    let all = l.tokens.union(&r.tokens).count();
                    let j = if all == 0 { 0.0 } else { both as f32 / all as f32 };
                    (j >= 0.8).then_some(j)
                }
            };
            if let Some(s) = score {
                scored.push((s, l_uid, r_uid));
            }
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)).then_with(|| a.2.cmp(b.2)));
    let (mut used_l, mut used_r) = (HashSet::new(), HashSet::new());
    let mut out = Vec::new();
    for (_, l, r) in scored {
        if used_l.contains(l) || used_r.contains(r) {
            continue;
        }
        used_l.insert(l);
        used_r.insert(r);
        out.push((l.clone(), r.clone()));
    }
    out
}

/// What's new on both PCs in the same scope, as knowledge: worth the AI comparing, since two PCs
/// that each learned something may have learned the same thing in different words, or
/// disagreed. When only one PC has anything new there's nothing to compare it with.
fn ai_candidates(result: &BTreeMap<String, Item>, new_local: &BTreeSet<String>, new_remote: &BTreeSet<String>) -> Vec<Candidate> {
    let mut groups: BTreeMap<String, (Vec<Candidate>, bool, bool)> = BTreeMap::new();
    for (uids, elsewhere) in [(new_local, false), (new_remote, true)] {
        for uid in uids {
            let Some(i) = result.get(uid).filter(|i| i.kind().is_knowledge()) else { continue };
            let g = groups.entry(i.scope.id()).or_default();
            g.0.push(Candidate { uid: uid.clone(), elsewhere });
            if elsewhere {
                g.2 = true;
            } else {
                g.1 = true;
            }
        }
    }
    groups.into_values().filter(|(_, l, r)| *l && *r).flat_map(|(c, _, _)| c).collect()
}

fn diff_memories(local: &BTreeMap<String, Item>, result: &BTreeMap<String, Item>, merged_away: &HashSet<String>) -> Vec<Change> {
    let mut out = Vec::new();
    for (uid, item) in result {
        match local.get(uid) {
            None => out.push(Change::Add(item.clone())),
            Some(l) if l.hash() != item.hash() => out.push(Change::Update(item.clone())),
            Some(_) => {}
        }
    }
    for uid in local.keys().filter(|u| !result.contains_key(*u)) {
        out.push(Change::Remove { uid: uid.clone(), merged: merged_away.contains(uid) });
    }
    out
}

fn count_differences<T>(before: &BTreeMap<String, T>, after: &BTreeMap<String, T>, hash: impl Fn(&T) -> String) -> usize {
    let changed = after.iter().filter(|(uid, item)| before.get(*uid).is_none_or(|b| hash(b) != hash(item))).count();
    changed + before.keys().filter(|uid| !after.contains_key(*uid)).count()
}

// ------------------------------------------------------------------ missions

fn merge_missions(inp: &Inputs, report: &mut Report) -> BTreeMap<String, MissionItem> {
    if inp.strategy == Strategy::UseRemote {
        return inp.remote.missions.clone();
    }
    let (base, local, remote) = (&inp.base.missions, &inp.local.missions, &inp.remote.missions);
    let mut result: BTreeMap<String, MissionItem> = BTreeMap::new();
    let mut new_local: BTreeSet<String> = BTreeSet::new();
    let mut new_remote: BTreeSet<String> = BTreeSet::new();
    let uids: BTreeSet<&String> = base.keys().chain(local.keys()).chain(remote.keys()).collect();
    for uid in uids {
        let was = base.get(uid);
        match (local.get(uid), remote.get(uid)) {
            (Some(l), Some(r)) => {
                let (lh, rh) = (l.hash(), r.hash());
                if lh == rh || was == Some(&lh) {
                    result.insert(uid.clone(), r.clone());
                } else if was == Some(&rh) {
                    result.insert(uid.clone(), l.clone());
                } else {
                    // An idea edited on both PCs: this PC's version stays.
                    result.insert(uid.clone(), l.clone());
                    report.conflicts += 1;
                }
            }
            (Some(l), None) => {
                if inp.unreadable.contains(uid) {
                    result.insert(uid.clone(), l.clone());
                    continue;
                }
                match was {
                    None => {
                        result.insert(uid.clone(), l.clone());
                        new_local.insert(uid.clone());
                    }
                    Some(h) if *h == l.hash() => {}
                    // Edited here after it was removed elsewhere: ideas aren't dropped.
                    Some(_) => {
                        result.insert(uid.clone(), l.clone());
                        report.conflicts += 1;
                    }
                }
            }
            (None, Some(r)) => match was {
                None => {
                    result.insert(uid.clone(), r.clone());
                    new_remote.insert(uid.clone());
                }
                Some(h) if *h == r.hash() => {}
                Some(_) => {
                    result.insert(uid.clone(), r.clone());
                    report.conflicts += 1;
                }
            },
            (None, None) => {}
        }
    }
    // The same idea captured on both PCs: the repository's copy stays.
    let key = |m: &MissionItem| format!("{}\u{1f}{}", norm(&m.title), norm(&m.notes));
    let remote_keys: HashSet<String> = new_remote.iter().filter_map(|u| result.get(u).map(key)).collect();
    let duplicates: Vec<String> = new_local.iter().filter(|u| result.get(*u).is_some_and(|m| remote_keys.contains(&key(m)))).cloned().collect();
    for uid in duplicates {
        result.remove(&uid);
        report.combined += 1;
    }
    result
}

fn diff_missions(local: &BTreeMap<String, MissionItem>, result: &BTreeMap<String, MissionItem>) -> Vec<MissionChange> {
    let mut out = Vec::new();
    for (uid, m) in result {
        match local.get(uid) {
            None => out.push(MissionChange::Add(m.clone())),
            Some(l) if l.hash() != m.hash() => out.push(MissionChange::Update(m.clone())),
            Some(_) => {}
        }
    }
    for uid in local.keys().filter(|u| !result.contains_key(*u)) {
        out.push(MissionChange::Remove(uid.clone()));
    }
    out
}

// ------------------------------------------------------------------ preferences

/// Preferences key by key. What only one side changed since the last sync is taken. Changed on
/// both, this PC's choice stays. With no history to go by (the first sync) a preference this PC
/// never moved off its default takes the repository's value, so a fresh install doesn't overwrite
/// the settings it's about to receive.
fn merge_settings(inp: &Inputs) -> (BTreeMap<String, Value>, BTreeMap<String, Value>) {
    let (local, remote) = (&inp.local.settings, &inp.remote.settings);
    let mut result = BTreeMap::new();
    let mut apply = BTreeMap::new();
    let keys: BTreeSet<&String> = local.keys().chain(remote.keys()).collect();
    for key in keys {
        let (l, r) = (local.get(key), remote.get(key));
        let take = |v: &Value, result: &mut BTreeMap<String, Value>, apply: &mut BTreeMap<String, Value>| {
            result.insert(key.clone(), v.clone());
            if l != Some(v) {
                apply.insert(key.clone(), v.clone());
            }
        };
        match (l, r) {
            (Some(l), None) => {
                result.insert(key.clone(), l.clone());
            }
            (None, Some(r)) => take(r, &mut result, &mut apply),
            (Some(l), Some(r)) if l == r => {
                result.insert(key.clone(), l.clone());
            }
            (Some(l), Some(r)) => {
                let was = inp.base.settings.get(key);
                let remote_wins = inp.strategy == Strategy::UseRemote
                    || match was {
                        Some(h) if *h == hash_value(l) => true,
                        Some(h) if *h == hash_value(r) => false,
                        Some(_) => false,
                        None => inp.defaults.get(key) == Some(l),
                    };
                if remote_wins {
                    take(r, &mut result, &mut apply);
                } else {
                    result.insert(key.clone(), l.clone());
                }
            }
            (None, None) => {}
        }
    }
    (result, apply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::items::tests::item;
    use crate::sync::items::{ItemScope, MissionItem};
    use serde_json::json;

    const OPTS: Options = Options { files: true, settings: true, missions: true };

    #[derive(Default)]
    struct World {
        base: Snapshot,
        local: Snapshot,
        remote: Snapshot,
        unreadable: HashSet<String>,
        rejected: Vec<String>,
        defaults: BTreeMap<String, Value>,
    }

    impl World {
        fn add(snap: &mut Snapshot, i: Item) {
            snap.memories.insert(i.uid.clone(), i);
        }

        /// The same memory on all three sides.
        fn synced(&mut self, i: Item) {
            Self::add(&mut self.base, i.clone());
            Self::add(&mut self.local, i.clone());
            Self::add(&mut self.remote, i);
        }

        fn run(&self, strategy: Strategy) -> Merged {
            let base = self.base.base();
            merge(&Inputs { base: &base, local: &self.local, remote: &self.remote, unreadable: &self.unreadable, rejected: &self.rejected, defaults: &self.defaults, options: OPTS, strategy })
        }

        fn merge(&self) -> Merged {
            self.run(Strategy::Merge)
        }

        /// What the merge `m` of this world removes that was synced before.
        fn gone(&self, m: &Merged) -> Removals {
            removals(&Inputs { base: &self.base.base(), local: &self.local, remote: &self.remote, unreadable: &self.unreadable, rejected: &[], defaults: &self.defaults, options: OPTS, strategy: Strategy::Merge }, m)
        }
    }

    fn texts(m: &Merged) -> Vec<String> {
        m.snapshot.memories.values().map(|i| i.text.clone()).collect()
    }

    fn with_text(i: &Item, text: &str, updated: i64) -> Item {
        Item { text: text.into(), updated, ..i.clone() }
    }

    #[test]
    fn what_only_one_side_added_is_added_to_the_other() {
        let mut w = World::default();
        World::add(&mut w.local, item("local000000000001", "preference", "Prefers pnpm"));
        World::add(&mut w.remote, item("remote00000000001", "fact", "The API runs on port 8080"));
        let m = w.merge();
        assert_eq!(m.snapshot.memories.len(), 2);
        assert_eq!(m.changes.len(), 1);
        assert!(matches!(&m.changes[0], Change::Add(i) if i.uid == "remote00000000001"));
        assert_eq!(m.report.pulled, 1);
        assert_eq!(m.report.pushed, 1);
    }

    #[test]
    fn a_change_on_one_side_wins_and_unchanged_things_stay_put() {
        let mut w = World::default();
        let a = item("aaaaaaaa00000001", "preference", "Prefers pnpm");
        let b = item("bbbbbbbb00000002", "preference", "Prefers dark themes");
        let c = item("cccccccc00000003", "preference", "Uses Helix");
        for i in [&a, &b, &c] {
            w.synced(i.clone());
        }
        World::add(&mut w.remote, with_text(&a, "Prefers yarn", 1_700_000_500));
        World::add(&mut w.local, with_text(&b, "Prefers light themes", 1_700_000_500));
        let m = w.merge();
        assert_eq!(m.snapshot.memories[&a.uid].text, "Prefers yarn");
        assert_eq!(m.snapshot.memories[&b.uid].text, "Prefers light themes");
        assert_eq!(m.snapshot.memories[&c.uid].text, "Uses Helix");
        // Only the remote edit has to be applied here; only the local edit has to be pushed.
        assert_eq!(m.changes, vec![Change::Update(with_text(&a, "Prefers yarn", 1_700_000_500))]);
        assert_eq!((m.report.pulled, m.report.pushed, m.report.conflicts), (1, 1, 0));
    }

    #[test]
    fn something_only_the_other_pc_deleted_is_removed_here_and_the_reverse() {
        let mut w = World::default();
        let a = item("aaaaaaaa00000001", "fact", "Staging is on port 9000");
        let b = item("bbbbbbbb00000002", "fact", "The build takes ten minutes");
        w.synced(a.clone());
        w.synced(b.clone());
        w.remote.memories.remove(&a.uid);
        w.local.memories.remove(&b.uid);
        let m = w.merge();
        assert!(m.snapshot.memories.is_empty());
        assert_eq!(m.changes, vec![Change::Remove { uid: a.uid.clone(), merged: false }]);
        assert_eq!((m.report.removed, m.report.pushed), (1, 1));
    }

    /// Eight memories synced everywhere.
    fn synced_world() -> World {
        let mut w = World::default();
        for n in 1..=8 {
            w.synced(item(&format!("aaaaaaaa0000000{n}"), "fact", &format!("Fact number {n} about the build")));
        }
        w
    }

    #[test]
    fn most_of_what_was_synced_going_at_once_is_noticed_from_either_side() {
        // The repository was deleted and made again: it holds nothing the last sync knew.
        let mut w = synced_world();
        w.remote.memories.clear();
        let gone = w.gone(&w.merge());
        assert!(gone.mass(), "{gone:?}");
        assert_eq!((gone.here.len(), gone.there.len(), gone.known), (8, 0, 8));

        // This PC's memory failed to load: it holds nothing, and the repository would lose it all.
        let mut w = synced_world();
        w.local.memories.clear();
        let gone = w.gone(&w.merge());
        assert!(gone.mass());
        assert_eq!((gone.here.len(), gone.there.len()), (0, 8));
    }

    #[test]
    fn ordinary_deletions_and_what_both_sides_already_lost_are_not_a_wipe() {
        // A few deleted on each side: that's housekeeping.
        let mut w = synced_world();
        for n in 1..=2 {
            w.local.memories.remove(&format!("aaaaaaaa0000000{n}"));
            w.remote.memories.remove(&format!("aaaaaaaa0000000{}", n + 2));
        }
        let gone = w.gone(&w.merge());
        assert_eq!((gone.here.len(), gone.there.len()), (2, 2));
        assert!(!gone.mass());

        // Gone from both sides already: nothing for this sync to remove or pass on.
        let mut w = synced_world();
        w.local.memories.clear();
        w.remote.memories.clear();
        let gone = w.gone(&w.merge());
        assert!(!gone.mass() && gone.uids().count() == 0);

        // A file that couldn't be read isn't removed, so it isn't counted.
        let mut w = synced_world();
        w.remote.memories.clear();
        w.unreadable = w.base.memories.keys().cloned().collect();
        assert_eq!(w.gone(&w.merge()).here.len(), 0);

        // The first sync has no history to compare with, so it never asks.
        let mut first = synced_world();
        first.base = Snapshot::default();
        first.remote.memories.clear();
        let gone = first.gone(&first.merge());
        assert!(!gone.mass() && gone.known == 0);
    }

    #[test]
    fn keeping_what_would_be_removed_brings_it_back_to_whichever_side_lost_it() {
        // The repository lost everything: forgetting those items from the base makes this PC's copies new, so they're saved again.
        let mut w = synced_world();
        w.remote.memories.clear();
        let mut base = w.base.base();
        for uid in w.local.memories.keys() {
            base.memories.remove(uid);
        }
        let kept = merge(&Inputs { base: &base, local: &w.local, remote: &w.remote, unreadable: &w.unreadable, rejected: &[], defaults: &w.defaults, options: OPTS, strategy: Strategy::Merge });
        assert_eq!(kept.snapshot.memories.len(), 8);
        assert!(kept.changes.is_empty());
        assert_eq!(kept.report.pushed, 8);

        // This PC lost everything: they come back here, even ones the user deleted (the rejected list stops them otherwise).
        let mut w = synced_world();
        w.local.memories.clear();
        let mut base = w.base.base();
        for uid in w.remote.memories.keys() {
            base.memories.remove(uid);
        }
        let kept = merge(&Inputs { base: &base, local: &w.local, remote: &w.remote, unreadable: &w.unreadable, rejected: &[], defaults: &w.defaults, options: OPTS, strategy: Strategy::Merge });
        assert_eq!(kept.snapshot.memories.len(), 8);
        assert_eq!(kept.changes.len(), 8);
        assert!(kept.changes.iter().all(|c| matches!(c, Change::Add(_))));
    }

    #[test]
    fn both_changed_the_same_memory_the_users_own_words_then_the_newer_win() {
        let mut w = World::default();
        let a = item("aaaaaaaa00000001", "preference", "Prefers pnpm");
        w.synced(a.clone());
        // Learned on both: the newer wins, and the older words are kept in its history.
        World::add(&mut w.local, with_text(&a, "Prefers pnpm for installs", 1_700_000_300));
        World::add(&mut w.remote, with_text(&a, "Prefers pnpm over npm", 1_700_000_900));
        let m = w.merge();
        let won = &m.snapshot.memories[&a.uid];
        assert_eq!(won.text, "Prefers pnpm over npm");
        assert!(won.history.iter().any(|r| r.text == "Prefers pnpm for installs" && r.why == "merged"));
        assert_eq!(m.report.conflicts, 1);

        // The user's own words beat a newer learned change.
        let mut w = World::default();
        w.synced(a.clone());
        World::add(&mut w.local, Item { origin: "user".into(), ..with_text(&a, "Prefers pnpm, always", 1_700_000_300) });
        World::add(&mut w.remote, with_text(&a, "Prefers yarn", 1_700_000_900));
        let m = w.merge();
        assert_eq!(m.snapshot.memories[&a.uid].text, "Prefers pnpm, always");
        assert_eq!(m.snapshot.memories[&a.uid].origin, "user");
    }

    #[test]
    fn deleting_beats_an_untouched_copy_but_never_the_users_own_words() {
        let mut w = World::default();
        let learned = item("aaaaaaaa00000001", "fact", "Deploys go out on Fridays");
        let written = Item { origin: "user".into(), ..item("bbbbbbbb00000002", "fact", "Staging password rotates monthly") };
        w.synced(learned.clone());
        w.synced(written.clone());
        // Deleted here; the other PC changed both since.
        w.local.memories.clear();
        World::add(&mut w.remote, with_text(&learned, "Deploys go out on Thursdays", 1_700_000_900));
        World::add(&mut w.remote, with_text(&written, "Staging credentials rotate monthly", 1_700_000_900));
        let m = w.merge();
        // The learned one stays deleted; the user's own words come back (as changed elsewhere).
        assert!(!m.snapshot.memories.contains_key(&learned.uid));
        assert_eq!(m.snapshot.memories[&written.uid].text, "Staging credentials rotate monthly");
        assert!(m.changes.iter().any(|c| matches!(c, Change::Add(i) if i.uid == written.uid)));
        assert_eq!(m.report.conflicts, 2);

        // The same the other way round: edited here, deleted there.
        let mut w = World::default();
        w.synced(learned.clone());
        w.synced(written.clone());
        w.remote.memories.clear();
        World::add(&mut w.local, with_text(&learned, "Deploys go out on Thursdays", 1_700_000_900));
        World::add(&mut w.local, with_text(&written, "Staging credentials rotate monthly", 1_700_000_900));
        let m = w.merge();
        assert!(m.snapshot.memories.contains_key(&written.uid) && !m.snapshot.memories.contains_key(&learned.uid));
        assert_eq!(m.changes, vec![Change::Remove { uid: learned.uid.clone(), merged: false }]);
    }

    #[test]
    fn notes_are_never_dropped() {
        let mut w = World::default();
        let note = item("aaaaaaaa00000001", "note", "Sent Charles the dark logo");
        w.synced(note.clone());
        w.remote.memories.clear();
        World::add(&mut w.local, with_text(&note, "Sent Charles the dark logo, v2", 1_700_000_900));
        let m = w.merge();
        assert!(m.snapshot.memories.contains_key(&note.uid));
        assert!(m.changes.is_empty());
    }

    #[test]
    fn a_repository_file_that_cannot_be_read_is_not_a_deletion() {
        let mut w = World::default();
        let a = item("aaaaaaaa00000001", "fact", "Staging is on port 9000");
        w.synced(a.clone());
        w.remote.memories.remove(&a.uid);
        w.unreadable.insert(a.uid.clone());
        let m = w.merge();
        assert!(m.snapshot.memories.contains_key(&a.uid));
        assert!(m.changes.is_empty());
    }

    #[test]
    fn the_same_thing_said_on_both_pcs_becomes_one_memory() {
        let mut w = World::default();
        World::add(&mut w.local, item("local000000000001", "preference", "Prefers pnpm over npm"));
        World::add(&mut w.remote, item("remote00000000001", "preference", "prefers pnpm over npm."));
        let m = w.merge();
        assert_eq!(m.snapshot.memories.len(), 1);
        assert_eq!(m.report.combined, 1);
        // The repository's copy survives; this PC's goes.
        assert!(m.snapshot.memories.contains_key("remote00000000001"));
        assert!(m.changes.contains(&Change::Remove { uid: "local000000000001".into(), merged: true }));
        assert!(m.candidates.is_empty());
    }

    #[test]
    fn near_duplicates_combine_but_different_scopes_and_notes_do_not() {
        let mut w = World::default();
        World::add(&mut w.local, item("local000000000001", "preference", "Prefers pnpm over npm for installs"));
        World::add(&mut w.remote, item("remote00000000001", "preference", "Prefers pnpm over npm for installs here"));
        // The same words in another project are another memory.
        World::add(&mut w.local, Item { scope: ItemScope::Project { name: "app".into(), remote: Some("github.com/me/app".into()), key: None }, ..item("local000000000002", "fact", "Builds with just") });
        World::add(&mut w.remote, item("remote00000000002", "fact", "Builds with just"));
        // Notes that say something different are two notes; ones that differ only in case,
        // spacing or closing punctuation are one.
        World::add(&mut w.local, item("local000000000003", "note", "Sent Charles the dark logo"));
        World::add(&mut w.remote, item("remote00000000003", "note", "Sent Charles the dark logo and the icon"));
        World::add(&mut w.local, item("local000000000004", "note", "Call the bank on Monday"));
        World::add(&mut w.remote, item("remote00000000004", "note", "call the bank on  monday."));
        let m = w.merge();
        let mut left = texts(&m);
        left.sort();
        assert_eq!(left.len(), 6, "{left:?}");
        assert_eq!(m.report.combined, 2);
        assert!(left.contains(&"Sent Charles the dark logo".to_owned()) && left.contains(&"Sent Charles the dark logo and the icon".to_owned()));
        assert_eq!(left.iter().filter(|t| t.as_str() == "Builds with just").count(), 2);
    }

    #[test]
    fn combining_keeps_what_both_copies_add() {
        let mut w = World::default();
        let mut l = item("local000000000001", "preference", "Prefers pnpm over npm");
        l.pinned = true;
        l.votes = 3;
        l.created = 1_600_000_000;
        let mut r = item("remote00000000001", "preference", "Prefers pnpm over npm");
        r.updated = 1_700_000_900;
        World::add(&mut w.local, l);
        World::add(&mut w.remote, r);
        let m = w.merge();
        let one = m.snapshot.memories.values().next().unwrap();
        // The user's pin decides which survives (it's theirs), and nothing it had is lost.
        assert!(one.pinned);
        assert_eq!(one.votes, 3);
        assert_eq!(one.created, 1_600_000_000);
        assert_eq!(one.updated, 1_700_000_900);
    }

    #[test]
    fn what_the_user_deleted_does_not_come_back_under_another_id() {
        let mut w = World::default();
        World::add(&mut w.remote, item("remote00000000001", "preference", "Prefers dark themes"));
        World::add(&mut w.remote, Item { origin: "user".into(), ..item("remote00000000002", "preference", "Uses Helix as my editor") });
        w.rejected = vec!["prefers dark themes".into(), "uses helix as my editor".into()];
        let m = w.merge();
        // A learned copy is left out; something written by the user is theirs to keep.
        assert_eq!(m.snapshot.memories.len(), 1);
        assert!(m.snapshot.memories.contains_key("remote00000000002"));
        assert!(m.report.notes[0].contains("left out"));
    }

    #[test]
    fn the_ai_is_asked_only_when_both_pcs_learned_something_in_the_same_place() {
        let mut w = World::default();
        World::add(&mut w.local, item("local000000000001", "preference", "Prefers tabs"));
        World::add(&mut w.remote, item("remote00000000001", "preference", "Prefers spaces"));
        World::add(&mut w.local, item("local000000000002", "note", "A note stays out of it"));
        World::add(&mut w.remote, item("remote00000000002", "note", "Another note"));
        // Only one PC learned something about this project.
        World::add(&mut w.local, Item { scope: ItemScope::Project { name: "app".into(), remote: Some("github.com/me/app".into()), key: None }, ..item("local000000000003", "fact", "Builds with just") });
        let m = w.merge();
        let cand = |uid: &str, elsewhere: bool| Candidate { uid: uid.to_owned(), elsewhere };
        assert_eq!(m.candidates, vec![cand("local000000000001", false), cand("remote00000000001", true)]);
        // After a sync both sides know everything: nothing is new any more.
        let mut after = World::default();
        for i in m.snapshot.memories.values() {
            after.synced(i.clone());
        }
        assert!(after.merge().candidates.is_empty());
    }

    #[test]
    fn using_the_repositorys_copy_makes_this_pc_match_it() {
        let mut w = World::default();
        World::add(&mut w.local, item("local000000000001", "preference", "Only on this PC"));
        World::add(&mut w.local, item("both0000000000001", "fact", "Differs"));
        World::add(&mut w.remote, item("both0000000000001", "fact", "Differs in the repository"));
        World::add(&mut w.remote, item("remote00000000001", "fact", "Only in the repository"));
        let m = w.run(Strategy::UseRemote);
        assert_eq!(m.snapshot.memories.len(), 2);
        assert_eq!(m.report.pushed, 0);
        assert!(m.changes.contains(&Change::Remove { uid: "local000000000001".into(), merged: false }));
        assert!(m.changes.iter().any(|c| matches!(c, Change::Update(i) if i.uid == "both0000000000001")));
        assert!(m.changes.iter().any(|c| matches!(c, Change::Add(i) if i.uid == "remote00000000001")));
    }

    #[test]
    fn a_first_sync_with_the_same_ids_on_both_sides_settles_each_by_rules() {
        // Disconnected and reconnected: same uids, no base.
        let mut w = World::default();
        let a = item("aaaaaaaa00000001", "fact", "Staging is on 9000");
        World::add(&mut w.local, a.clone());
        World::add(&mut w.remote, with_text(&a, "Staging is on 9100", 1_700_000_900));
        let m = w.merge();
        assert_eq!(m.snapshot.memories[&a.uid].text, "Staging is on 9100");
        assert_eq!(m.report.conflicts, 1);
    }

    fn mission(uid: &str, title: &str) -> MissionItem {
        MissionItem { uid: uid.into(), title: title.into(), notes: String::new(), status: "idea".into(), created: 1_700_000_000, from_agent: false }
    }

    #[test]
    fn missions_merge_by_id_and_the_same_idea_on_both_pcs_is_one() {
        let mut w = World::default();
        let kept = mission("aaaaaaaa00000001", "Draft release notes");
        w.base.missions.insert(kept.uid.clone(), kept.clone());
        w.local.missions.insert(kept.uid.clone(), kept.clone());
        w.remote.missions.insert(kept.uid.clone(), MissionItem { status: "done".into(), ..kept.clone() });
        w.local.missions.insert("local000000000001".into(), mission("local000000000001", "Fix the login bug"));
        w.remote.missions.insert("remote00000000001".into(), mission("remote00000000001", "fix the login bug"));
        w.remote.missions.insert("remote00000000002".into(), mission("remote00000000002", "Try the new model"));
        let m = w.merge();
        assert_eq!(m.snapshot.missions.len(), 3);
        assert_eq!(m.snapshot.missions[&kept.uid].status, "done");
        assert_eq!(m.report.combined, 1);
        assert!(m.mission_changes.contains(&MissionChange::Update(MissionItem { status: "done".into(), ..kept })));
        assert!(m.mission_changes.contains(&MissionChange::Remove("local000000000001".into())));
        assert!(m.mission_changes.iter().any(|c| matches!(c, MissionChange::Add(x) if x.uid == "remote00000000002")));
    }

    #[test]
    fn preferences_merge_key_by_key() {
        let mut w = World::default();
        w.defaults = BTreeMap::from([("theme".to_owned(), json!("graphite")), ("layout".to_owned(), json!("grid")), ("show_reasoning".to_owned(), json!(true))]);
        // A fresh install (all defaults) receives what the repository has.
        w.local.settings = w.defaults.clone();
        w.remote.settings = BTreeMap::from([("theme".to_owned(), json!("slate")), ("layout".to_owned(), json!("grid"))]);
        let m = w.merge();
        assert_eq!(m.snapshot.settings["theme"], json!("slate"));
        assert_eq!(m.setting_changes, BTreeMap::from([("theme".to_owned(), json!("slate"))]));
        // The repository has no opinion on show_reasoning: this PC's value goes in.
        assert_eq!(m.snapshot.settings["show_reasoning"], json!(true));

        // A PC that chose a theme keeps it on a first sync.
        w.local.settings.insert("theme".into(), json!("paper"));
        let m = w.merge();
        assert_eq!(m.snapshot.settings["theme"], json!("paper"));
        assert!(m.setting_changes.is_empty());

        // After that, a change on one side travels, and a change on both keeps this PC's.
        w.base.settings = BTreeMap::from([("theme".to_owned(), json!("paper")), ("layout".to_owned(), json!("grid"))]);
        w.local.settings.insert("layout".into(), json!("rows"));
        w.remote.settings.insert("theme".into(), json!("carbon"));
        let m = w.merge();
        assert_eq!(m.snapshot.settings["theme"], json!("carbon"));
        assert_eq!(m.snapshot.settings["layout"], json!("rows"));
        w.local.settings.insert("theme".into(), json!("slate"));
        w.remote.settings.insert("layout".into(), json!("columns"));
        let m = w.merge();
        assert_eq!((m.snapshot.settings["theme"].clone(), m.snapshot.settings["layout"].clone()), (json!("slate"), json!("rows")));
    }

    #[test]
    fn what_is_not_synced_is_left_out_of_the_merge() {
        let mut w = World::default();
        w.local.missions.insert("local000000000001".into(), mission("local000000000001", "Only here"));
        w.remote.missions.insert("remote00000000001".into(), mission("remote00000000001", "Only there"));
        w.local.settings.insert("theme".into(), json!("paper"));
        w.remote.settings.insert("theme".into(), json!("slate"));
        let base = w.base.base();
        let m = merge(&Inputs { base: &base, local: &w.local, remote: &w.remote, unreadable: &w.unreadable, rejected: &[], defaults: &w.defaults, options: Options { files: true, settings: false, missions: false }, strategy: Strategy::Merge });
        assert!(m.mission_changes.is_empty() && m.setting_changes.is_empty());
        assert_eq!(m.report.pushed, 0);
    }
}
