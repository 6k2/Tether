//! Trust-on-first-use host key store (`<config>/known_hosts.json`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::profile::write_json_atomic;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyVerdict {
    Trusted,
    /// First time we see this host; the key was recorded.
    Learned,
    /// Stored fingerprint differs — possible MITM.
    Mismatch {
        expected: String,
    },
}

#[derive(Debug, Default)]
pub struct KnownHosts {
    path: PathBuf,
    entries: BTreeMap<String, String>,
    /// Set when the file exists but can't be parsed. Treating it as empty would
    /// silently re-trust every host (including an attacker's key), so we refuse instead.
    corrupt: Option<String>,
}

impl KnownHosts {
    pub fn load(path: PathBuf) -> Self {
        let parsed = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.to_string()),
        };
        match parsed {
            Ok(entries) => Self { path, entries, corrupt: None },
            Err(e) => Self { path, entries: BTreeMap::new(), corrupt: Some(e) },
        }
    }

    fn key(host: &str, port: u16) -> String {
        format!("{}:{port}", host.to_ascii_lowercase())
    }

    pub fn verify(&mut self, host: &str, port: u16, fingerprint: &str) -> Result<HostKeyVerdict> {
        if let Some(e) = &self.corrupt {
            bail!("{} is unreadable ({e}); fix or delete it to continue", self.path.display());
        }
        let key = Self::key(host, port);
        match self.entries.get(&key) {
            Some(known) if known == fingerprint => Ok(HostKeyVerdict::Trusted),
            Some(known) => Ok(HostKeyVerdict::Mismatch { expected: known.clone() }),
            None => {
                self.entries.insert(key, fingerprint.to_string());
                write_json_atomic(&self.path, &self.entries)?;
                Ok(HostKeyVerdict::Learned)
            }
        }
    }

    /// Explicitly replace a stored key (user accepted a changed key).
    pub fn forget(&mut self, host: &str, port: u16) -> Result<()> {
        if self.corrupt.is_some() {
            bail!("{} is unreadable; fix or delete it first", self.path.display());
        }
        self.entries.remove(&Self::key(host, port));
        write_json_atomic(&self.path, &self.entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tofu_then_trust_then_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kh.json");
        let mut kh = KnownHosts::load(path.clone());
        assert_eq!(kh.verify("Host", 22, "SHA256:a").unwrap(), HostKeyVerdict::Learned);
        let mut kh = KnownHosts::load(path);
        assert_eq!(kh.verify("host", 22, "SHA256:a").unwrap(), HostKeyVerdict::Trusted);
        assert_eq!(kh.verify("host", 22, "SHA256:b").unwrap(), HostKeyVerdict::Mismatch { expected: "SHA256:a".into() });
        kh.forget("host", 22).unwrap();
        assert_eq!(kh.verify("host", 22, "SHA256:b").unwrap(), HostKeyVerdict::Learned);
    }

    #[test]
    fn corrupt_file_refuses_instead_of_retrusting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kh.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let mut kh = KnownHosts::load(path.clone());
        assert!(kh.verify("h", 22, "x").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json", "file must not be overwritten");
    }

    #[test]
    fn ports_are_distinct_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let mut kh = KnownHosts::load(dir.path().join("kh.json"));
        kh.verify("h", 22, "x").unwrap();
        assert_eq!(kh.verify("h", 2222, "y").unwrap(), HostKeyVerdict::Learned);
    }
}
