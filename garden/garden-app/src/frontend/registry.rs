//! Bookkeeping for the windowed frontend's per-window states (MWI Phase 1).
//!
//! [`WindowRegistry`] owns the [`WindowState`](super::window)s of the windowed
//! frontend, keyed by winit [`WindowId`], and tracks which window is focused.
//! It is generic over the state type so the focus/removal decisions stay
//! unit-testable without a renderer or GPU.

use std::collections::HashMap;

use winit::window::WindowId;

/// Per-window states keyed by [`WindowId`], plus which window has focus.
///
/// Invariant: `focused` is `Some` exactly while the registry is non-empty —
/// an insert focuses the new window, and removing the focused window falls
/// back to an arbitrary remaining one.
pub struct WindowRegistry<T> {
    windows: HashMap<WindowId, T>,
    focused: Option<WindowId>,
}

impl<T> WindowRegistry<T> {
    pub fn new() -> WindowRegistry<T> {
        WindowRegistry {
            windows: HashMap::new(),
            focused: None,
        }
    }

    /// Register a window's state. The new window takes focus (a freshly
    /// created window comes up frontmost on every platform we target).
    pub fn insert(&mut self, id: WindowId, state: T) {
        self.windows.insert(id, state);
        self.focused = Some(id);
    }

    /// Move focus to `id`. Unknown ids are ignored (a stale focus event can
    /// race a window's removal).
    pub fn set_focused(&mut self, id: WindowId) {
        if self.windows.contains_key(&id) {
            self.focused = Some(id);
        }
    }

    pub fn focused_id(&self) -> Option<WindowId> {
        self.focused
    }

    /// Remove and return a window's state. If it was focused, focus falls
    /// back to an arbitrary remaining window (or clears when none remain).
    pub fn remove(&mut self, id: WindowId) -> Option<T> {
        let state = self.windows.remove(&id)?;
        if self.focused == Some(id) {
            self.focused = self.windows.keys().next().copied();
        }
        Some(state)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut T> {
        self.windows.get_mut(&id)
    }

    pub fn focused_mut(&mut self) -> Option<&mut T> {
        let id = self.focused?;
        self.windows.get_mut(&id)
    }

    /// Visit every window's state (arbitrary order).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (WindowId, &mut T)> {
        self.windows.iter_mut().map(|(id, state)| (*id, state))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }
}

impl<T> Default for WindowRegistry<T> {
    fn default() -> WindowRegistry<T> {
        WindowRegistry::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> WindowId {
        WindowId::from(n)
    }

    #[test]
    fn insert_focuses_the_new_window() {
        let mut reg: WindowRegistry<i32> = WindowRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.focused_id(), None);

        reg.insert(id(1), 10);
        assert_eq!(reg.focused_id(), Some(id(1)));
        assert_eq!(reg.len(), 1);

        // A second insert moves focus to the newly inserted window.
        reg.insert(id(2), 20);
        assert_eq!(reg.focused_id(), Some(id(2)));
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn set_focused_switches_between_known_ids_and_ignores_unknown() {
        let mut reg: WindowRegistry<i32> = WindowRegistry::new();
        reg.insert(id(1), 10);
        reg.insert(id(2), 20);
        assert_eq!(reg.focused_id(), Some(id(2)));

        reg.set_focused(id(1));
        assert_eq!(reg.focused_id(), Some(id(1)));

        // Unknown id: no-op, focus unchanged.
        reg.set_focused(id(99));
        assert_eq!(reg.focused_id(), Some(id(1)));

        reg.set_focused(id(2));
        assert_eq!(reg.focused_id(), Some(id(2)));
    }

    #[test]
    fn remove_of_non_focused_window_keeps_focus() {
        let mut reg: WindowRegistry<i32> = WindowRegistry::new();
        reg.insert(id(1), 10);
        reg.insert(id(2), 20);
        assert_eq!(reg.focused_id(), Some(id(2)));

        assert_eq!(reg.remove(id(1)), Some(10));
        assert_eq!(reg.focused_id(), Some(id(2)));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn remove_of_focused_window_falls_back_to_a_remaining_window() {
        let mut reg: WindowRegistry<i32> = WindowRegistry::new();
        reg.insert(id(1), 10);
        reg.insert(id(2), 20);
        reg.insert(id(3), 30);
        assert_eq!(reg.focused_id(), Some(id(3)));

        assert_eq!(reg.remove(id(3)), Some(30));

        // Focus fell back to some still-present window.
        let focused = reg.focused_id();
        assert!(focused.is_some());
        let focused = focused.unwrap();
        assert!(
            focused == id(1) || focused == id(2),
            "focus must land on a remaining window"
        );
        assert!(reg.get_mut(focused).is_some());
    }

    #[test]
    fn remove_of_last_window_clears_focus_and_empties_registry() {
        let mut reg: WindowRegistry<i32> = WindowRegistry::new();
        reg.insert(id(1), 10);

        assert_eq!(reg.remove(id(1)), Some(10));
        assert_eq!(reg.focused_id(), None);
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);

        // Removing an id that isn't there returns None.
        assert_eq!(reg.remove(id(1)), None);
    }

    #[test]
    fn get_mut_and_focused_mut_return_the_right_state() {
        let mut reg: WindowRegistry<String> = WindowRegistry::new();
        reg.insert(id(1), "one".to_string());
        reg.insert(id(2), "two".to_string());

        assert_eq!(reg.get_mut(id(1)).map(|s| s.as_str()), Some("one"));
        assert_eq!(reg.get_mut(id(2)).map(|s| s.as_str()), Some("two"));
        assert_eq!(reg.get_mut(id(99)), None);

        // focused_mut tracks the focused id (window 2 was inserted last).
        assert_eq!(reg.focused_mut().map(|s| s.as_str()), Some("two"));
        reg.set_focused(id(1));
        assert_eq!(reg.focused_mut().map(|s| s.as_str()), Some("one"));

        // Mutations through the returned reference stick.
        reg.focused_mut().unwrap().push_str("!");
        assert_eq!(reg.get_mut(id(1)).map(|s| s.as_str()), Some("one!"));

        let mut empty: WindowRegistry<String> = WindowRegistry::new();
        assert_eq!(empty.focused_mut(), None);
    }

    #[test]
    fn iter_mut_visits_every_window_exactly_once() {
        let mut reg: WindowRegistry<i32> = WindowRegistry::new();
        reg.insert(id(1), 10);
        reg.insert(id(2), 20);
        reg.insert(id(3), 30);

        let mut seen: Vec<(WindowId, i32)> = Vec::new();
        for (wid, state) in reg.iter_mut() {
            seen.push((wid, *state));
            *state += 1; // prove the references are mutable
        }
        assert_eq!(seen.len(), 3);
        let mut ids: Vec<WindowId> = seen.iter().map(|(wid, _)| *wid).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids, vec![id(1), id(2), id(3)]);
        for (wid, val) in &seen {
            let expected = *val;
            assert_eq!(reg.get_mut(*wid), Some(&mut (expected + 1)));
        }
    }
}
