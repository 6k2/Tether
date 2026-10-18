//! Host profiles (Termius "hosts") persisted as JSON; secrets live in the OS keyring.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const KEYRING_SERVICE: &str = "tether-ssh";
pub const DEFAULT_PORT: u16 = 22;
/// Bitvise defaults to 512 KiB; benchmarks at 20 ms RTT peak around 2-4 MiB here.
pub const DEFAULT_PIPELINE_KB: u32 = 4096;
pub const MIN_PIPELINE_KB: u32 = 16;
pub const MAX_PIPELINE_KB: u32 = 16384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMethod {
    Password,
    Key {
        path: PathBuf,
    },
    /// A key from the built-in key manager (`keystore`).
    Managed {
        key: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    /// Legacy group name (and import hint). Migrated into `folder` on load.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// Sidebar folder (`folders.rs`); `None` = top level.
    #[serde(default)]
    pub folder: Option<Uuid>,
    /// Archived hosts are tucked into the collapsed "Archived" section.
    #[serde(default)]
    pub archived: bool,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    /// Accent hue (0..360) used for the host dot and tab marker.
    #[serde(default)]
    pub hue: f32,
    #[serde(default = "default_pipeline")]
    pub pipeline_kb: u32,
    #[serde(default)]
    pub remote_start_dir: String,
    #[serde(default)]
    pub last_used: u64,
}

fn default_pipeline() -> u32 {
    DEFAULT_PIPELINE_KB
}

impl Profile {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            group: String::new(),
            folder: None,
            archived: false,
            host: String::new(),
            port: DEFAULT_PORT,
            user: String::new(),
            auth: AuthMethod::Password,
            hue: (Uuid::new_v4().as_u128() % 360) as f32,
            pipeline_kb: DEFAULT_PIPELINE_KB,
            remote_start_dir: String::new(),
            last_used: 0,
        }
    }

    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() { &self.host } else { &self.name }
    }

    pub fn address(&self) -> String {
        if self.port == DEFAULT_PORT { format!("{}@{}", self.user, self.host) } else { format!("{}@{}:{}", self.user, self.host, self.port) }
    }

    /// Same server, user and credentials: an open connection made for `other` can be
    /// reused for `self` (labels, folders, colours or `last_used` don't matter).
    pub fn same_endpoint(&self, other: &Profile) -> bool {
        self.host == other.host && self.port == other.port && self.user == other.user && self.auth == other.auth
    }

    /// Keyring entry holding this host's secret: its password / key passphrase, or the
    /// passphrase of the managed key it uses (shared by every host using that key).
    pub fn secret_id(&self) -> Uuid {
        match &self.auth {
            AuthMethod::Managed { key } => *key,
            _ => self.id,
        }
    }

    /// Validation used by the editor before saving.
    pub fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("Host is required".into());
        }
        if self.user.trim().is_empty() {
            return Err("Username is required".into());
        }
        if self.port == 0 {
            return Err("Port must be between 1 and 65535".into());
        }
        if let AuthMethod::Key { path } = &self.auth
            && path.as_os_str().is_empty()
        {
            return Err("Choose a private key file".into());
        }
        if let AuthMethod::Managed { key } = &self.auth
            && key.is_nil()
        {
            return Err("Choose a key".into());
        }
        if !(MIN_PIPELINE_KB..=MAX_PIPELINE_KB).contains(&self.pipeline_kb) {
            return Err(format!("Pipeline must be {MIN_PIPELINE_KB}..{MAX_PIPELINE_KB} KB"));
        }
        Ok(())
    }

    /// Parse a quick-connect string: `user@host[:port]`.
    pub fn parse_quick(input: &str) -> Option<Self> {
        let input = input.trim().trim_start_matches("ssh://");
        let (user, rest) = input.split_once('@')?;
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()?),
            None => (rest, DEFAULT_PORT),
        };
        if user.is_empty() || host.is_empty() || port == 0 {
            return None;
        }
        let mut p = Self::new();
        p.user = user.to_string();
        p.host = host.to_string();
        p.port = port;
        Some(p)
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::new()
    }
}

