//! Data helpers behind the SFTP view: remote and local listings, the per-host cache
//! bookkeeping and the persisted path history.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use uuid::Uuid;

use super::dir_cache::SharedHostCache;
use super::file_list::Row;
use super::path_complete::{self, Candidate};
use super::suggest::{self, PathEntry, SUGGEST_LIMIT};
use crate::profile::unix_now;
use crate::sftp_history::{self, KnownDirs};
use crate::ssh::sftp::Sftp;

pub fn set<T>(m: &Mutex<T>, f: impl FnOnce(&mut T)) {
    if let Ok(mut g) = m.lock() {
        f(&mut g);
    }
}

pub async fn list_rows(sftp: &Sftp, path: &str) -> Result<Vec<Row>> {
    Ok(sftp.list(path).await?.into_iter().map(|e| Row { name: e.name, is_dir: e.is_dir, is_link: e.is_link, size: e.size, mtime: e.mtime }).collect())
}

/// Lists a local directory; an empty path lists drive roots (Windows) or `/`.
pub fn list_local(path: &Path) -> std::io::Result<Vec<Row>> {
    if path.as_os_str().is_empty() {
        return Ok(drive_roots());
    }
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        rows.push(Row {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            is_link: meta.file_type().is_symlink(),
            size: meta.len(),
            mtime,
        });
    }
    rows.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(rows)
}

fn drive_roots() -> Vec<Row> {
    if !cfg!(windows) {
        return vec![Row { name: "/".into(), is_dir: true, is_link: false, size: 0, mtime: 0 }];
    }
    (b'A'..=b'Z')
        .map(|c| format!("{}:\\", c as char))
        .filter(|d| Path::new(d).exists())
        .map(|name| Row { name, is_dir: true, is_link: false, size: 0, mtime: 0 })
        .collect()
}

pub fn candidates(rows: &[Row]) -> Vec<Candidate> {
    rows.iter().map(|r| Candidate { name: r.name.clone(), is_dir: r.is_dir }).collect()
}

/// Cached listing of `dir`, if any.
pub fn cached(cache: &SharedHostCache, dir: &str) -> Option<Vec<Row>> {
    cache.lock().ok()?.listings.get(dir).map(|(rows, _)| rows)
}

/// Stores a fresh listing in the host cache; a visit is also persisted to the known dirs.
pub fn learn(cache: &SharedHostCache, host: Uuid, dir: &str, rows: &[Row], visited: bool) {
    let now = unix_now();
    let snapshot = cache.lock().ok().and_then(|mut c| {
        c.store(dir, rows, now);
        if visited {
            c.visit(dir, now);
        }
        visited.then(|| c.known.clone())
    });
    // Suggestions keep working from memory if the config dir is not writable.
    if let Some(known) = snapshot {
        let _ = sftp_history::save_known(&sftp_history::known_default_path(), host, &known);
    }
}

/// Remote suggestions for the absolute `query`: known dirs (visited and seen in listings)
/// plus the recent-folders history.
pub fn rank_remote(cache: Option<&crate::ui::dir_cache::HostCache>, history: &[String], query: &str) -> Vec<String> {
    let known = cache.into_iter().flat_map(|c| c.known.iter()).map(|k| PathEntry { path: &k.path, visits: k.visits, last_visit: k.last_visit });
    let recent = history.iter().map(|p| PathEntry { path: p, visits: 1, last_visit: 0 });
    suggest::rank_paths(query, known.chain(recent), path_complete::REMOTE, unix_now(), SUGGEST_LIMIT)
}

/// Local suggestions: folders visited (persisted across sessions) plus recent local
/// history plus the sub-folders of the directory being typed into.
pub fn rank_local(known: &KnownDirs, history: &[String], query: &str) -> Vec<String> {
    let style = path_complete::LOCAL;
    let (dir, _) = path_complete::split(query, style);
    let dir = path_complete::listing_dir(dir, style);
    let base = PathBuf::from(&dir);
    let children: Vec<String> = list_local(&base)
        .map(|rows| rows.into_iter().filter(|r| r.is_dir).map(|r| if dir.is_empty() { r.name } else { base.join(r.name).display().to_string() }).collect())
        .unwrap_or_default();
    let visited = known.iter().map(|k| PathEntry { path: &k.path, visits: k.visits, last_visit: k.last_visit });
    let recent = history.iter().map(|p| PathEntry { path: p, visits: 1, last_visit: 0 });
    let listed = children.iter().map(|p| PathEntry { path: p, visits: 0, last_visit: 0 });
    suggest::rank_paths(query, visited.chain(recent).chain(listed), style, unix_now(), SUGGEST_LIMIT)
}

/// Records successful remote navigations into the host's persisted path history.
pub struct HistoryRecorder {
    pub file: PathBuf,
    pub host: Uuid,
    pub list: Arc<Mutex<Vec<String>>>,
}

impl HistoryRecorder {
    pub fn record(&self, dir: &str) {
        match sftp_history::record(&self.file, self.host, dir) {
            Ok(updated) => set(&self.list, |l| *l = updated),
            // Persisting failed (read-only config dir?): still keep it for this session.
            Err(_) => set(&self.list, |l| {
                sftp_history::push_front(l, dir);
            }),
        }
    }
}
