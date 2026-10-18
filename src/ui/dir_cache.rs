//! In-memory per-host cache of remote directory listings, shared by every SFTP pane of
//! the same host, plus the host's set of known directories used for suggestions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use uuid::Uuid;

use super::file_list::Row;
use crate::sftp_history::{self, KnownDirs};

/// Listings kept per host before the least recently used one is dropped.
pub const DIR_CACHE_CAP: usize = 500;

struct Entry<V> {
    value: V,
    at: Instant,
    tick: u64,
}

/// Small LRU map keyed by normalised path.
pub struct DirCache<V> {
    cap: usize,
    tick: u64,
    map: HashMap<String, Entry<V>>,
}

impl<V: Clone> DirCache<V> {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), tick: 0, map: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn insert(&mut self, path: &str, value: V) {
        self.tick += 1;
        self.map.insert(path.to_string(), Entry { value, at: Instant::now(), tick: self.tick });
        while self.map.len() > self.cap {
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.tick).map(|(k, _)| k.clone()) else { break };
            self.map.remove(&oldest);
        }
    }

    /// The cached value and when it was fetched; marks it as recently used.
    pub fn get(&mut self, path: &str) -> Option<(V, Instant)> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(path).map(|e| {
            e.tick = tick;
            (e.value.clone(), e.at)
        })
    }

    pub fn contains(&self, path: &str) -> bool {
        self.map.contains_key(path)
    }
}

/// Everything remembered about one host's remote tree.
pub struct HostCache {
    pub listings: DirCache<Vec<Row>>,
    pub known: KnownDirs,
    /// Bumped whenever `known` changes, so suggestion lists know to recompute.
    pub version: u64,
}

pub type SharedHostCache = Arc<Mutex<HostCache>>;

/// The shared cache of `host`, loading its known directories from disk on first use.
pub fn host_cache(host: Uuid) -> SharedHostCache {
    static ALL: OnceLock<Mutex<HashMap<Uuid, SharedHostCache>>> = OnceLock::new();
    let all = ALL.get_or_init(Default::default);
    let mut all = all.lock().unwrap_or_else(|e| e.into_inner());
    all.entry(host)
        .or_insert_with(|| {
            let known = sftp_history::load_known(&sftp_history::known_default_path(), host);
            Arc::new(Mutex::new(HostCache { listings: DirCache::new(DIR_CACHE_CAP), known, version: 1 }))
        })
        .clone()
}

impl HostCache {
    /// Stores a fresh listing of `dir` and learns its sub-directories.
    pub fn store(&mut self, dir: &str, rows: &[Row], now: u64) {
        self.listings.insert(dir, rows.to_vec());
        let children: Vec<String> = rows.iter().filter(|r| r.is_dir).map(|r| crate::ssh::sftp::join(dir, &r.name)).collect();
        if self.known.see(dir, now) | self.known.see_all(&children, now) {
            self.version += 1;
        }
    }

    pub fn visit(&mut self, dir: &str, now: u64) {
        self.known.visit(dir, now);
        self.version += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut c = DirCache::new(2);
        c.insert("/a", 1);
        c.insert("/b", 2);
        assert_eq!(c.get("/a").map(|v| v.0), Some(1)); // /a is now newer than /b
        c.insert("/c", 3);
        assert_eq!(c.len(), 2);
        assert!(c.contains("/a") && c.contains("/c") && !c.contains("/b"));
    }

    #[test]
    fn reinsert_replaces_value() {
        let mut c = DirCache::new(4);
        c.insert("/a", 1);
        c.insert("/a", 5);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get("/a").map(|v| v.0), Some(5));
        assert!(c.get("/missing").is_none());
    }

    #[test]
    fn store_learns_child_dirs() {
        let mut hc = HostCache { listings: DirCache::new(4), known: KnownDirs::default(), version: 0 };
        let row = |n: &str, d: bool| Row { name: n.into(), is_dir: d, is_link: false, size: 0, mtime: 0 };
        hc.store("/srv", &[row("www", true), row("a.txt", false)], 10);
        assert!(hc.version > 0);
        let paths: Vec<_> = hc.known.iter().map(|k| k.path.clone()).collect();
        assert!(paths.contains(&"/srv/www".to_string()));
        assert!(!paths.contains(&"/srv/a.txt".to_string()));
        let v = hc.version;
        hc.store("/srv", &[row("www", true)], 11);
        assert_eq!(hc.version, v, "nothing new learned");
    }
}
