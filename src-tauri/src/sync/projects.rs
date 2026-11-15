//! Which project a memory belongs to, in terms every PC understands. Memory scopes are keyed by
//! the folder's path, which differs from PC to PC, so a project is also known by the git remote
//! of its folder (`github.com/owner/repo`): the same repository cloned anywhere shares its memories.

use std::collections::HashMap;
use std::path::Path;

use super::items::ItemScope;
use crate::memory::{self, MemoryStore, Scope};

/// Maps memory scopes to portable ones and back, for the folders this PC knows about.
pub struct Projects {
    /// The sessions folder's scope key: every session folder shares it.
    sessions_key: String,
    /// Each known folder's remote.
    remote_of: HashMap<String, Option<String>>,
    /// The folder to use for a remote (where this PC has a clone).
    key_of: HashMap<String, String>,
}

impl Projects {
    /// `keys` are the project keys of the folders this PC knows (from its memories, recent folders
    /// and open sessions). Their remotes are read from their `.git/config`.
    pub fn new(sessions_root: &Path, keys: impl IntoIterator<Item = String>) -> Projects {
        let sessions_key = memory::project_key(sessions_root);
        let mut sorted: Vec<String> = keys.into_iter().filter(|k| *k != sessions_key && !k.starts_with("git:")).collect();
        sorted.sort();
        sorted.dedup();
        let mut remote_of = HashMap::new();
        let mut key_of: HashMap<String, String> = HashMap::new();
        for key in sorted {
            let remote = git_remote(Path::new(&key));
            if let Some(r) = &remote {
                key_of.entry(r.clone()).or_insert_with(|| key.clone());
            }
            remote_of.insert(key, remote);
        }
        Projects { sessions_key, remote_of, key_of }
    }

    /// A memory's scope as other PCs read it.
    pub fn to_item(&self, scope: &Scope) -> ItemScope {
        let Scope::Project(key) = scope else { return ItemScope::Global };
        if *key == self.sessions_key {
            return ItemScope::Sessions;
        }
        if let Some(remote) = key.strip_prefix("git:") {
            return ItemScope::Project { name: last_part(remote), remote: Some(remote.to_owned()), key: None };
        }
        let remote = self.remote_of.get(key).cloned().flatten().or_else(|| git_remote(Path::new(key)));
        // A remote identifies the project on any PC; the path is only kept when it's all there is.
        let path = remote.is_none().then(|| key.clone());
        ItemScope::Project { name: last_part(key), remote, key: path }
    }

    /// The scope on this PC for one that came from another. A project this PC has no clone of
    /// waits under a `git:` key, and is claimed by the folder once there is one.
    pub fn to_local(&self, scope: &ItemScope) -> Scope {
        match scope {
            ItemScope::Global => Scope::Global,
            ItemScope::Sessions => Scope::Project(self.sessions_key.clone()),
            ItemScope::Project { remote: Some(r), .. } => Scope::Project(self.key_of.get(r).cloned().unwrap_or_else(|| format!("git:{r}"))),
            ItemScope::Project { key: Some(k), .. } => Scope::Project(k.clone()),
            ItemScope::Project { name, .. } => Scope::Project(format!("name:{name}")),
        }
    }

    /// Projects waiting under a `git:` key that this PC now has a folder for: `(remote, key)`.
    pub fn claims(&self, store: &MemoryStore) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for m in &store.memories {
            let Scope::Project(k) = &m.scope else { continue };
            let Some(remote) = k.strip_prefix("git:") else { continue };
            if let Some(key) = self.key_of.get(remote)
                && !out.iter().any(|(r, _)| r == remote)
            {
                out.push((remote.to_owned(), key.clone()));
            }
        }
        out
    }
}

/// The last part of a path or remote: the folder's or repository's own name.
fn last_part(s: &str) -> String {
    s.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(s).to_owned()
}

