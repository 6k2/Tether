//! Pure shell-style path completion used by the SFTP path fields.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub is_dir: bool,
}

/// How paths look on one side of the SFTP view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathStyle {
    /// Separator appended after a completed directory.
    pub sep: char,
    pub case_insensitive: bool,
}

pub const REMOTE: PathStyle = PathStyle { sep: '/', case_insensitive: false };
pub const LOCAL: PathStyle = PathStyle { sep: std::path::MAIN_SEPARATOR, case_insensitive: cfg!(windows) };

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    NoMatch,
    /// Exactly one candidate: the full completed text.
    Unique(String),
    /// Several: the text extended to their common prefix, plus the candidates.
    Ambiguous {
        text: String,
        candidates: Vec<Candidate>,
    },
}

fn is_sep(c: char, style: PathStyle) -> bool {
    c == '/' || c == style.sep
}

/// Splits `text` after its last separator: (`directory part incl. separator`, `partial name`).
pub fn split(text: &str, style: PathStyle) -> (&str, &str) {
    match text.char_indices().rev().find(|&(_, c)| is_sep(c, style)) {
        Some((i, c)) => text.split_at(i + c.len_utf8()),
        None => ("", text),
    }
}

/// Directory to list for a directory part; keeps roots like `/` and `C:\` intact.
pub fn listing_dir(dir: &str, style: PathStyle) -> String {
    let trimmed = dir.trim_end_matches(|c| is_sep(c, style));
    if trimmed.is_empty() || trimmed.ends_with(':') { dir.to_string() } else { trimmed.to_string() }
}

/// `dir` with exactly one trailing separator (empty stays empty).
pub fn with_trailing_sep(dir: &str, style: PathStyle) -> String {
    if dir.is_empty() || dir.ends_with(|c| is_sep(c, style)) { dir.to_string() } else { format!("{dir}{}", style.sep) }
}

fn is_absolute(path: &str, style: PathStyle) -> bool {
    let b = path.as_bytes();
    let drive = b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic();
    path.starts_with(|c| is_sep(c, style)) || (style.sep == '\\' && drive)
}

/// Makes a typed directory part absolute against `cwd` (empty means `cwd` itself).
pub fn absolutize(cwd: &str, dir: &str, style: PathStyle) -> String {
    if is_absolute(dir, style) { dir.to_string() } else { format!("{}{dir}", with_trailing_sep(cwd, style)) }
}

fn starts_with(name: &str, prefix: &str, ci: bool) -> bool {
    if ci { name.to_lowercase().starts_with(&prefix.to_lowercase()) } else { name.starts_with(prefix) }
}

/// Candidates whose name starts with `prefix`, directories first, then by name.
pub fn matching(candidates: &[Candidate], prefix: &str, style: PathStyle) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = candidates.iter().filter(|c| starts_with(&c.name, prefix, style.case_insensitive)).cloned().collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Longest common prefix (by chars); the spelling of the first name is kept.
pub fn common_prefix<S: AsRef<str>>(names: &[S], ci: bool) -> String {
    let Some(first) = names.first().map(AsRef::as_ref) else { return String::new() };
    let eq = |a: char, b: char| if ci { a.to_lowercase().eq(b.to_lowercase()) } else { a == b };
    let mut len = first.chars().count();
    for other in &names[1..] {
        len = first.chars().zip(other.as_ref().chars()).take(len).take_while(|&(a, b)| eq(a, b)).count();
    }
    first.chars().take(len).collect()
}

/// Text after choosing `c` inside `dir`; directories get a trailing separator.
pub fn completion_text(dir: &str, c: &Candidate, style: PathStyle) -> String {
    let mut s = format!("{dir}{}", c.name);
    if c.is_dir {
        s.push(style.sep);
    }
    s
}

pub fn complete(dir: &str, prefix: &str, candidates: &[Candidate], style: PathStyle) -> Outcome {
    let found = matching(candidates, prefix, style);
    match found.len() {
        0 => Outcome::NoMatch,
        1 => Outcome::Unique(completion_text(dir, &found[0], style)),
        _ => {
            let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
            let mut common = common_prefix(&names, style.case_insensitive);
            // Never shorten what the user typed (case-insensitive prefixes can differ in case only).
            if common.chars().count() < prefix.chars().count() {
                common = prefix.to_string();
            }
            Outcome::Ambiguous { text: format!("{dir}{common}"), candidates: found }
        }
    }
}

