//! Editing remote files: text decoding rules for the built-in editor, atomic uploads, and
//! the Bitvise-style "open with the PC's default app" flow (download to a temp folder,
//! watch it, upload every saved change).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};

use crate::ssh::sftp::{Sftp, is_eof};
use crate::ssh::transfer::{self, Direction, Progress};

/// Largest file the built-in editor opens.
pub const MAX_EDITOR_BYTES: u64 = 10 * 1024 * 1024;
/// A NUL byte in this many leading bytes marks a file as binary.
const BINARY_SNIFF: usize = 8 * 1024;
const TEMP_SUFFIX: &str = ".tether-tmp";
const POLL: Duration = Duration::from_millis(700);
const DEBOUNCE: Duration = Duration::from_millis(300);
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

// ---- pure helpers -------------------------------------------------------------

/// Temporary sibling an upload is written to before it replaces `path`.
pub fn temp_name(path: &str) -> String {
    format!("{path}{TEMP_SUFFIX}")
}

/// Why the built-in editor will not open a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Binary,
    NotUtf8,
    TooLarge(u64),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary => f.write_str("This looks like a binary file."),
            Self::NotUtf8 => f.write_str("This file is not valid UTF-8 text."),
            Self::TooLarge(n) => {
                write!(f, "This file is {} (the editor opens files up to {}).", crate::theme::human_bytes(*n), crate::theme::human_bytes(MAX_EDITOR_BYTES))
            }
        }
    }
}

impl std::error::Error for Refusal {}

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0)
}

/// Text plus the file conventions needed to write it back byte-compatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Contents with `\n` line endings.
    pub text: String,
    pub crlf: bool,
    pub bom: bool,
}

pub fn decode(bytes: &[u8]) -> Result<Decoded, Refusal> {
    if bytes.len() as u64 > MAX_EDITOR_BYTES {
        return Err(Refusal::TooLarge(bytes.len() as u64));
    }
    if looks_binary(bytes) {
        return Err(Refusal::Binary);
    }
    let bom = bytes.starts_with(UTF8_BOM);
    let body = if bom { &bytes[UTF8_BOM.len()..] } else { bytes };
    let raw = std::str::from_utf8(body).map_err(|_| Refusal::NotUtf8)?;
    let crlf_count = raw.matches("\r\n").count();
    let lf_count = raw.matches('\n').count();
    // CRLF when most line breaks are CRLF; a mixed file keeps its lone `\r`s as text.
    let crlf = crlf_count > 0 && crlf_count * 2 >= lf_count;
    let text = if crlf { raw.replace("\r\n", "\n") } else { raw.to_string() };
    Ok(Decoded { text, crlf, bom })
}

pub fn encode(text: &str, crlf: bool, bom: bool) -> Vec<u8> {
    let body = if crlf { text.replace('\n', "\r\n") } else { text.to_string() };
    let mut out = Vec::with_capacity(body.len() + 3);
    if bom {
        out.extend_from_slice(UTF8_BOM);
    }
    out.extend_from_slice(body.as_bytes());
    out
}

/// Size + modification time of a watched file.
pub type Signature = (Option<SystemTime>, u64);

/// Decides when a watched file has changed and settled (no further change for
/// [`DEBOUNCE`]), so an editor that saves in several writes triggers one upload.
#[derive(Debug, Clone)]
pub struct ChangeTracker {
    baseline: Signature,
    pending: Option<(Signature, Instant)>,
}

impl ChangeTracker {
    pub fn new(baseline: Signature) -> Self {
        Self { baseline, pending: None }
    }

    /// Feeds a new observation; returns the signature to upload once it has settled.
    pub fn observe(&mut self, sig: Signature, now: Instant) -> Option<Signature> {
        if sig == self.baseline {
            self.pending = None;
            return None;
        }
        match self.pending {
            Some((p, since)) if p == sig && now.duration_since(since) >= DEBOUNCE => Some(sig),
            Some((p, _)) if p == sig => None,
            _ => {
                self.pending = Some((sig, now));
                None
            }
        }
    }

    /// The upload of `sig` is done (or abandoned): stop reporting it.
    pub fn commit(&mut self, sig: Signature) {
        self.baseline = sig;
        self.pending = None;
    }

