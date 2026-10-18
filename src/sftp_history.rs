//! Per-host remote path history (Bitvise-style): the directories most recently visited
//! over SFTP, newest first, persisted in `sftp_history.json` next to the other config.
//! Also every directory a host is known to have (visited, or seen in a listing), with visit
//! counts, persisted in `sftp_known_dirs.json` so path suggestions survive restarts.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile::write_json_atomic;

/// How many directories are remembered per host.
pub const HISTORY_CAP: usize = 20;
const FILE_NAME: &str = "sftp_history.json";
/// How many known directories are remembered per host.
pub const KNOWN_CAP: usize = 2000;
const KNOWN_FILE_NAME: &str = "sftp_known_dirs.json";

/// Serialises read-modify-write cycles so two SFTP panes never clobber each other.
static FILE_LOCK: Mutex<()> = Mutex::new(());

pub type HistoryMap = BTreeMap<Uuid, Vec<String>>;

/// Default location: `<config_dir>/sftp_history.json`.
pub fn default_path() -> PathBuf {
    crate::app::config_dir().join(FILE_NAME)
}

/// Moves `path` to the front of `list`, removing duplicates and trimming to [`HISTORY_CAP`].
/// Returns `false` when nothing changed (already the newest entry, or a blank path).
pub fn push_front(list: &mut Vec<String>, path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() || list.first().map(String::as_str) == Some(path) {
        return false;
    }
    list.retain(|p| p != path);
    list.insert(0, path.to_string());
    list.truncate(HISTORY_CAP);
    true
}

pub fn load_map(path: &Path) -> HistoryMap {
    crate::json_file::load_or_default(path)
}

/// The saved history of one host (newest first).
pub fn load(path: &Path, host: Uuid) -> Vec<String> {
    let _guard = FILE_LOCK.lock();
    load_map(path).remove(&host).unwrap_or_default()
}

/// Records a visit and returns the host's updated list. Re-reads the file first so
/// concurrent panes (or other hosts) are preserved.
pub fn record(path: &Path, host: Uuid, dir: &str) -> Result<Vec<String>> {
    let _guard = FILE_LOCK.lock();
    let mut map = load_map(path);
    let list = map.entry(host).or_default();
    if push_front(list, dir) {
        let out = list.clone();
        write_json_atomic(path, &map)?;
        return Ok(out);
    }
    Ok(list.clone())
}

/// A directory known to exist on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownDir {
    pub path: String,
    #[serde(default)]
    pub visits: u32,
    /// Unix seconds of the last visit (0 = never visited, only seen in a listing).
    #[serde(default)]
    pub last_visit: u64,
    /// Unix seconds it was last seen in a listing.
    #[serde(default)]
    pub seen: u64,
}

/// One host's known directories, keyed by path.
#[derive(Debug, Clone, Default)]
pub struct KnownDirs {
    map: HashMap<String, KnownDir>,
}

impl KnownDirs {
    pub fn from_vec(list: Vec<KnownDir>) -> Self {
        let mut out = Self::default();
        for k in list {
            if !k.path.trim().is_empty() {
                out.map.insert(k.path.clone(), k);
            }
        }
        out.evict(KNOWN_CAP);
        out
    }

    pub fn to_vec(&self) -> Vec<KnownDir> {
        let mut v: Vec<KnownDir> = self.map.values().cloned().collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &KnownDir> {
        self.map.values()
    }

    /// The entry for `path` (created if missing) and whether it is new.
    fn entry(&mut self, path: &str) -> (&mut KnownDir, bool) {
        let is_new = !self.map.contains_key(path);
        let k = self.map.entry(path.to_string()).or_insert_with(|| KnownDir { path: path.to_string(), visits: 0, last_visit: 0, seen: 0 });
        (k, is_new)
    }

    /// Records a visit to `path` at `now` (unix seconds).
    pub fn visit(&mut self, path: &str, now: u64) {
        if path.trim().is_empty() {
            return;
        }
        let (k, _) = self.entry(path);
        k.visits = k.visits.saturating_add(1);
        k.last_visit = now;
        k.seen = now;
        self.evict(KNOWN_CAP);
    }

    /// Records that `path` exists; returns whether it was new.
    pub fn see(&mut self, path: &str, now: u64) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        let (k, is_new) = self.entry(path);
        k.seen = now;
        if is_new {
            self.evict(KNOWN_CAP);
        }
        is_new
    }

    /// [`Self::see`] for many paths; returns whether any was new.
    pub fn see_all(&mut self, paths: &[String], now: u64) -> bool {
        let mut any = false;
        for p in paths {
            any |= self.see(p, now);
        }
        any
    }

