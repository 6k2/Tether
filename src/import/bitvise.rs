//! Bitvise SSH Client profiles (`.tlp`, "Tunnelier" format).
//!
//! Layout observed across Tunnelier 9.17–9.66: a big-endian `u32` length-prefixed
//! version string, a block of fixed fields, then `u32 len + host`, `u32 port`,
//! 11 bytes of flags, and `u32 len + username`. Passwords and keypairs live in
//! Bitvise's own encrypted stores and are not imported.

use std::path::{Path, PathBuf};

use super::{Candidate, Source};
use crate::profile::Profile;

const MAGIC: &[u8] = b"Tunnelier";
const USER_GAP: usize = 11;
const MAX_HOST: usize = 253;
const MAX_USER: usize = 128;

fn u32_at(b: &[u8], pos: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(pos..pos + 4)?.try_into().ok()?) as usize)
}

fn is_host_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b".-:[]_".contains(&c)
}

pub fn is_profile(bytes: &[u8]) -> bool {
    bytes.get(4..4 + MAGIC.len()) == Some(MAGIC)
}

/// Extracts (host, port, user) from a profile file's bytes.
pub fn parse(bytes: &[u8]) -> Option<(String, u16, String)> {
    if !is_profile(bytes) {
        return None;
    }
    let mut pos = 4 + u32_at(bytes, 0)?;
    while pos + 8 < bytes.len() {
        let n = u32_at(bytes, pos)?;
        let candidate = bytes.get(pos + 4..pos + 4 + n);
        if let Some(host) = candidate.filter(|h| (1..=MAX_HOST).contains(&n) && h.iter().all(|&c| is_host_char(c)) && h.iter().any(u8::is_ascii_alphanumeric)) {
            let port_at = pos + 4 + n;
            let port = u32_at(bytes, port_at)?;
            let user_at = port_at + 4 + USER_GAP;
            if let (Ok(port @ 1..=65535), Some(ulen)) = (u16::try_from(port), u32_at(bytes, user_at))
                && let Some(user) =
                    bytes.get(user_at + 4..user_at + 4 + ulen).filter(|u| (1..=MAX_USER).contains(&ulen) && u.iter().all(|&c| (33..127).contains(&c)))
            {
                return Some((String::from_utf8_lossy(host).into_owned(), port, String::from_utf8_lossy(user).into_owned()));
            }
        }
        pos += 1;
    }
    None
}

pub fn load(path: &Path) -> Option<Candidate> {
    let bytes = std::fs::read(path).ok()?;
    let (host, port, user) = parse(&bytes)?;
    let mut profile = Profile::new();
    profile.name = path.file_stem()?.to_string_lossy().into_owned();
    profile.group = "Bitvise".into();
    profile.host = host;
    profile.port = port;
    profile.user = user;
    Some(Candidate { source: Source::Bitvise, origin: path.to_path_buf(), profile })
}

/// Folders where Bitvise profiles usually live: the folder of the last used profile
/// (from the registry), `~/.ssh`, Documents and Desktop.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(last) = last_used_profile()
        && let Some(parent) = last.parent()
    {
        dirs.push(parent.to_path_buf());
    }
    if let Some(u) = directories::UserDirs::new() {
        dirs.push(u.home_dir().join(".ssh"));
        dirs.extend(u.document_dir().map(Path::to_path_buf));
        dirs.extend(u.desktop_dir().map(Path::to_path_buf));
    }
    dirs.dedup();
    dirs
}

/// Profiles found in `search_dirs` (files with the Tunnelier header, any extension).
pub fn discover() -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for dir in search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let small = entry.metadata().is_ok_and(|m| m.is_file() && m.len() < 1024 * 1024);
            if small && !out.iter().any(|c| c.origin == path) {
                out.extend(load(&path));
            }
        }
    }
    out.sort_by_key(|a| a.profile.name.to_lowercase());
    out
}

#[cfg(windows)]
fn last_used_profile() -> Option<PathBuf> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Bitvise\BvSshClient").ok()?;
    let raw = key.get_raw_value("LastUsedProfile").ok()?;
    let units: Vec<u16> = raw.bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(PathBuf::from(String::from_utf16_lossy(&units)))
}

#[cfg(not(windows))]
fn last_used_profile() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp(b: &mut Vec<u8>, s: &[u8]) {
        b.extend((s.len() as u32).to_be_bytes());
        b.extend_from_slice(s);
    }

    fn sample(host: &str, port: u32, user: &str) -> Vec<u8> {
        let mut b = Vec::new();
        lp(&mut b, b"Tunnelier 9.66");
        for v in [7u32, 2, 0x8e9, 0x1d77, 0x1f, 0x9de, 0xaf0, 0, 1] {
            b.extend(v.to_be_bytes());
        }
        lp(&mut b, host.as_bytes());
        b.extend(port.to_be_bytes());
        b.extend([0u8; USER_GAP]);
        lp(&mut b, user.as_bytes());
        b.extend(b"\x02\x00\x00\x00\x00\x01 trailing data xterm");
        b
    }

    #[test]
    fn parses_host_port_user() {
        assert_eq!(parse(&sample("104.243.46.168", 428, "root")), Some(("104.243.46.168".into(), 428, "root".into())));
        assert_eq!(parse(&sample("vps.example.com", 22, "deploy")), Some(("vps.example.com".into(), 22, "deploy".into())));
    }

    #[test]
    fn rejects_other_files() {
        assert_eq!(parse(b"-----BEGIN OPENSSH PRIVATE KEY-----"), None);
        assert_eq!(parse(&[]), None);
        let mut bad_port = sample("h", 0, "u");
        bad_port.truncate(bad_port.len() - 30);
        assert_eq!(parse(&bad_port), None);
    }

    #[test]
    fn load_names_profile_after_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-01.tlp");
        std::fs::write(&path, sample("10.0.0.9", 22, "root")).unwrap();
        let c = load(&path).unwrap();
        assert_eq!((c.profile.name.as_str(), c.profile.group.as_str(), c.profile.address().as_str()), ("web-01", "Bitvise", "root@10.0.0.9"));
    }
}
