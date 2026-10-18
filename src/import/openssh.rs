//! `~/.ssh/config` hosts (the format most clients, including Termius, can export to).

use std::path::{Path, PathBuf};

use super::{Candidate, Source};
use crate::profile::{AuthMethod, DEFAULT_PORT, Profile};

pub fn default_path() -> Option<PathBuf> {
    let p = directories::UserDirs::new()?.home_dir().join(".ssh").join("config");
    p.is_file().then_some(p)
}

/// Concrete `Host` blocks (wildcard patterns are skipped).
pub fn parse(text: &str, origin: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut current: Option<(String, Profile)> = None;
    let home = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf());
    let flush = |cur: Option<(String, Profile)>, out: &mut Vec<Candidate>| {
        if let Some((alias, mut p)) = cur {
            if p.host.is_empty() {
                p.host = alias.clone();
            }
            if !p.user.is_empty() {
                p.name = alias;
                p.group = "SSH config".into();
                out.push(Candidate { source: Source::OpenSsh, origin: origin.to_path_buf(), profile: p });
            }
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_ascii_lowercase(), v.trim().trim_start_matches('=').trim().trim_matches('"').to_string()),
            None => continue,
        };
        match key.as_str() {
            "host" | "match" => {
                flush(current.take(), &mut out);
                let alias = value.split_whitespace().next().unwrap_or("").to_string();
                if key == "host" && !alias.is_empty() && !alias.contains(['*', '?', '!']) {
                    current = Some((alias, Profile::new()));
                }
            }
            _ => {
                let Some((_, p)) = current.as_mut() else { continue };
                match key.as_str() {
                    "hostname" => p.host = value,
                    "user" => p.user = value,
                    "port" => p.port = value.parse().unwrap_or(DEFAULT_PORT),
                    "identityfile" => {
                        let path = match (value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")), &home) {
                            (Some(rest), Some(h)) => h.join(rest),
                            _ => PathBuf::from(&value),
                        };
                        p.auth = AuthMethod::Key { path };
                    }
                    _ => {}
                }
            }
        }
    }
    flush(current, &mut out);
    out
}

pub fn load(path: &Path) -> Vec<Candidate> {
    std::fs::read_to_string(path).map(|t| parse(&t, path)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_and_skips_wildcards() {
        let cfg = "\
# comment
Host *
  User nobody
Host web prod-web
  HostName 10.0.0.5
  User deploy
  Port 2222
  IdentityFile C:/keys/id_ed25519
Host nouser
  HostName x
Host=bastion
  User=root
";
        let c = parse(cfg, Path::new("config"));
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].profile.name.as_str(), c[0].profile.address().as_str()), ("web", "deploy@10.0.0.5:2222"));
        assert_eq!(c[0].profile.auth, AuthMethod::Key { path: PathBuf::from("C:/keys/id_ed25519") });
        assert_eq!(c[1].profile.host, "bastion");
        assert_eq!(c[1].profile.auth, AuthMethod::Password);
    }
}
