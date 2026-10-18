//! Pipelined SFTP transfer engine.
//!
//! Bitvise's speed comes from keeping a large "pipeline" of outstanding READ/WRITE
//! requests (`-pipelineSize`, default 512 KiB) instead of the request/response
//! ping-pong most clients do. We do the same: `pipeline_bytes / chunk` requests are
//! kept in flight per file, results are consumed in order so the local file is
//! written sequentially, and many small files are transferred concurrently.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use futures::stream::{FuturesOrdered, StreamExt};
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};

use super::sftp::{Sftp, is_eof, join};

const MIN_DEPTH: usize = 4;
const MAX_DEPTH: usize = 256;
/// Files transferred side by side; helps a lot with many small files.
const PARALLEL_FILES: usize = 4;
const LOCAL_BUF: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub files_done: AtomicU64,
    pub files_total: AtomicU64,
    cancel: AtomicBool,
    /// Set internally after the first failure so sibling jobs stop early.
    abort: AtomicBool,
}

impl Progress {
    fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn should_stop(&self) -> bool {
        self.is_cancelled() || self.abort.load(Ordering::Relaxed)
    }

    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 { 0.0 } else { (self.done.load(Ordering::Relaxed) as f64 / total as f64) as f32 }
    }
}

pub struct Transfer {
    pub id: u64,
    pub direction: Direction,
    pub label: String,
    pub progress: Arc<Progress>,
    pub state: Arc<Mutex<TransferState>>,
    pub started: Instant,
    pub finished: Arc<Mutex<Option<Instant>>>,
}

impl Transfer {
    pub fn state(&self) -> TransferState {
        self.state.lock().map(|s| s.clone()).unwrap_or(TransferState::Failed("poisoned".into()))
    }

    /// Average throughput in bytes/s.
    pub fn speed(&self) -> f64 {
        let end = self.finished.lock().ok().and_then(|f| *f).unwrap_or_else(Instant::now);
        let secs = end.duration_since(self.started).as_secs_f64().max(0.001);
        self.progress.done.load(Ordering::Relaxed) as f64 / secs
    }
}

pub fn pipeline_depth(pipeline_kb: u32, chunk: u32) -> usize {
    let bytes = pipeline_kb as usize * 1024;
    (bytes / chunk.max(1) as usize).clamp(MIN_DEPTH, MAX_DEPTH)
}

struct FileJob {
    remote: String,
    local: PathBuf,
    size: u64,
    mtime: Option<u64>,
}

/// Transfers `local` <-> `remote` (file or directory tree).
pub async fn run(sftp: Arc<Sftp>, direction: Direction, local: PathBuf, remote: String, pipeline_kb: u32, progress: Arc<Progress>) -> Result<()> {
    let jobs = match direction {
        Direction::Download => plan_download(&sftp, &remote, &local).await?,
        Direction::Upload => plan_upload(&sftp, &local, &remote).await?,
    };
    progress.total.store(jobs.iter().map(|j| j.size).sum(), Ordering::Relaxed);
    progress.files_total.store(jobs.len() as u64, Ordering::Relaxed);

    let results = futures::stream::iter(jobs)
        .map(|job| {
            let (sftp, progress) = (sftp.clone(), progress.clone());
            async move {
                if progress.should_stop() {
                    bail!("Cancelled");
                }
                let result = match direction {
                    Direction::Download => download_file(&sftp, &job, pipeline_kb, &progress).await,
                    Direction::Upload => upload_file(&sftp, &job, pipeline_kb, &progress).await,
                }
                .with_context(|| format!("{} -> {}", job.remote, job.local.display()));
                if result.is_err() {
                    progress.abort.store(true, Ordering::Relaxed);
                }
                result?;
                progress.files_done.fetch_add(1, Ordering::Relaxed);
                anyhow::Ok(())
            }
        })
        .buffer_unordered(PARALLEL_FILES)
        .collect::<Vec<_>>()
        .await;
    // Report the root cause, not the "Cancelled" of jobs stopped because of it.
    let mut errors: Vec<anyhow::Error> = results.into_iter().filter_map(Result::err).collect();
    if let Some(i) = errors.iter().position(|e| e.to_string() != "Cancelled") {
        return Err(errors.swap_remove(i));
    }
    errors.into_iter().next().map_or(Ok(()), Err)
}

/// A remote name is only usable locally if it is one plain path component.
/// Rejects `..`, separators, drive prefixes and NTFS stream syntax (`a:b`), which a
/// hostile server could use to write outside the destination directory.
pub fn is_safe_local_name(name: &str) -> bool {
    use std::path::Component;
    if name.is_empty() || name.contains(['/', '\\', ':', '\0']) {
        return false;
    }
    let mut parts = Path::new(name).components();
    matches!((parts.next(), parts.next()), (Some(Component::Normal(n)), None) if n == name)
}