    /// How long to wait before the next observation.
    pub fn next_delay(&self) -> Duration {
        if self.pending.is_some() { DEBOUNCE } else { POLL }
    }
}

fn ssh_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

// ---- remote IO ------------------------------------------------------------------

/// Reads a whole remote file, refusing (with a [`Refusal`] error) files over `cap` bytes.
pub async fn read_capped(sftp: &Sftp, path: &str, cap: u64) -> Result<Vec<u8>> {
    let attrs = sftp.stat(path).await.with_context(|| format!("stat {path}"))?;
    if attrs.is_dir() {
        bail!("{path} is a folder");
    }
    let size = attrs.size.unwrap_or(0);
    if size > cap {
        return Err(Refusal::TooLarge(size).into());
    }
    let handle = sftp.raw.open(path, OpenFlags::READ, FileAttributes::empty()).await.with_context(|| format!("opening {path}"))?.handle;
    let mut out = Vec::with_capacity(size as usize);
    let result = loop {
        match sftp.raw.read(handle.as_str(), out.len() as u64, sftp.read_chunk).await {
            Ok(d) if d.data.is_empty() => break Ok(()),
            Ok(d) => {
                out.extend_from_slice(&d.data);
                if out.len() as u64 > cap {
                    break Err(Refusal::TooLarge(out.len() as u64).into());
                }
            }
            Err(e) if is_eof(&e) => break Ok(()),
            Err(e) => break Err(anyhow::Error::from(e).context(format!("reading {path}"))),
        }
    };
    let _ = sftp.raw.close(handle).await;
    result.map(|()| out)
}

/// Writes `data` to `path` so readers never see a half-written file: the bytes go to
/// `<path>.tether-tmp`, which then takes the original's permissions (and owner, when the
/// server allows) and is renamed over it. Symlinks, and folders where a sibling cannot be
/// created, are written in place instead.
pub async fn upload_atomic(sftp: &Sftp, path: &str, data: &[u8], pipeline_kb: u32) -> Result<()> {
    let existing = sftp.raw.lstat(path).await.ok().map(|a| a.attrs);
    if existing.as_ref().is_some_and(FileAttributes::is_symlink) {
        return write_in_place(sftp, path, data, pipeline_kb).await;
    }
    let tmp = temp_name(path);
    let create = FileAttributes { permissions: existing.as_ref().and_then(|a| a.permissions).map(|m| m & 0o7777), ..FileAttributes::empty() };
    let flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
    let handle = match sftp.raw.open(tmp.as_str(), flags, create).await {
        Ok(h) => h.handle,
        // Directory not writable (but maybe the file is): fall back to an in-place write.
        Err(_) => return write_in_place(sftp, path, data, pipeline_kb).await,
    };
    let written = write_all(sftp, &handle, data, pipeline_kb).await;
    let closed = sftp.raw.close(handle).await;
    if let Err(e) = written.and_then(|()| closed.map(|_| ()).context("closing remote file")) {
        let _ = sftp.raw.remove(tmp.as_str()).await;
        return Err(e.context(format!("writing {tmp}")));
    }
    if let Some(old) = &existing {
        restore_attrs(sftp, &tmp, old).await;
    }
    if let Err(e) = replace(sftp, &tmp, path).await {
        let _ = sftp.raw.remove(tmp.as_str()).await;
        return Err(e.context(format!("replacing {path}")));
    }
    Ok(())
}

async fn restore_attrs(sftp: &Sftp, tmp: &str, old: &FileAttributes) {
    if let Some(mode) = old.permissions {
        let attrs = FileAttributes { permissions: Some(mode & 0o7777), ..FileAttributes::empty() };
        let _ = sftp.raw.setstat(tmp, attrs).await;
    }
    if let (Some(uid), Some(gid)) = (old.uid, old.gid) {
        // Only succeeds for root or when nothing changes; harmless otherwise.
        let attrs = FileAttributes { uid: Some(uid), gid: Some(gid), ..FileAttributes::empty() };
        let _ = sftp.raw.setstat(tmp, attrs).await;
    }
}