/// `github.com/owner/repo` for a git remote URL in any of its spellings (https, ssh, scp-style),
/// lower-cased, without credentials, port or `.git`. `None` for anything that isn't a hosted
/// repository (a local path, say), which can't identify a project on another PC.
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = ["https://", "http://", "ssh://", "git://", "git+ssh://"].iter().find_map(|p| url.strip_prefix(p));
    let (host, path) = match rest {
        Some(rest) => {
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            (host.split(':').next()?, path)
        }
        None => {
            // scp-style: git@github.com:owner/repo.git
            let (before, path) = url.split_once(':')?;
            if path.starts_with('/') && !before.contains('@') || url.chars().nth(1) == Some(':') {
                return None;
            }
            (before.rsplit_once('@').map_or(before, |(_, h)| h), path)
        }
    };
    let path = path.trim_matches('/').trim_end_matches(".git").trim_end_matches('/');
    if !host.contains('.') || path.split('/').filter(|p| !p.is_empty()).count() < 2 || host.contains(char::is_whitespace) || path.contains(char::is_whitespace) {
        return None;
    }
    Some(format!("{}/{}", host.to_lowercase(), path.to_lowercase()))
}

/// The remote of the git repository at `dir` (its `origin`), read from `.git/config`. Nothing is
/// read from a path that leads off this PC (a network share): the key of a folder can come from
/// another PC's data, and Windows signs in to a server just to look at a path on it.
pub fn git_remote(dir: &Path) -> Option<String> {
    if !super::is_local_path(&dir.to_string_lossy()) {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(".git").join("config")).ok()?;
    origin_url(&text).and_then(|url| normalize_remote(&url))
}

fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            let section: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            in_origin = section.eq_ignore_ascii_case("[remote\"origin\"]");
        } else if in_origin && let Some((key, value)) = line.split_once('=') && key.trim().eq_ignore_ascii_case("url") {
            return Some(value.trim().to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Kind, Origin, State};

    #[test]
    fn remotes_are_the_same_in_every_spelling() {
        let want = Some("github.com/owner/repo".to_owned());
        for url in [
            "https://github.com/Owner/Repo.git",
            "https://github.com/owner/repo",
            "https://user:token@github.com/owner/repo.git/",
            "http://github.com/Owner/Repo",
            "git@github.com:Owner/Repo.git",
            "ssh://git@github.com/owner/repo.git",
            "ssh://git@github.com:22/owner/repo.git",
            "git://github.com/owner/repo.git",
        ] {
            assert_eq!(normalize_remote(url), want, "{url}");
        }
        assert_eq!(normalize_remote("https://dev.azure.com/org/proj/_git/repo").as_deref(), Some("dev.azure.com/org/proj/_git/repo"));
        for not in ["C:\\repos\\app", "/srv/git/app.git", "../app", "file:///srv/app", "", "https://github.com/owner", "origin"] {
            assert_eq!(normalize_remote(not), None, "{not}");
        }
    }

    #[test]
    fn the_origin_remote_is_read_from_git_config() {
        let config = "[core]\n\trepositoryformatversion = 0\n[remote \"upstream\"]\n\turl = https://github.com/other/fork.git\n[remote \"origin\"]\n\turl = git@github.com:Me/App.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n";
        assert_eq!(origin_url(config).as_deref(), Some("git@github.com:Me/App.git"));
        assert_eq!(origin_url("[core]\n\tbare = false\n"), None);
    }

    fn clone_with_remote(root: &Path, name: &str, remote: &str) -> String {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("config"), format!("[remote \"origin\"]\n\turl = {remote}\n")).unwrap();
        memory::project_key(&dir)
    }

    #[test]
    fn a_project_follows_its_remote_between_pcs() {
        let root = std::env::temp_dir().join(format!("gs-proj-{}", memory::new_uid()));
        let mine = clone_with_remote(&root, "work\\app", "https://github.com/me/app.git");
        let sessions = root.join("sessions");
        let p = Projects::new(&sessions, vec![mine.clone(), memory::project_key(&sessions)]);

        // Out: this PC's key becomes the remote (the path stays here: it's no use to another PC).
        let out = p.to_item(&Scope::Project(mine.clone()));
        assert_eq!(out, ItemScope::Project { name: "app".into(), remote: Some("github.com/me/app".into()), key: None });
        assert_eq!(p.to_item(&Scope::Global), ItemScope::Global);
        assert_eq!(p.to_item(&Scope::Project(memory::project_key(&sessions))), ItemScope::Sessions);

        // In, on a PC that keeps the clone elsewhere: found by remote, not by path.
        let other_root = std::env::temp_dir().join(format!("gs-proj-{}", memory::new_uid()));
        let theirs = clone_with_remote(&other_root, "dev\\app", "git@github.com:Me/App.git");
        let there = Projects::new(&other_root.join("s"), vec![theirs.clone()]);
        assert_eq!(there.to_local(&out), Scope::Project(theirs));
        assert_eq!(there.to_local(&ItemScope::Sessions), Scope::Project(memory::project_key(&other_root.join("s"))));
        assert_eq!(there.to_local(&ItemScope::Global), Scope::Global);

        // A project this PC has no clone of waits under a `git:` key, and is claimed later.
        let nowhere = Projects::new(&other_root.join("s"), Vec::new());
        let waiting = nowhere.to_local(&out);
        assert_eq!(waiting, Scope::Project("git:github.com/me/app".into()));
        assert_eq!(nowhere.to_item(&waiting), ItemScope::Project { name: "app".into(), remote: Some("github.com/me/app".into()), key: None });

        // Without a remote only the same path matches.
        let plain = ItemScope::Project { name: "notes".into(), remote: None, key: Some("d:\\notes".into()) };
        assert_eq!(there.to_local(&plain), Scope::Project("d:\\notes".into()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other_root);
    }

    #[test]
    fn a_folder_that_leads_off_the_pc_is_never_read() {
        let root = std::env::temp_dir().join(format!("gs-local-{}", memory::new_uid()));
        let key = clone_with_remote(&root, "app", "https://github.com/me/app.git");
        assert_eq!(git_remote(Path::new(&key)).as_deref(), Some("github.com/me/app"));
        // The same folder spelled as a device path (`\\?\C:\…`) is refused before anything is opened: the
        // check is on how the path begins, so a network share can't be reached by a key that came from a repository.
        let device = format!("\\\\?\\{}", root.join("app").display());
        assert!(root.join("app").join(".git").join("config").is_file());
        assert_eq!(git_remote(Path::new(&device)), None);
        // Such a key is still only a name for the project; it just finds no remote.
        let p = Projects::new(&root.join("sessions"), vec![device.clone()]);
        assert_eq!(p.to_item(&Scope::Project(device.clone())), ItemScope::Project { name: "app".into(), remote: None, key: Some(device) });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memories_waiting_for_a_project_are_claimed_when_its_folder_is_known() {
        let root = std::env::temp_dir().join(format!("gs-claim-{}", memory::new_uid()));
        let key = clone_with_remote(&root, "app", "https://github.com/me/app");
        let mut store = MemoryStore::load_from(root.join("memory.json"));
        let id = store.insert(Kind::Fact, Scope::Project("git:github.com/me/app".into()), "The app builds with just", Origin::Learned, State::Active, 0.9, None).unwrap();
        store.insert(Kind::Fact, Scope::Project("git:github.com/me/other".into()), "Another repo's fact", Origin::Learned, State::Active, 0.9, None);
        let p = Projects::new(&root.join("sessions"), vec![key.clone()]);
        let claims = p.claims(&store);
        assert_eq!(claims, vec![("github.com/me/app".to_owned(), key.clone())]);
        assert_eq!(store.adopt_project("github.com/me/app", &key), 1);
        assert_eq!(store.get(id).unwrap().scope, Scope::Project(key));
        let _ = std::fs::remove_dir_all(&root);
    }
}