/// Repository for profiles stored at `<config>/profiles.json`.
#[derive(Debug, Default)]
pub struct ProfileStore {
    path: PathBuf,
    pub profiles: Vec<Profile>,
}

impl ProfileStore {
    pub fn load(path: PathBuf) -> Result<Self> {
        let profiles = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("profiles file is corrupt: {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).context("reading profiles"),
        };
        Ok(Self { path, profiles })
    }

    pub fn save(&self) -> Result<()> {
        write_json_atomic(&self.path, &self.profiles)
    }

    pub fn get(&self, id: Uuid) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn upsert(&mut self, profile: Profile) {
        match self.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(slot) => *slot = profile,
            None => self.profiles.push(profile),
        }
    }

    pub fn remove(&mut self, id: Uuid) {
        self.profiles.retain(|p| p.id != id);
        let _ = secret::delete(id);
    }

    pub fn touch(&mut self, id: Uuid) {
        if let Some(p) = self.profiles.iter_mut().find(|p| p.id == id) {
            p.last_used = unix_now();
        }
    }

    /// Group names in display order (empty group last).
    pub fn groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = self.profiles.iter().map(|p| p.group.clone()).collect();
        groups.sort_by(|a, b| a.is_empty().cmp(&b.is_empty()).then(a.cmp(b)));
        groups.dedup();
        groups
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Write via temp file + rename so a crash never leaves a half-written config.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context("creating config dir")?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?).context("writing temp file")?;
    std::fs::rename(&tmp, path).context("replacing config file")?;
    Ok(())
}

/// Password / key passphrase storage in the OS credential vault.
pub mod secret {
    use super::*;

    fn entry(id: Uuid) -> keyring::Result<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, &id.to_string())
    }

    pub fn get(id: Uuid) -> Option<String> {
        entry(id).ok()?.get_password().ok()
    }

    pub fn set(id: Uuid, value: &str) -> Result<()> {
        entry(id)?.set_password(value).context("storing secret in keyring")
    }

    pub fn delete(id: Uuid) -> Result<()> {
        match entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("deleting secret"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quick_connect_with_port() {
        let p = Profile::parse_quick("root@10.0.0.5:2222").unwrap();
        assert_eq!((p.user.as_str(), p.host.as_str(), p.port), ("root", "10.0.0.5", 2222));
    }

    #[test]
    fn parses_quick_connect_default_port() {
        let p = Profile::parse_quick("ssh://ana@example.com").unwrap();
        assert_eq!(p.port, DEFAULT_PORT);
        assert_eq!(p.address(), "ana@example.com");
    }

    #[test]
    fn rejects_bad_quick_connect() {
        assert!(Profile::parse_quick("example.com").is_none());
        assert!(Profile::parse_quick("@host").is_none());
        assert!(Profile::parse_quick("u@host:0").is_none());
        assert!(Profile::parse_quick("u@host:abc").is_none());
    }

    #[test]
    fn validate_catches_missing_fields() {
        let mut p = Profile::new();
        assert!(p.validate().is_err());
        p.host = "h".into();
        p.user = "u".into();
        assert!(p.validate().is_ok());
        p.auth = AuthMethod::Key { path: PathBuf::new() };
        assert!(p.validate().is_err());
    }

    #[test]
    fn store_roundtrip_and_groups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let mut store = ProfileStore::load(path.clone()).unwrap();
        let mut a = Profile::new();
        a.host = "a".into();
        a.group = "prod".into();
        let b = Profile::new();
        store.upsert(a.clone());
        store.upsert(b);
        store.save().unwrap();
        let loaded = ProfileStore::load(path).unwrap();
        assert_eq!(loaded.profiles.len(), 2);
        assert_eq!(loaded.get(a.id).unwrap().group, "prod");
        assert_eq!(loaded.groups(), vec!["prod".to_string(), String::new()]);
    }
}