/// Renames `from` over `to`: atomic `posix-rename@openssh.com` when available, otherwise a
/// plain rename (which SFTP v3 refuses over an existing file, so remove the target first).
async fn replace(sftp: &Sftp, from: &str, to: &str) -> Result<()> {
    let mut data = Vec::new();
    ssh_string(&mut data, from);
    ssh_string(&mut data, to);
    if let Ok(Packet::Status(s)) = sftp.raw.extended("posix-rename@openssh.com", data).await
        && s.status_code == StatusCode::Ok
    {
        return Ok(());
    }
    if sftp.raw.rename(from, to).await.is_ok() {
        return Ok(());
    }
    let _ = sftp.raw.remove(to).await;
    sftp.raw.rename(from, to).await?;
    Ok(())
}

async fn write_in_place(sftp: &Sftp, path: &str, data: &[u8], pipeline_kb: u32) -> Result<()> {
    let flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
    let handle = sftp.raw.open(path, flags, FileAttributes::empty()).await.with_context(|| format!("opening {path} for writing"))?.handle;
    let written = write_all(sftp, &handle, data, pipeline_kb).await;
    let closed = sftp.raw.close(handle).await;
    written?;
    closed.context("closing remote file")?;
    Ok(())
}

/// Pipelined WRITE requests (same depth rule as the transfer engine).
async fn write_all(sftp: &Sftp, handle: &str, data: &[u8], pipeline_kb: u32) -> Result<()> {
    let chunk = sftp.write_chunk.max(1) as usize;
    let depth = transfer::pipeline_depth(pipeline_kb, chunk as u32);
    // Owned items: borrowed ones make the spawned future's lifetimes not general enough.
    let parts: Vec<(u64, Vec<u8>)> = data.chunks(chunk).enumerate().map(|(i, p)| ((i * chunk) as u64, p.to_vec())).collect();
    let results: Vec<_> =
        futures::stream::iter(parts).map(|(offset, part)| sftp.raw.write(handle.to_string(), offset, part)).buffer_unordered(depth).collect().await;
    for r in results {
        r?;
    }
    Ok(())
}

// ---- system app + watcher --------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum WatchStatus {
    Downloading,
    Watching,
    Uploading,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct WatchEntry {
    pub id: u64,
    pub remote_path: String,
    pub local_path: PathBuf,
    pub status: WatchStatus,
    pub last_upload: Option<Instant>,
    pub uploads: u32,
    /// Last upload failure while still watching (cleared by the next good upload).
    pub last_error: Option<String>,
    stop: Arc<AtomicBool>,
}

impl WatchEntry {
    pub fn file_name(&self) -> &str {
        self.remote_path.rsplit('/').next().unwrap_or(&self.remote_path)
    }
}

/// Files of one SFTP view opened with the system's default app. Cloning shares the list.
#[derive(Clone, Default)]
pub struct Watcher {
    entries: Arc<Mutex<Vec<WatchEntry>>>,
    next: Arc<AtomicU64>,
    /// Remote folders that received an upload since the last [`Self::take_uploaded_dirs`].
    uploaded_dirs: Arc<Mutex<Vec<String>>>,
}

/// Everything a watch task needs besides the entry itself.
pub struct WatchContext {
    pub sftp: Arc<Sftp>,
    pub pipeline_kb: u32,
    pub rt: tokio::runtime::Handle,
    pub ctx: eframe::egui::Context,
}

impl Watcher {
    pub fn entries(&self) -> Vec<WatchEntry> {
        self.entries.lock().map(|e| e.clone()).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().map(|e| e.is_empty()).unwrap_or(true)
    }

    pub fn take_uploaded_dirs(&self) -> Vec<String> {
        self.uploaded_dirs.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default()
    }

