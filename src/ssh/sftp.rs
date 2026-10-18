//! SFTP client over the shared connection, built on the raw session so transfers can
//! keep many READ/WRITE requests in flight (see `transfer.rs`).

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, StatusCode};

use super::connection::Connection;

/// Fallback chunk when the server doesn't publish `limits@openssh.com`.
/// 32 KiB is the SFTP draft's guaranteed-safe size.
const SAFE_CHUNK: u32 = 32 * 1024;
/// OpenSSH accepts up to 255 KiB reads; larger buys little and costs latency.
const MAX_CHUNK: u32 = 255 * 1024;
const REQUEST_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: u64,
    pub permissions: u32,
}

pub struct Sftp {
    pub raw: RawSftpSession,
    pub read_chunk: u32,
    pub write_chunk: u32,
}

pub fn is_eof(err: &SftpError) -> bool {
    matches!(err, SftpError::Status(s) if s.status_code == StatusCode::Eof)
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((p, _)) => p.to_string(),
    }
}

impl Sftp {
    pub async fn open(conn: &Connection) -> Result<Arc<Self>> {
        let channel = conn.handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await.context("server refused SFTP subsystem")?;
        let cfg = russh_sftp::client::Config { max_packet_len: 512 * 1024, request_timeout_secs: REQUEST_TIMEOUT_SECS, ..Default::default() };
        let mut raw = RawSftpSession::new_with_config(channel.into_stream(), cfg);
        raw.init().await.context("SFTP handshake failed")?;

        let (mut read_chunk, mut write_chunk) = (SAFE_CHUNK, SAFE_CHUNK);
        if let Ok(limits) = raw.limits().await {
            read_chunk = clamp_chunk(limits.max_read_len);
            write_chunk = clamp_chunk(limits.max_write_len);
            raw.set_limits(limits.into());
        }
        Ok(Arc::new(Self { raw, read_chunk, write_chunk }))
    }

    pub async fn canonicalize(&self, path: &str) -> Result<String> {
        let name = self.raw.realpath(path).await?;
        name.files.into_iter().next().map(|f| f.filename).context("empty realpath reply")
    }

    pub async fn list(&self, path: &str) -> Result<Vec<RemoteEntry>> {
        let handle = self.raw.opendir(path).await.with_context(|| format!("opening {path}"))?.handle;
        let mut out = Vec::new();
        let result = loop {
            match self.raw.readdir(handle.as_str()).await {
                Ok(name) => out.extend(name.files.into_iter().filter(|f| f.filename != "." && f.filename != "..").map(|f| entry_from(f.filename, &f.attrs))),
                Err(e) if is_eof(&e) => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        let _ = self.raw.close(handle).await;
        result?;
        sort_entries(&mut out);
        Ok(out)
    }

    pub async fn stat(&self, path: &str) -> Result<FileAttributes> {
        Ok(self.raw.stat(path).await?.attrs)
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.raw.mkdir(path, FileAttributes::empty()).await?;
        Ok(())
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.raw.rename(from, to).await?;
        Ok(())
    }

    /// Recursive delete; directories are emptied depth-first.
    pub async fn remove(&self, path: &str, is_dir: bool) -> Result<()> {
        if !is_dir {
            self.raw.remove(path).await.with_context(|| format!("deleting {path}"))?;
            return Ok(());
        }
        let mut stack = vec![(path.to_string(), false)];
        while let Some((dir, visited)) = stack.pop() {
            if visited {
                self.raw.rmdir(dir.as_str()).await.with_context(|| format!("removing {dir}"))?;
                continue;
            }
            stack.push((dir.clone(), true));
            for e in self.list(&dir).await? {
                let child = join(&dir, &e.name);
                if e.is_dir && !e.is_link {
                    stack.push((child, false));
                } else {
                    self.raw.remove(child.as_str()).await.with_context(|| format!("deleting {child}"))?;
                }
            }
        }
        Ok(())
    }

    pub async fn ensure_dir(&self, path: &str) -> Result<()> {
        match self.raw.stat(path).await {
            Ok(a) if a.attrs.is_dir() => Ok(()),
            Ok(_) => bail!("{path} exists and is not a directory"),
            Err(_) => self.mkdir(path).await,
        }
    }
}

fn clamp_chunk(limit: u64) -> u32 {
    if limit == 0 { SAFE_CHUNK } else { limit.min(MAX_CHUNK as u64) as u32 }
}

fn entry_from(name: String, a: &FileAttributes) -> RemoteEntry {
    RemoteEntry {
        name,
        is_dir: a.is_dir(),
        is_link: a.is_symlink(),
        size: a.size.unwrap_or(0),
        mtime: a.mtime.unwrap_or(0) as u64,
        permissions: a.permissions.unwrap_or(0),
    }
}

pub fn sort_entries(entries: &mut [RemoteEntry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers() {
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("/var", "log"), "/var/log");
        assert_eq!(parent("/var/log"), "/var");
        assert_eq!(parent("/var/"), "/");
        assert_eq!(parent("/"), "/");
    }

    #[test]
    fn chunk_clamping() {
        assert_eq!(clamp_chunk(0), SAFE_CHUNK);
        assert_eq!(clamp_chunk(1 << 30), MAX_CHUNK);
        assert_eq!(clamp_chunk(65536), 65536);
    }

    #[test]
    fn dirs_sort_first_case_insensitive() {
        let mk = |n: &str, d: bool| RemoteEntry { name: n.into(), is_dir: d, is_link: false, size: 0, mtime: 0, permissions: 0 };
        let mut v = vec![mk("b.txt", false), mk("Zed", true), mk("a.txt", false), mk("app", true)];
        sort_entries(&mut v);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["app", "Zed", "a.txt", "b.txt"]);
    }
}
