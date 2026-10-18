//! Windows Explorer style "type to select": typed characters accumulate into a prefix that
//! resets after a short pause; repeating one letter cycles through the names starting with it.

/// Seconds without typing after which the next character starts a new search.
pub const TIMEOUT: f64 = 1.0;
/// Seconds the indicator takes to fade out once the search has expired.
const FADE: f64 = 0.35;

#[derive(Debug, Clone, Default)]
pub struct TypeAhead {
    buf: String,
    last: f64,
    matched: bool,
}

impl TypeAhead {
    /// Adds a typed character at time `now` (seconds). Returns false when it was ignored
    /// (control characters, or a space that would start a new search).
    pub fn push(&mut self, ch: char, now: f64) -> bool {
        if ch.is_control() {
            return false;
        }
        let expired = self.buf.is_empty() || now - self.last > TIMEOUT;
        if expired && ch.is_whitespace() {
            return false;
        }
        if expired {
            self.buf.clear();
        }
        self.buf.push(ch);
        self.last = now;
        true
    }

    pub fn buffer(&self) -> &str {
        &self.buf
    }

    pub fn matched(&self) -> bool {
        self.matched
    }

    pub fn set_matched(&mut self, matched: bool) {
        self.matched = matched;
    }

    pub fn reset(&mut self) {
        self.buf.clear();
    }

    /// Opacity (0..1) of the on-screen indicator at `now`.
    pub fn visibility(&self, now: f64) -> f32 {
        if self.buf.is_empty() {
            return 0.0;
        }
        let elapsed = now - self.last;
        if elapsed <= TIMEOUT { 1.0 } else { (1.0 - (elapsed - TIMEOUT) / FADE).clamp(0.0, 1.0) as f32 }
    }
}

/// Index of the row to select for the typed `buf`, searching from `current`.
///
/// A buffer of one repeated character ("s", "ss", …) cycles: it picks the next name after
/// `current` starting with that character. Any other buffer is a prefix search starting at
/// `current` itself, so extending a prefix keeps the selection while it still matches.
/// Matching is case-insensitive and wraps around.
pub fn find(names: &[&str], buf: &str, current: Option<usize>) -> Option<usize> {
    let needle = buf.to_lowercase();
    let first = needle.chars().next()?;
    let cycling = needle.chars().all(|c| c == first);
    let len = names.len();
    if len == 0 {
        return None;
    }
    let (prefix, start) = if cycling { (first.to_string(), current.map_or(0, |c| c + 1)) } else { (needle, current.unwrap_or(0)) };
    (0..len).map(|n| (start + n) % len).find(|&i| names[i].to_lowercase().starts_with(&prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 6] = ["bin", "sbin", "Src", "ssh", "srv", "usr"];

    #[test]
    fn single_letter_jumps_to_first_match() {
        assert_eq!(find(&NAMES, "s", None), Some(1));
        assert_eq!(find(&NAMES, "U", None), Some(5));
        assert_eq!(find(&NAMES, "x", None), None);
        assert_eq!(find(&[], "a", None), None);
        assert_eq!(find(&NAMES, "", None), None);
    }

    #[test]
    fn repeated_letter_cycles_and_wraps() {
        assert_eq!(find(&NAMES, "s", Some(1)), Some(2));
        assert_eq!(find(&NAMES, "ss", Some(2)), Some(3));
        assert_eq!(find(&NAMES, "sss", Some(4)), Some(1));
        assert_eq!(find(&NAMES, "b", Some(0)), Some(0));
    }

    #[test]
    fn prefix_search_is_inclusive_and_case_insensitive() {
        assert_eq!(find(&NAMES, "sr", Some(1)), Some(2));
        assert_eq!(find(&NAMES, "src", Some(2)), Some(2));
        assert_eq!(find(&NAMES, "SRV", Some(2)), Some(4));
        assert_eq!(find(&NAMES, "sb", Some(4)), Some(1));
    }

    #[test]
    fn buffer_resets_after_timeout() {
        let mut t = TypeAhead::default();
        assert!(t.push('s', 10.0));
        assert!(t.push('r', 10.5));
        assert_eq!(t.buffer(), "sr");
        assert!(t.push('u', 11.8));
        assert_eq!(t.buffer(), "u");
    }

    #[test]
    fn ignores_control_and_leading_space() {
        let mut t = TypeAhead::default();
        assert!(!t.push(' ', 1.0));
        assert!(!t.push('\u{8}', 1.0));
        assert!(t.push('a', 1.0));
        assert!(t.push(' ', 1.2));
        assert_eq!(t.buffer(), "a ");
    }

    #[test]
    fn indicator_fades_after_timeout() {
        let mut t = TypeAhead::default();
        assert_eq!(t.visibility(0.0), 0.0);
        t.push('a', 5.0);
        assert_eq!(t.visibility(5.5), 1.0);
        assert!(t.visibility(6.2) > 0.0 && t.visibility(6.2) < 1.0);
        assert_eq!(t.visibility(7.0), 0.0);
    }
}