    /// Downloads `remote_path`, opens it with the default app and uploads every change.
    /// Opening a file that is already watched just shows the local copy again.
    pub fn open(&self, remote_path: String, wc: WatchContext) {
        let name = remote_path.rsplit('/').next().unwrap_or_default().to_string();
        if !transfer::is_safe_local_name(&name) {
            self.push_failed(remote_path, format!("Refusing unsafe remote file name {name:?}"));
            return;
        }
        let existing = self.entries.lock().ok().and_then(|e| {
            e.iter().find(|w| w.remote_path == remote_path && matches!(w.status, WatchStatus::Watching | WatchStatus::Uploading)).map(|w| w.local_path.clone())
        });
        if let Some(local) = existing {
            if let Err(e) = open_with_system(&local) {
                self.push_failed(remote_path, format!("{e:#}"));
            }
            return;
        }
        let dir = std::env::temp_dir().join("tether-edit").join(uuid::Uuid::new_v4().simple().to_string());
        let local = dir.join(&name);
        let stop = Arc::new(AtomicBool::new(false));
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut e) = self.entries.lock() {
            e.push(WatchEntry {
                id,
                remote_path: remote_path.clone(),
                local_path: local.clone(),
                status: WatchStatus::Downloading,
                last_upload: None,
                uploads: 0,
                last_error: None,
                stop: stop.clone(),
            });
        }
        let this = self.clone();
        let rt = wc.rt.clone();
        rt.spawn(async move {
            let ctx = wc.ctx.clone();
            if let Err(e) = this.run(id, &remote_path, &dir, &local, &stop, wc).await {
                this.set_status(id, WatchStatus::Failed(format!("{e:#}")));
                ctx.request_repaint();
                // Keep the failed entry visible until the user dismisses it.
                while !stop.load(Ordering::Relaxed) {
                    tokio::time::sleep(POLL).await;
                }
            }
            let _ = tokio::fs::remove_dir_all(&dir).await;
            this.remove(id);
            ctx.request_repaint();
        });
    }

    async fn run(&self, id: u64, remote: &str, dir: &Path, local: &Path, stop: &AtomicBool, wc: WatchContext) -> Result<()> {
        tokio::fs::create_dir_all(dir).await.with_context(|| format!("creating {}", dir.display()))?;
        let progress = Arc::new(Progress::default());
        transfer::run(wc.sftp.clone(), Direction::Download, local.to_path_buf(), remote.to_string(), wc.pipeline_kb, progress).await?;
        let mut tracker = ChangeTracker::new(signature(local).await?);
        self.set_status(id, WatchStatus::Watching);
        wc.ctx.request_repaint();
        open_with_system(local)?;
        while !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(tracker.next_delay()).await;
            // A save in progress (or a locked file) is simply seen again on the next poll.
            let Ok(sig) = signature(local).await else { continue };
            let Some(ready) = tracker.observe(sig, Instant::now()) else { continue };
            let Ok(bytes) = tokio::fs::read(local).await else { continue };
            self.set_status(id, WatchStatus::Uploading);
            wc.ctx.request_repaint();
            let result = upload_atomic(&wc.sftp, remote, &bytes, wc.pipeline_kb).await;
            tracker.commit(ready);
            match result {
                Ok(()) => {
                    self.mark_uploaded(id);
                    if let Ok(mut d) = self.uploaded_dirs.lock() {
                        d.push(crate::ssh::sftp::parent(remote));
                    }
                }
                Err(e) => self.update(id, |w| {
                    w.status = WatchStatus::Watching;
                    w.last_error = Some(format!("Upload failed: {e:#}"));
                }),
            }
            wc.ctx.request_repaint();
        }
        Ok(())
    }

    /// Stops watching one file and deletes its local copy.
    pub fn stop(&self, id: u64) {
        if let Ok(e) = self.entries.lock()
            && let Some(w) = e.iter().find(|w| w.id == id)
        {
            w.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn stop_all(&self) {
        if let Ok(e) = self.entries.lock() {
            for w in e.iter() {
                w.stop.store(true, Ordering::Relaxed);
            }
        }
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut WatchEntry)) {
        if let Ok(mut e) = self.entries.lock()
            && let Some(w) = e.iter_mut().find(|w| w.id == id)
        {
            f(w);
        }
    }

    fn set_status(&self, id: u64, status: WatchStatus) {
        self.update(id, |w| w.status = status);
    }

    fn mark_uploaded(&self, id: u64) {
        self.update(id, |w| {
            w.status = WatchStatus::Watching;
            w.last_upload = Some(Instant::now());
            w.uploads += 1;
            w.last_error = None;
        });
    }

    fn remove(&self, id: u64) {
        if let Ok(mut e) = self.entries.lock() {
            e.retain(|w| w.id != id);
        }
    }

    /// An entry that failed before a watch task existed; dismissing it just removes it.
    fn push_failed(&self, remote_path: String, error: String) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut e) = self.entries.lock() {
            e.push(WatchEntry {
                id,
                remote_path,
                local_path: PathBuf::new(),
                status: WatchStatus::Failed(error),
                last_upload: None,
                uploads: 0,
                last_error: None,
                stop: Arc::new(AtomicBool::new(true)),
            });
        }
    }

    /// Removes an entry that has no running task (see [`Self::push_failed`]); others stop.
    pub fn dismiss(&self, id: u64) {
        let orphan = self.entries.lock().ok().is_some_and(|e| e.iter().any(|w| w.id == id && w.local_path.as_os_str().is_empty()));
        if orphan { self.remove(id) } else { self.stop(id) }
    }
}

