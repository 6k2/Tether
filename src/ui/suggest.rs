//! Pure ranking behind the as-you-type suggestions: remote/local paths, plain words
//! (usernames, hosts) and quick-connect `user@host:port` strings.

use std::collections::HashSet;

use super::path_complete::PathStyle;
use crate::profile::{DEFAULT_PORT, Profile};

/// Suggestions shown in a dropdown.
pub const SUGGEST_LIMIT: usize = 8;
const DAY_SECS: f64 = 86_400.0;
/// Fuzzy (subsequence) matching starts at this many typed characters.
const FUZZY_MIN: usize = 2;

/// One known path with how often / how recently it was visited.
#[derive(Debug, Clone, Copy)]
pub struct PathEntry<'a> {
    pub path: &'a str,
    pub visits: u32,
    /// Unix seconds (0 = never visited).
    pub last_visit: u64,
}

/// Lower-cased (when the style is case-insensitive) with `/` mapped to the style's
/// separator; one char in, one char out, so char counts stay aligned with the original.
fn norm(s: &str, style: PathStyle) -> String {
    let mapped = s.chars().map(|c| if c == '/' { style.sep } else { c });
    if style.case_insensitive { mapped.map(|c| c.to_lowercase().next().unwrap_or(c)).collect() } else { mapped.collect() }
}

fn last_segment(s: &str, style: PathStyle) -> &str {
    s.rsplit(['/', style.sep]).next().unwrap_or(s)
}

fn depth(s: &str, style: PathStyle) -> usize {
    s.split(['/', style.sep]).filter(|p| !p.is_empty()).count()
}

/// `needle` appears in `hay` in order, not necessarily contiguous.
pub fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

/// Visit frequency and recency (decaying over about a week).
fn frecency(visits: u32, last_visit: u64, now: u64) -> f64 {
    let freq = f64::from(visits).ln_1p() * 10.0;
    let recency = if last_visit == 0 {
        0.0
    } else {
        let age_days = now.saturating_sub(last_visit) as f64 / DAY_SECS;
        10.0 * (-age_days / 7.0).exp()
    };
    freq + recency
}

/// Ranks `entries` for an absolute `query`: prefix of the full path first, then prefix of
/// the last segment, then fuzzy subsequence; within a tier by frecency, then shallower.
pub fn rank_paths<'a>(query: &str, entries: impl IntoIterator<Item = PathEntry<'a>>, style: PathStyle, now: u64, limit: usize) -> Vec<String> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let q = norm(query, style);
    let q_exact = q.trim_end_matches(style.sep).to_string();
    let q_last = last_segment(&q, style).to_lowercase();
    let q_depth = depth(query, style);
    let mut seen = HashSet::new();
    let mut scored: Vec<(u8, f64, usize, String)> = Vec::new();
    for e in entries {
        let p = norm(e.path, style);
        if p.trim_end_matches(style.sep) == q_exact || !seen.insert(p.clone()) {
            continue;
        }
        let p_last = last_segment(&p, style).to_lowercase();
        let tier = if p.starts_with(&q) {
            0
        } else if !q_last.is_empty() && p_last.starts_with(&q_last) {
            1
        } else if q_last.chars().count() >= FUZZY_MIN && is_subsequence(&q_last, &p.to_lowercase()) {
            2
        } else {
            continue;
        };
        let extra_depth = depth(e.path, style).saturating_sub(q_depth) as f64;
        let score = frecency(e.visits, e.last_visit, now) - 1.5 * extra_depth;
        scored.push((tier, score, e.path.chars().count(), e.path.to_string()));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)).then_with(|| a.3.cmp(&b.3)));
    scored.into_iter().take(limit).map(|s| s.3).collect()
}

/// The dim text to show after `query` when `suggestion` extends it, or `None`.
pub fn path_ghost(query: &str, suggestion: &str, style: PathStyle) -> Option<String> {
    let (q, s) = (norm(query, style), norm(suggestion, style));
    (s.starts_with(&q) && s.chars().count() > q.chars().count()).then(|| suggestion.chars().skip(query.chars().count()).collect())
}