/// Lexically cleans an absolute remote path: no empty or `.` segments, `..` applied, no
/// trailing slash (except the root). The server's canonical form still wins afterwards.
pub fn normalize_remote(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Next index when cycling with repeated Tab (Shift+Tab goes backwards).
pub fn next_index(current: Option<usize>, len: usize, backwards: bool) -> usize {
    match (current, backwards) {
        _ if len == 0 => 0,
        (None, false) => 0,
        (None, true) => len - 1,
        (Some(i), false) => (i + 1) % len,
        (Some(i), true) => (i + len - 1) % len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, is_dir: bool) -> Candidate {
        Candidate { name: name.into(), is_dir }
    }

    const WIN: PathStyle = PathStyle { sep: '\\', case_insensitive: true };

    #[test]
    fn splits_at_last_separator() {
        assert_eq!(split("/home/us", REMOTE), ("/home/", "us"));
        assert_eq!(split("/", REMOTE), ("/", ""));
        assert_eq!(split("docs", REMOTE), ("", "docs"));
        assert_eq!(split(r"C:\Users\Bo", WIN), (r"C:\Users\", "Bo"));
        assert_eq!(split("C:/Users/Bo", WIN), ("C:/Users/", "Bo"));
    }

    #[test]
    fn listing_dir_keeps_roots() {
        assert_eq!(listing_dir("/home/", REMOTE), "/home");
        assert_eq!(listing_dir("/", REMOTE), "/");
        assert_eq!(listing_dir(r"C:\", WIN), r"C:\");
        assert_eq!(listing_dir(r"C:\Users\", WIN), r"C:\Users");
        assert_eq!(with_trailing_sep("/home", REMOTE), "/home/");
        assert_eq!(with_trailing_sep("/", REMOTE), "/");
    }

    #[test]
    fn relative_dirs_resolve_against_cwd() {
        assert_eq!(absolutize("/home/u", "", REMOTE), "/home/u/");
        assert_eq!(absolutize("/home/u", "src/", REMOTE), "/home/u/src/");
        assert_eq!(absolutize("/home/u", "/etc/", REMOTE), "/etc/");
        assert_eq!(absolutize("/", "etc/", REMOTE), "/etc/");
        assert_eq!(absolutize(r"C:\Users", r"D:\", WIN), r"D:\");
        assert_eq!(absolutize(r"C:\Users", r"Bob\", WIN), r"C:\Users\Bob\");
    }

    #[test]
    fn common_prefix_handles_case() {
        assert_eq!(common_prefix(&["logs", "local", "lost+found"], false), "lo");
        assert_eq!(common_prefix(&["Documents", "downloads"], true), "Do");
        assert_eq!(common_prefix(&["Documents", "downloads"], false), "");
        assert_eq!(common_prefix(&["only"], false), "only");
        assert_eq!(common_prefix::<&str>(&[], false), "");
    }

    #[test]
    fn unique_match_completes_dir_with_separator() {
        let cands = [c("etc", true), c("home", true), c("hosts.txt", false)];
        assert_eq!(complete("/", "e", &cands, REMOTE), Outcome::Unique("/etc/".into()));
        assert_eq!(complete("/", "hos", &cands, REMOTE), Outcome::Unique("/hosts.txt".into()));
        assert_eq!(complete("/", "x", &cands, REMOTE), Outcome::NoMatch);
    }

    #[test]
    fn ambiguous_match_extends_to_common_prefix_dirs_first() {
        let cands = [c("logrotate.conf", false), c("local", true), c("log", true)];
        let Outcome::Ambiguous { text, candidates } = complete("/var/", "l", &cands, REMOTE) else { panic!() };
        assert_eq!(text, "/var/lo");
        let names: Vec<_> = candidates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["local", "log", "logrotate.conf"]);
    }

    #[test]
    fn case_insensitive_local_matching() {
        let cands = [c("Users", true), c("Program Files", true)];
        assert_eq!(complete(r"C:\", "us", &cands, WIN), Outcome::Unique(r"C:\Users\".into()));
        assert_eq!(complete(r"C:\", "u", &cands, REMOTE), Outcome::NoMatch);
    }

    #[test]
    fn remote_paths_normalise_lexically() {
        assert_eq!(normalize_remote("/home/"), "/home");
        assert_eq!(normalize_remote("//home//u/./x/.."), "/home/u");
        assert_eq!(normalize_remote("/.."), "/");
        assert_eq!(normalize_remote("/"), "/");
    }

    #[test]
    fn tab_cycles_both_ways() {
        assert_eq!(next_index(None, 3, false), 0);
        assert_eq!(next_index(Some(2), 3, false), 0);
        assert_eq!(next_index(None, 3, true), 2);
        assert_eq!(next_index(Some(0), 3, true), 2);
        assert_eq!(next_index(Some(0), 0, false), 0);
    }
}