async fn plan_download(sftp: &Sftp, remote: &str, local: &Path) -> Result<Vec<FileJob>> {
    let attrs = sftp.stat(remote).await.with_context(|| format!("stat {remote}"))?;
    if !attrs.is_dir() {
        let mtime = attrs.mtime.map(u64::from);
        return Ok(vec![FileJob { remote: remote.into(), local: local.into(), size: attrs.size.unwrap_or(0), mtime }]);
    }
    let mut jobs = Vec::new();
    let mut stack = vec![(remote.to_string(), local.to_path_buf())];
    while let Some((rdir, ldir)) = stack.pop() {
        tokio::fs::create_dir_all(&ldir).await.with_context(|| format!("creating {}", ldir.display()))?;
        for e in sftp.list(&rdir).await? {
            if !is_safe_local_name(&e.name) {
                bail!("Refusing unsafe remote file name {:?} in {rdir}", e.name);
            }
            let (r, l) = (join(&rdir, &e.name), ldir.join(&e.name));
            if e.is_link {
                // readdir reports the link itself; follow it for files, skip linked
                // directories so a link cycle can't recurse forever.
                if let Ok(a) = sftp.stat(&r).await
                    && !a.is_dir()
                {
                    jobs.push(FileJob { remote: r, local: l, size: a.size.unwrap_or(0), mtime: a.mtime.map(u64::from) });
                }
            } else if e.is_dir {
                stack.push((r, l));
            } else {
                jobs.push(FileJob { remote: r, local: l, size: e.size, mtime: Some(e.mtime) });
            }
        }
    }
    Ok(jobs)
}

async fn plan_upload(sftp: &Sftp, local: &Path, remote: &str) -> Result<Vec<FileJob>> {
    let meta = tokio::fs::metadata(local).await.with_context(|| format!("reading {}", local.display()))?;
    if !meta.is_dir() {
        return Ok(vec![FileJob { remote: remote.into(), local: local.into(), size: meta.len(), mtime: mtime_of(&meta) }]);
    }
    let mut jobs = Vec::new();
    let mut stack = vec![(local.to_path_buf(), remote.to_string())];
    while let Some((ldir, rdir)) = stack.pop() {
        sftp.ensure_dir(&rdir).await?;
        let mut rd = tokio::fs::read_dir(&ldir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let meta = entry.metadata().await?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let (l, r) = (entry.path(), join(&rdir, &name));
            if meta.is_dir() {
                stack.push((l, r));
            } else {
                jobs.push(FileJob { remote: r, local: l, size: meta.len(), mtime: mtime_of(&meta) });
            }
        }
    }
    Ok(jobs)
}

fn mtime_of(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Downloads into `<name>.part` and renames on success, so a failed or cancelled
/// transfer never clobbers an existing file with a truncated one.
async fn download_file(sftp: &Sftp, job: &FileJob, pipeline_kb: u32, p: &Progress) -> Result<()> {
    let handle = sftp.raw.open(job.remote.as_str(), OpenFlags::READ, FileAttributes::empty()).await?.handle;
    let mut part = job.local.clone().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    let result = download_body(sftp, &handle, &part, pipeline_kb, p).await;
    let _ = sftp.raw.close(handle).await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, &job.local).await.with_context(|| format!("replacing {}", job.local.display()))?;
    if let Some(m) = job.mtime {
        let t = UNIX_EPOCH + Duration::from_secs(m);
        if let Ok(f) = std::fs::File::options().write(true).open(&job.local) {
            let _ = f.set_modified(t);
        }
    }
    Ok(())
}