/// Case-insensitive ghost for plain text fields.
pub fn word_ghost(query: &str, suggestion: &str) -> Option<String> {
    // A style whose separator is `/` maps nothing, leaving plain case-insensitive matching.
    let ci = PathStyle { sep: '/', case_insensitive: true };
    if query.is_empty() { None } else { path_ghost(query, suggestion, ci) }
}

/// Ranks `words` (already ordered by popularity) for `query`, case-insensitively:
/// prefix, then substring, then subsequence. The exact word itself is left out.
pub fn rank_words(query: &str, words: &[String], limit: usize) -> Vec<String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut tiers: [Vec<String>; 3] = Default::default();
    for w in words {
        let lw = w.to_lowercase();
        if lw == q || !seen.insert(lw.clone()) {
            continue;
        }
        let tier = if lw.starts_with(&q) {
            0
        } else if lw.contains(&q) {
            1
        } else if q.chars().count() >= FUZZY_MIN && is_subsequence(&q, &lw) {
            2
        } else {
            continue;
        };
        tiers[tier].push(w.clone());
    }
    tiers.into_iter().flatten().take(limit).collect()
}

/// `user@host[:port]` for a profile (port only when not the default).
pub fn quick_string(user: &str, host: &str, port: u16) -> String {
    if port == DEFAULT_PORT { format!("{user}@{host}") } else { format!("{user}@{host}:{port}") }
}

/// Quick-connect strings of the saved profiles, most recently used first.
pub fn quick_entries(profiles: &[Profile]) -> Vec<String> {
    let mut sorted: Vec<&Profile> = profiles.iter().filter(|p| !p.user.is_empty() && !p.host.is_empty()).collect();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.last_used));
    let mut seen = HashSet::new();
    sorted.into_iter().map(|p| quick_string(&p.user, &p.host, p.port)).filter(|s| seen.insert(s.clone())).collect()
}

/// Ranks quick-connect strings. Once a user is typed (`admin@my`), every known host is
/// also offered with that user, so new user/host combinations complete too.
pub fn rank_quick(query: &str, entries: &[String], limit: usize) -> Vec<String> {
    let query = query.trim();
    let mut pool = entries.to_vec();
    if let Some((user, _)) = query.split_once('@')
        && !user.is_empty()
    {
        pool.extend(entries.iter().filter_map(|e| e.split_once('@')).map(|(_, host)| format!("{user}@{host}")));
    }
    // Prefix matches only make sense on the full string; plain substring on hosts too.
    rank_words(query, &pool, limit)
}