    /// Keeps at most `cap` entries: never-visited ones go first (oldest seen first),
    /// then the least recently visited.
    pub fn evict(&mut self, cap: usize) {
        if self.map.len() <= cap {
            return;
        }
        let mut keys: Vec<(bool, u64, u64, String)> = self.map.values().map(|k| (k.visits > 0, k.last_visit, k.seen, k.path.clone())).collect();
        keys.sort();
        let excess = self.map.len() - cap;
        for (_, _, _, path) in keys.into_iter().take(excess) {
            self.map.remove(&path);
        }
    }
}

pub type KnownMap = BTreeMap<Uuid, Vec<KnownDir>>;

/// Default location: `<config_dir>/sftp_known_dirs.json`.
pub fn known_default_path() -> PathBuf {
    crate::app::config_dir().join(KNOWN_FILE_NAME)
}

fn load_known_map(path: &Path) -> KnownMap {
    crate::json_file::load_or_default(path)
}

/// The known directories of one host.
pub fn load_known(path: &Path, host: Uuid) -> KnownDirs {
    let _guard = FILE_LOCK.lock();
    KnownDirs::from_vec(load_known_map(path).remove(&host).unwrap_or_default())
}

/// Replaces the host's known directories on disk, keeping every other host's.
pub fn save_known(path: &Path, host: Uuid, known: &KnownDirs) -> Result<()> {
    let _guard = FILE_LOCK.lock();
    let mut map = load_known_map(path);
    map.insert(host, known.to_vec());
    write_json_atomic(path, &map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dirs_visit_and_see() {
        let mut k = KnownDirs::default();
        assert!(k.see("/srv", 1));
        assert!(!k.see("/srv", 2), "already known");
        k.visit("/srv", 3);
        k.visit("/srv", 4);
        let e = k.iter().find(|e| e.path == "/srv").unwrap();
        assert_eq!((e.visits, e.last_visit), (2, 4));
        assert!(!k.see("  ", 5));
    }

    #[test]
    fn known_dirs_evict_unvisited_first() {
        let mut k = KnownDirs::default();
        k.visit("/old-visit", 1);
        k.see("/seen-early", 2);
        k.see("/seen-late", 9);
        k.visit("/new-visit", 10);
        k.evict(2);
        let mut paths: Vec<_> = k.iter().map(|e| e.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["/new-visit", "/old-visit"]);
        k.evict(1);
        assert_eq!(k.iter().next().unwrap().path, "/new-visit");
    }

    #[test]
    fn known_dirs_cap_applies_on_insert() {
        let mut k = KnownDirs::default();
        for i in 0..(KNOWN_CAP + 3) {
            k.see(&format!("/d{i}"), i as u64);
        }
        assert_eq!(k.len(), KNOWN_CAP);
    }

    #[test]
    fn known_dirs_persist_per_host() {
        let dir = std::env::temp_dir().join(format!("tether-known-{}", Uuid::new_v4()));
        let file = dir.join(KNOWN_FILE_NAME);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut ka = KnownDirs::default();
        ka.visit("/srv", 5);
        ka.see("/srv/www", 5);
        save_known(&file, a, &ka).unwrap();
        let mut kb = KnownDirs::default();
        kb.see("/etc", 1);
        save_known(&file, b, &kb).unwrap();
        assert_eq!(load_known(&file, a).to_vec(), ka.to_vec());
        assert_eq!(load_known(&file, b).len(), 1);
        assert!(load_known(&file, Uuid::new_v4()).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn push_front_dedups_and_orders_newest_first() {
        let mut l = vec![];
        assert!(push_front(&mut l, "/a"));
        assert!(push_front(&mut l, "/b"));
        assert!(push_front(&mut l, "/a"));
        assert_eq!(l, ["/a", "/b"]);
        assert!(!push_front(&mut l, "/a"), "already newest");
        assert!(!push_front(&mut l, "   "), "blank ignored");
    }

    #[test]
    fn push_front_caps_length() {
        let mut l = vec![];
        for i in 0..(HISTORY_CAP + 5) {
            push_front(&mut l, &format!("/d{i}"));
        }
        assert_eq!(l.len(), HISTORY_CAP);
        assert_eq!(l[0], format!("/d{}", HISTORY_CAP + 4));
    }

    #[test]
    fn record_persists_per_host() {
        let dir = std::env::temp_dir().join(format!("tether-hist-{}", Uuid::new_v4()));
        let file = dir.join(FILE_NAME);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        record(&file, a, "/srv").unwrap();
        record(&file, a, "/home/u").unwrap();
        record(&file, b, "/var/log").unwrap();
        assert_eq!(load(&file, a), ["/home/u", "/srv"]);
        assert_eq!(load(&file, b), ["/var/log"]);
        assert!(load(&file, Uuid::new_v4()).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
