//! Sidebar folders: a tree of named folders hosts can live in, persisted to
//! `<config>/folders.json` together with the open/closed state of each one.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile::{Profile, write_json_atomic};

pub const MAX_NAME: usize = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub parent: Option<Uuid>,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FolderFile {
    #[serde(default)]
    folders: Vec<Folder>,
    #[serde(default)]
    archive_open: bool,
}

#[derive(Debug, Default)]
pub struct FolderStore {
    path: PathBuf,
    pub folders: Vec<Folder>,
    /// Whether the "Archived" section of the sidebar is expanded.
    pub archive_open: bool,
}

impl FolderStore {
    pub fn load(path: PathBuf) -> Result<Self> {
        let file: FolderFile = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("folders file is corrupt: {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FolderFile::default(),
            Err(e) => return Err(e).context("reading folders"),
        };
        let mut store = Self { path, folders: file.folders, archive_open: file.archive_open };
        store.repair();
        Ok(store)
    }

    pub fn save(&self) -> Result<()> {
        let file = FolderFile { folders: self.folders.clone(), archive_open: self.archive_open };
        write_json_atomic(&self.path, &file)
    }

    pub fn get(&self, id: Uuid) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    fn get_mut(&mut self, id: Uuid) -> Option<&mut Folder> {
        self.folders.iter_mut().find(|f| f.id == id)
    }

    /// Sub-folders of `parent` (top level for `None`), sorted by name.
    pub fn children(&self, parent: Option<Uuid>) -> Vec<&Folder> {
        let mut out: Vec<&Folder> = self.folders.iter().filter(|f| f.parent == parent).collect();
        out.sort_by_key(|f| f.name.to_lowercase());
        out
    }

    pub fn create(&mut self, name: &str, parent: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        let parent = parent.filter(|p| self.get(*p).is_some());
        self.folders.push(Folder { id, name: clean_name(name), parent, collapsed: false });
        id
    }

    pub fn rename(&mut self, id: Uuid, name: &str) {
        if let Some(f) = self.get_mut(id) {
            f.name = clean_name(name);
        }
    }

    pub fn set_collapsed(&mut self, id: Uuid, collapsed: bool) {
        if let Some(f) = self.get_mut(id) {
            f.collapsed = collapsed;
        }
    }

    pub fn set_all_collapsed(&mut self, collapsed: bool) {
        for f in &mut self.folders {
            f.collapsed = collapsed;
        }
    }

    /// Moves `id` under `parent`. Refused (false) when that would put a folder inside itself.
    pub fn move_to(&mut self, id: Uuid, parent: Option<Uuid>) -> bool {
        if let Some(p) = parent
            && (p == id || self.descendants(id).contains(&p) || self.get(p).is_none())
        {
            return false;
        }
        match self.get_mut(id) {
            Some(f) => {
                f.parent = parent;
                true
            }
            None => false,
        }
    }

    /// Deletes the folder; its sub-folders move up to its parent, which is returned so
    /// the caller can move the folder's hosts there too.
    pub fn remove(&mut self, id: Uuid) -> Option<Uuid> {
        let parent = self.get(id)?.parent;
        for f in &mut self.folders {
            if f.parent == Some(id) {
                f.parent = parent;
            }
        }
        self.folders.retain(|f| f.id != id);
        parent
    }