async fn signature(path: &Path) -> Result<Signature> {
    let meta = tokio::fs::metadata(path).await.with_context(|| format!("reading {}", path.display()))?;
    Ok((meta.modified().ok(), meta.len()))
}

/// Opens a local file with its default app, without going through a shell (remote file
/// names must never be interpreted as commands).
pub fn open_with_system(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer splits unquoted arguments at commas; Windows paths cannot contain quotes.
        std::process::Command::new("explorer.exe").raw_arg(format!("\"{}\"", path.display())).spawn().context("starting Explorer")?;
    }
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(path).spawn().context("running open")?;
    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(path).spawn().context("running xdg-open")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_name_is_a_sibling() {
        assert_eq!(temp_name("/etc/nginx/nginx.conf"), "/etc/nginx/nginx.conf.tether-tmp");
        assert_eq!(crate::ssh::sftp::parent(&temp_name("/a/b")), "/a");
    }

    #[test]
    fn detects_binary_only_in_the_sniff_window() {
        assert!(looks_binary(b"PNG\0\x01"));
        assert!(!looks_binary(b"plain text\n"));
        let mut late = vec![b'a'; BINARY_SNIFF];
        late.push(0);
        assert!(!looks_binary(&late));
        assert_eq!(decode(b"ab\0c"), Err(Refusal::Binary));
        assert_eq!(decode(&[0xff, 0xfe, b'a']), Err(Refusal::NotUtf8));
    }

    #[test]
    fn decode_encode_round_trips_line_endings_and_bom() {
        for raw in [&b"a\nb\n"[..], b"a\r\nb\r\n", b"\xEF\xBB\xBFx\r\ny", b"", b"one line"] {
            let d = decode(raw).unwrap();
            assert!(!d.text.contains("\r\n"));
            assert_eq!(encode(&d.text, d.crlf, d.bom), raw);
        }
        let d = decode(b"\xEF\xBB\xBFhi\r\n").unwrap();
        assert!(d.bom && d.crlf);
        assert_eq!(d.text, "hi\n");
    }

    #[test]
    fn tracker_waits_for_the_file_to_settle() {
        let t0 = Instant::now();
        let base = (None, 10);
        let mut t = ChangeTracker::new(base);
        assert_eq!(t.observe(base, t0), None);
        assert_eq!(t.next_delay(), POLL);
        let a = (None, 11);
        assert_eq!(t.observe(a, t0), None, "first sighting only starts the debounce");
        assert_eq!(t.next_delay(), DEBOUNCE);
        assert_eq!(t.observe(a, t0 + Duration::from_millis(100)), None, "not settled yet");
        let b = (None, 12);
        assert_eq!(t.observe(b, t0 + Duration::from_millis(200)), None, "changed again: restart");
        assert_eq!(t.observe(b, t0 + Duration::from_millis(400)), None);
        assert_eq!(t.observe(b, t0 + Duration::from_millis(500)), Some(b));
        t.commit(b);
        assert_eq!(t.observe(b, t0 + Duration::from_secs(2)), None, "uploaded state is the new baseline");
        assert_eq!(t.next_delay(), POLL);
    }

    #[test]
    fn ssh_strings_are_length_prefixed() {
        let mut out = Vec::new();
        ssh_string(&mut out, "ab");
        assert_eq!(out, [0, 0, 0, 2, b'a', b'b']);
    }

    #[test]
    fn refusal_messages_are_readable() {
        assert!(Refusal::TooLarge(20 * 1024 * 1024).to_string().contains("up to"));
        let e: anyhow::Error = Refusal::Binary.into();
        assert_eq!(e.downcast_ref::<Refusal>(), Some(&Refusal::Binary));
    }
}