/// Usernames and hosts used by saved profiles, most used (then most recent) first.
pub fn profile_words(profiles: &[Profile]) -> (Vec<String>, Vec<String>) {
    let by_use = |pick: fn(&Profile) -> &str| {
        let mut stats: Vec<(String, usize, u64)> = Vec::new();
        for p in profiles {
            let v = pick(p).trim();
            if v.is_empty() {
                continue;
            }
            match stats.iter_mut().find(|s| s.0 == v) {
                Some(s) => {
                    s.1 += 1;
                    s.2 = s.2.max(p.last_used);
                }
                None => stats.push((v.to_string(), 1, p.last_used)),
            }
        }
        stats.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then_with(|| a.0.cmp(&b.0)));
        stats.into_iter().map(|s| s.0).collect::<Vec<_>>()
    };
    (by_use(|p| &p.user), by_use(|p| &p.host))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::path_complete::REMOTE;

    const WIN: PathStyle = PathStyle { sep: '\\', case_insensitive: true };
    const NOW: u64 = 1_000_000;

    fn e(path: &str, visits: u32, last_visit: u64) -> PathEntry<'_> {
        PathEntry { path, visits, last_visit }
    }

    #[test]
    fn prefix_beats_segment_beats_fuzzy() {
        let entries = [e("/var/www/home", 0, 0), e("/home", 0, 0), e("/opt/hxoxmxe", 0, 0)];
        let r = rank_paths("/ho", entries, REMOTE, NOW, 8);
        assert_eq!(r, ["/home", "/var/www/home", "/opt/hxoxmxe"]);
        let r = rank_paths("/srv/ho", entries, REMOTE, NOW, 8);
        assert_eq!(r, ["/home", "/var/www/home", "/opt/hxoxmxe"]);
    }

    #[test]
    fn frecency_orders_within_tier_and_shallow_wins_ties() {
        let entries = [e("/home/user/deep", 0, 0), e("/home/user", 0, 0), e("/home/admin", 9, NOW)];
        let r = rank_paths("/home/", entries, REMOTE, NOW, 8);
        assert_eq!(r, ["/home/admin", "/home/user", "/home/user/deep"]);
    }

    #[test]
    fn exact_and_duplicate_paths_are_skipped() {
        let entries = [e("/home", 3, NOW), e("/home/", 1, 0), e("/home/u", 0, 0), e("/home/u", 0, 0)];
        assert_eq!(rank_paths("/home", entries, REMOTE, NOW, 8), ["/home/u"]);
        assert!(rank_paths("", entries, REMOTE, NOW, 8).is_empty());
    }

    #[test]
    fn remote_is_case_sensitive_local_windows_is_not() {
        // Full-path prefix is case-sensitive remotely; the segment tier is not.
        assert_eq!(path_ghost("/Ho", "/home", REMOTE), None);
        assert_eq!(rank_paths("/Ho", [e("/home", 0, 0)], REMOTE, NOW, 8), ["/home"]);
        let r = rank_paths("c:/users/bo", [e(r"C:\Users\Bob", 0, 0)], WIN, NOW, 8);
        assert_eq!(r, [r"C:\Users\Bob"]);
    }

    #[test]
    fn ghost_extends_query_only_on_prefix() {
        assert_eq!(path_ghost("/ho", "/home", REMOTE), Some("me".into()));
        assert_eq!(path_ghost("/home", "/home", REMOTE), None);
        assert_eq!(path_ghost("/x", "/home", REMOTE), None);
        assert_eq!(path_ghost(r"c:\us", r"C:\Users", WIN), Some("ers".into()));
        assert_eq!(word_ghost("RO", "root"), Some("ot".into()));
        assert_eq!(word_ghost("", "root"), None);
    }

    #[test]
    fn words_rank_prefix_then_substring_then_fuzzy() {
        let words: Vec<String> = ["ubuntu", "root", "deploy", "rodrigo", "Root"].map(String::from).into();
        assert_eq!(rank_words("ro", &words, 8), ["root", "rodrigo"]);
        assert_eq!(rank_words("ub", &words, 8), ["ubuntu"]);
        assert_eq!(rank_words("dpy", &words, 8), ["deploy"]);
        assert_eq!(rank_words("ot", &words, 8), ["root"]);
        assert!(rank_words("root", &words, 8).is_empty(), "exact word is not suggested");
    }

    #[test]
    fn quick_offers_new_user_host_combinations() {
        let entries: Vec<String> = ["root@web.lan", "deploy@db.lan:2222"].map(String::from).into();
        assert_eq!(rank_quick("ro", &entries, 8), ["root@web.lan"]);
        assert_eq!(rank_quick("admin@d", &entries, 8), ["admin@db.lan:2222"]);
        assert_eq!(rank_quick("db", &entries, 8), ["deploy@db.lan:2222"]);
        assert_eq!(quick_string("u", "h", 22), "u@h");
        assert_eq!(quick_string("u", "h", 2200), "u@h:2200");
    }

    #[test]
    fn profile_words_by_frequency() {
        let mk = |user: &str, host: &str, last_used: u64| {
            let mut p = Profile::new();
            p.user = user.into();
            p.host = host.into();
            p.last_used = last_used;
            p
        };
        let ps = [mk("root", "a", 1), mk("deploy", "b", 9), mk("root", "c", 2)];
        let (users, hosts) = profile_words(&ps);
        assert_eq!(users, ["root", "deploy"]);
        assert_eq!(hosts, ["b", "c", "a"]);
        assert_eq!(quick_entries(&ps), ["deploy@b", "root@c", "root@a"]);
    }
}