    /// Every folder below `id` (not `id` itself).
    pub fn descendants(&self, id: Uuid) -> HashSet<Uuid> {
        let mut out = HashSet::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            for f in self.folders.iter().filter(|f| f.parent == Some(cur)) {
                if out.insert(f.id) {
                    stack.push(f.id);
                }
            }
        }
        out
    }

    /// `Production / API`.
    pub fn path_of(&self, id: Uuid) -> String {
        let mut names = Vec::new();
        let mut cur = Some(id);
        let mut guard = 0;
        while let Some(c) = cur
            && guard < 64
        {
            let Some(f) = self.get(c) else { break };
            names.push(f.name.as_str());
            cur = f.parent;
            guard += 1;
        }
        names.reverse();
        names.join(" / ")
    }

    /// Every folder as (id, depth) in tree order, for pickers.
    pub fn flattened(&self) -> Vec<(Uuid, usize)> {
        fn walk(store: &FolderStore, parent: Option<Uuid>, depth: usize, out: &mut Vec<(Uuid, usize)>) {
            for f in store.children(parent) {
                out.push((f.id, depth));
                if depth < 32 {
                    walk(store, Some(f.id), depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self, None, 0, &mut out);
        out
    }

    /// The top-level folder called `name` (case-insensitive), created if missing.
    pub fn ensure_root(&mut self, name: &str) -> Uuid {
        let wanted = clean_name(name);
        match self.folders.iter().find(|f| f.parent.is_none() && f.name.eq_ignore_ascii_case(&wanted)) {
            Some(f) => f.id,
            None => self.create(&wanted, None),
        }
    }

    /// Moves legacy `group` names into folders. Returns whether any profile changed.
    pub fn migrate_groups(&mut self, profiles: &mut [Profile]) -> bool {
        let mut changed = false;
        for p in profiles.iter_mut() {
            if p.group.trim().is_empty() {
                continue;
            }
            if p.folder.is_none() {
                p.folder = Some(self.ensure_root(&p.group));
            }
            p.group.clear();
            changed = true;
        }
        changed
    }

    /// Drops dangling parents and breaks cycles (hand-edited files).
    fn repair(&mut self) {
        let ids: HashSet<Uuid> = self.folders.iter().map(|f| f.id).collect();
        for f in &mut self.folders {
            if f.parent.is_some_and(|p| !ids.contains(&p) || p == f.id) {
                f.parent = None;
            }
        }
        for i in 0..self.folders.len() {
            let id = self.folders[i].id;
            if let Some(p) = self.folders[i].parent
                && self.descendants(id).contains(&p)
            {
                self.folders[i].parent = None;
            }
        }
    }
}

fn clean_name(raw: &str) -> String {
    let name: String = raw.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let name = name.trim();
    if name.is_empty() { "New folder".into() } else { name.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, FolderStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = FolderStore::load(dir.path().join("folders.json")).unwrap();
        (dir, s)
    }

    #[test]
    fn create_nest_and_path() {
        let (_d, mut s) = store();
        let prod = s.create("Production", None);
        let api = s.create(" API ", Some(prod));
        assert_eq!(s.path_of(api), "Production / API");
        assert_eq!(s.children(None).len(), 1);
        assert_eq!(s.children(Some(prod))[0].id, api);
        assert_eq!(s.flattened(), vec![(prod, 0), (api, 1)]);
        assert_eq!(s.create("x", Some(Uuid::new_v4())), s.children(None)[1].id, "unknown parent -> top level");
    }

    #[test]
    fn move_refuses_cycles() {
        let (_d, mut s) = store();
        let a = s.create("a", None);
        let b = s.create("b", Some(a));
        let c = s.create("c", Some(b));
        assert!(!s.move_to(a, Some(c)), "a inside its own grandchild");
        assert!(!s.move_to(a, Some(a)));
        assert!(s.move_to(c, None));
        assert!(s.move_to(a, Some(c)));
        assert_eq!(s.path_of(b), "c / a / b");
    }

    #[test]
    fn remove_lifts_children() {
        let (_d, mut s) = store();
        let a = s.create("a", None);
        let b = s.create("b", Some(a));
        let c = s.create("c", Some(b));
        assert_eq!(s.remove(b), Some(a));
        assert_eq!(s.get(c).unwrap().parent, Some(a));
        assert_eq!(s.remove(Uuid::new_v4()), None);
    }

    #[test]
    fn persists_collapse_state_and_archive() {
        let (d, mut s) = store();
        let a = s.create("a", None);
        s.set_collapsed(a, true);
        s.archive_open = true;
        s.save().unwrap();
        let back = FolderStore::load(d.path().join("folders.json")).unwrap();
        assert!(back.get(a).unwrap().collapsed);
        assert!(back.archive_open);
    }

    #[test]
    fn migrates_legacy_groups_once() {
        let (_d, mut s) = store();
        let mut a = Profile::new();
        a.group = "Prod".into();
        let mut b = Profile::new();
        b.group = "prod".into();
        let c = Profile::new();
        let mut profiles = vec![a, b, c];
        assert!(s.migrate_groups(&mut profiles));
        assert_eq!(s.folders.len(), 1, "case-insensitive merge");
        assert_eq!(profiles[0].folder, profiles[1].folder);
        assert!(profiles[2].folder.is_none());
        assert!(profiles.iter().all(|p| p.group.is_empty()));
        assert!(!s.migrate_groups(&mut profiles));
    }

    #[test]
    fn repair_fixes_dangling_and_cyclic_parents() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let json = format!(
            r#"{{"folders":[{{"id":"{a}","name":"a","parent":"{b}"}},{{"id":"{b}","name":"b","parent":"{a}"}},{{"id":"{}","name":"c","parent":"{}"}}]}}"#,
            Uuid::new_v4(),
            Uuid::new_v4()
        );
        std::fs::write(d.path().join("f.json"), json).unwrap();
        let s = FolderStore::load(d.path().join("f.json")).unwrap();
        assert!(s.folders.iter().filter(|f| f.parent.is_none()).count() >= 2);
        assert!(!s.flattened().is_empty());
    }
}