async fn download_body(sftp: &Sftp, handle: &str, dest: &Path, pipeline_kb: u32, p: &Progress) -> Result<()> {
    let chunk = sftp.read_chunk;
    let depth = pipeline_depth(pipeline_kb, chunk);
    let file = tokio::fs::File::create(dest).await.with_context(|| format!("creating {}", dest.display()))?;
    let mut out = BufWriter::with_capacity(LOCAL_BUF, file);
    let mut pending = FuturesOrdered::new();
    let (mut next_offset, mut eof) = (0u64, false);

    loop {
        while !eof && pending.len() < depth {
            let off = next_offset;
            next_offset += chunk as u64;
            pending.push_back(async move { (off, sftp.raw.read(handle, off, chunk).await) });
        }
        let Some((off, reply)) = pending.next().await else { break };
        if p.should_stop() {
            bail!("Cancelled");
        }
        let data = match reply {
            Ok(d) if !d.data.is_empty() => d.data,
            Ok(_) => {
                eof = true;
                continue;
            }
            Err(e) if is_eof(&e) => {
                eof = true;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        out.write_all(&data).await?;
        let mut got = data.len() as u64;
        p.add(got);
        // A short read without EOF leaves a gap before the next in-flight chunk; fill it.
        while got < chunk as u64 && !eof {
            match sftp.raw.read(handle, off + got, chunk - got as u32).await {
                Ok(d) if !d.data.is_empty() => {
                    out.write_all(&d.data).await?;
                    got += d.data.len() as u64;
                    p.add(d.data.len() as u64);
                }
                Ok(_) => eof = true,
                Err(e) if is_eof(&e) => eof = true,
                Err(e) => return Err(e.into()),
            }
        }
    }
    out.flush().await?;
    Ok(())
}

async fn upload_file(sftp: &Sftp, job: &FileJob, pipeline_kb: u32, p: &Progress) -> Result<()> {
    let flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
    let handle = sftp.raw.open(job.remote.as_str(), flags, FileAttributes::empty()).await?.handle;
    let result = upload_body(sftp, &handle, job, pipeline_kb, p).await;
    if result.is_ok()
        && let Some(m) = job.mtime
    {
        let attrs = FileAttributes { mtime: Some(m as u32), atime: Some(now_secs()), ..FileAttributes::empty() };
        let _ = sftp.raw.fsetstat(handle.as_str(), attrs).await;
    }
    // Some servers only report write failures when the handle is closed.
    let closed = sftp.raw.close(handle).await;
    result?;
    closed.context("closing remote file")?;
    Ok(())
}

async fn upload_body(sftp: &Sftp, handle: &str, job: &FileJob, pipeline_kb: u32, p: &Progress) -> Result<()> {
    let chunk = sftp.write_chunk as usize;
    let depth = pipeline_depth(pipeline_kb, chunk as u32);
    let mut file = tokio::fs::File::open(&job.local).await.with_context(|| format!("opening {}", job.local.display()))?;
    let mut pending = FuturesOrdered::new();
    let (mut offset, mut local_eof) = (0u64, false);

    loop {
        while !local_eof && pending.len() < depth {
            let buf = read_full(&mut file, chunk).await?;
            if buf.is_empty() {
                local_eof = true;
                break;
            }
            let (off, len) = (offset, buf.len() as u64);
            offset += len;
            pending.push_back(async move { sftp.raw.write(handle, off, buf).await.map(|_| len) });
        }
        let Some(reply) = pending.next().await else { break };
        p.add(reply?);
        if p.should_stop() {
            bail!("Cancelled");
        }
    }
    Ok(())
}

/// Reads up to `n` bytes, looping over short reads; empty means EOF.
async fn read_full(file: &mut tokio::fs::File, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        let read = file.read(&mut buf[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buf.truncate(filled);
    Ok(buf)
}

fn now_secs() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_follows_pipeline_size() {
        // Bitvise default: 512 KiB over 32 KiB chunks = 16 outstanding requests.
        assert_eq!(pipeline_depth(512, 32 * 1024), 16);
        assert_eq!(pipeline_depth(2048, 255 * 1024), 8);
        assert_eq!(pipeline_depth(16, 255 * 1024), MIN_DEPTH);
        assert_eq!(pipeline_depth(16384, 1024), MAX_DEPTH);
    }

    #[test]
    fn rejects_names_that_escape_the_directory() {
        for bad in ["", ".", "..", "../x", "..\\x", "a/b", "C:\\x", "C:", "file:stream", "nul\0"] {
            assert!(!is_safe_local_name(bad), "{bad:?} must be rejected");
        }
        for ok in ["file.txt", ".bashrc", "my dir", "naïve", "a..b"] {
            assert!(is_safe_local_name(ok), "{ok:?} must be accepted");
        }
    }

    #[test]
    fn progress_fraction() {
        let p = Progress::default();
        assert_eq!(p.fraction(), 0.0);
        p.total.store(200, Ordering::Relaxed);
        p.add(50);
        assert!((p.fraction() - 0.25).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn read_full_handles_eof() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"hello world").unwrap();
        let mut f = tokio::fs::File::open(&path).await.unwrap();
        assert_eq!(read_full(&mut f, 5).await.unwrap(), b"hello");
        assert_eq!(read_full(&mut f, 100).await.unwrap(), b" world");
        assert!(read_full(&mut f, 100).await.unwrap().is_empty());
    }
}
