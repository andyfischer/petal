//! Clipboard access behind a small trait, so the app core and the vim layer
//! stay pure and unit-testable: tests use [`InMemoryClipboard`], the running
//! app uses [`SystemClipboard`] (the OS pasteboard via `arboard`).
//!
//! `SystemClipboard` is fault-tolerant by design — `arboard::Clipboard::new()`
//! can fail (e.g. in some headless environments), and every `set` is mirrored
//! into an in-process fallback, so Cmd+C → Cmd+V still round-trips inside the
//! app and nothing ever panics when no pasteboard is available.

/// Read/write access to a clipboard. `get` returns `None` when no text is
/// available.
pub trait Clipboard {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: &str);
}

/// Cloneable handle to one process-wide clipboard: every window's `App` gets a
/// clone of the same [`SharedClipboard`], so a yank in one window pastes in
/// another — including through [`SystemClipboard`]'s in-process fallback when
/// `arboard` can't reach an OS pasteboard. `Rc` (not `Arc`) because every
/// `App` lives on the one event-loop thread, the codebase convention (see the
/// note in `script_client.rs`).
#[derive(Clone)]
pub struct SharedClipboard {
    inner: std::rc::Rc<std::cell::RefCell<Box<dyn Clipboard>>>,
}

impl SharedClipboard {
    pub fn new(inner: Box<dyn Clipboard>) -> SharedClipboard {
        SharedClipboard {
            inner: std::rc::Rc::new(std::cell::RefCell::new(inner)),
        }
    }
}

impl Clipboard for SharedClipboard {
    fn get(&mut self) -> Option<String> {
        self.inner.borrow_mut().get()
    }

    fn set(&mut self, text: &str) {
        self.inner.borrow_mut().set(text)
    }
}

/// Plain in-process clipboard: the unit-test impl, and the fallback that
/// [`SystemClipboard`] degrades to when the OS pasteboard is unavailable.
#[derive(Default)]
pub struct InMemoryClipboard {
    text: Option<String>,
}

impl Clipboard for InMemoryClipboard {
    fn get(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn set(&mut self, text: &str) {
        self.text = Some(text.to_string());
    }
}

/// Lazily-initialized handle to the OS clipboard.
enum OsClipboard {
    Untried,
    Unavailable,
    Ready(arboard::Clipboard),
}

/// The OS clipboard, constructed lazily on first use and degrading to an
/// in-process [`InMemoryClipboard`] when `arboard` can't reach a pasteboard.
pub struct SystemClipboard {
    os: OsClipboard,
    fallback: InMemoryClipboard,
}

impl SystemClipboard {
    pub fn new() -> SystemClipboard {
        SystemClipboard {
            os: OsClipboard::Untried,
            fallback: InMemoryClipboard::default(),
        }
    }

    fn os(&mut self) -> Option<&mut arboard::Clipboard> {
        if matches!(self.os, OsClipboard::Untried) {
            self.os = match arboard::Clipboard::new() {
                Ok(c) => OsClipboard::Ready(c),
                Err(err) => {
                    eprintln!(
                        "garden: system clipboard unavailable ({err}); \
                         copy/paste stays within this session"
                    );
                    OsClipboard::Unavailable
                }
            };
        }
        match &mut self.os {
            OsClipboard::Ready(c) => Some(c),
            _ => None,
        }
    }
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        if let Some(os) = self.os() {
            if let Ok(text) = os.get_text() {
                return Some(text);
            }
        }
        // OS clipboard unavailable, empty, or holding non-text content: fall
        // back to the last text set in-process (None if we never set any).
        self.fallback.get()
    }

    fn set(&mut self, text: &str) {
        if let Some(os) = self.os() {
            let _ = os.set_text(text.to_string());
        }
        // Mirror every set so in-app copy/paste round-trips even without an
        // OS pasteboard.
        self.fallback.set(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In a multi-window process every `App` gets its own `Box<dyn Clipboard>`,
    /// so the shared handle must make clones observe one underlying clipboard:
    /// a set through any clone is visible to every other — even when the inner
    /// clipboard is the in-process fallback (no OS pasteboard involved).
    #[test]
    fn shared_clipboard_is_shared_between_clones() {
        let shared = SharedClipboard::new(Box::new(InMemoryClipboard::default()));
        let mut a = shared.clone();
        let mut b = shared.clone();

        assert_eq!(a.get(), None);
        a.set("yanked in a");
        assert_eq!(b.get().as_deref(), Some("yanked in a"));

        // And the other direction: later writes win, whichever clone wrote.
        b.set("then from b");
        assert_eq!(a.get().as_deref(), Some("then from b"));
    }

    #[test]
    fn in_memory_round_trips() {
        let mut clip = InMemoryClipboard::default();
        assert_eq!(clip.get(), None);
        clip.set("hello");
        assert_eq!(clip.get().as_deref(), Some("hello"));
        clip.set("again");
        assert_eq!(clip.get().as_deref(), Some("again"));
    }
}
