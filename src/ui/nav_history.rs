//! Browser-style back/forward stacks for one side of an SFTP pane.

/// Entries kept in each direction.
pub const NAV_CAP: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavHistory<T> {
    back: Vec<T>,
    forward: Vec<T>,
}

impl<T> Default for NavHistory<T> {
    fn default() -> Self {
        Self { back: Vec::new(), forward: Vec::new() }
    }
}

impl<T: Clone + PartialEq> NavHistory<T> {
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// A new navigation away from `from`: remembered for Back, and Forward is cleared.
    /// Going to the same place (a refresh) changes nothing.
    pub fn visit(&mut self, from: T, to: &T) {
        if &from == to {
            return;
        }
        if self.back.last() != Some(&from) {
            self.back.push(from);
            if self.back.len() > NAV_CAP {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// Steps back from `current`; returns where to go.
    pub fn back(&mut self, current: T) -> Option<T> {
        let target = self.back.pop()?;
        self.forward.push(current);
        Some(target)
    }

    /// Steps forward from `current`; returns where to go.
    pub fn forward(&mut self, current: T) -> Option<T> {
        let target = self.forward.pop()?;
        self.back.push(current);
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward_round_trip() {
        let mut h = NavHistory::default();
        h.visit("/", &"/home");
        h.visit("/home", &"/home/u");
        assert!(h.can_back() && !h.can_forward());
        assert_eq!(h.back("/home/u"), Some("/home"));
        assert_eq!(h.back("/home"), Some("/"));
        assert_eq!(h.back("/"), None);
        assert_eq!(h.forward("/"), Some("/home"));
        assert_eq!(h.forward("/home"), Some("/home/u"));
        assert_eq!(h.forward("/home/u"), None);
    }

    #[test]
    fn new_visit_clears_forward() {
        let mut h = NavHistory::default();
        h.visit("/", &"/a");
        assert_eq!(h.back("/a"), Some("/"));
        assert!(h.can_forward());
        h.visit("/", &"/b");
        assert!(!h.can_forward());
        assert_eq!(h.back("/b"), Some("/"));
    }

    #[test]
    fn refresh_is_not_a_visit_and_cap_holds() {
        let mut h = NavHistory::default();
        h.visit("/a", &"/a");
        assert!(!h.can_back());
        let mut h = NavHistory::default();
        for i in 0..(NAV_CAP + 10) {
            h.visit(i, &(i + 1));
        }
        let mut n = 0;
        let mut cur = NAV_CAP + 10;
        while let Some(prev) = h.back(cur) {
            cur = prev;
            n += 1;
        }
        assert_eq!(n, NAV_CAP);
    }
}
