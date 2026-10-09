//! The clipboard channel behind the prelude's `clipboard_get()` and
//! `clipboard_set(text)`.
//!
//! Neither is a native. Both ride on the two host channels the core runtime
//! already has, so a host that knows nothing about clipboards still loads the
//! prelude, and `petal check` has no new name to learn:
//!
//! - **Read** is a binding, like every other input. [`bind_clipboard`] (called
//!   by [`crate::input::bind_input`], so every host does it) publishes the
//!   clipboard's text as [`SYM_CLIPBOARD`], and `clipboard_get()` is
//!   `binding(symbol("clipboard"))`. A scope that pasted is therefore
//!   validated like one that read the keyboard.
//! - **Write** is an output, like a draw command. `clipboard_set(text)` is
//!   `push_output(symbol("clipboard_write"), text)`; the host drains
//!   [`CLIPBOARD_WRITE_SIGNAL`] after the run with [`flush_clipboard`]
//!   ([`crate::frame_core::FrameCore::frame`] does). A memoized scope that
//!   copied replays the push, which writes the same text again.
//!
//! Where the text goes is the host's business. A host with a system clipboard
//! attaches a [`ClipboardProvider`] once at startup with
//! [`set_clipboard_provider`]. Without one the text is held in memory for the
//! thread, so cut, copy and paste still work between the fields of one program
//! (and in a headless test) and simply do not leave the process.
//!
//! ## When the system clipboard is read
//!
//! Asking the window system for the clipboard every frame is not free, and a
//! script only looks on a paste. So with a provider attached the binding is
//! refreshed on the frames where a paste could happen — `v` or `insert` went
//! down — which is exactly when the prelude's `text_field` reads it. The
//! in-memory clipboard is a string compare and is kept current every frame.
//! A script that calls `clipboard_get()` at some other moment sees the text as
//! of the last such frame (or its own last `clipboard_set`).

use std::cell::RefCell;

use petal::env::Env;
use petal::stack::StackKey;
use petal::value::Value;

use crate::input::InputState;

/// Binding: the clipboard's text, read by the prelude's `clipboard_get()`.
pub const SYM_CLIPBOARD: &str = "clipboard";

/// Output channel: text the script asked to put on the clipboard this frame
/// (`clipboard_set`). The last entry wins.
pub const CLIPBOARD_WRITE_SIGNAL: &str = "clipboard_write";

/// A host's clipboard: plain text out, plain text in. `get` answers the empty
/// string when the clipboard holds no text.
pub struct ClipboardProvider {
    pub get: Box<dyn FnMut() -> String>,
    pub set: Box<dyn FnMut(&str)>,
}

enum Backend {
    /// No host clipboard: the text stays in this thread.
    Memory(String),
    Host(ClipboardProvider),
}

thread_local! {
    static CLIPBOARD: RefCell<Backend> = const { RefCell::new(Backend::Memory(String::new())) };
}

/// Attach the host's clipboard for this thread, or detach it with `None`
/// (back to an empty in-memory one). Returns the provider that was attached.
pub fn set_clipboard_provider(provider: Option<ClipboardProvider>) -> Option<ClipboardProvider> {
    let next = match provider {
        Some(p) => Backend::Host(p),
        None => Backend::Memory(String::new()),
    };
    match CLIPBOARD.with(|c| std::mem::replace(&mut *c.borrow_mut(), next)) {
        Backend::Host(p) => Some(p),
        Backend::Memory(_) => None,
    }
}

/// The clipboard's text right now: the provider's answer, or the in-memory
/// text. Public so a test can look at what a script copied.
pub fn clipboard_text() -> String {
    CLIPBOARD.with(|c| match &mut *c.borrow_mut() {
        Backend::Memory(text) => text.clone(),
        Backend::Host(p) => (p.get)(),
    })
}

/// Put `text` on the clipboard: the provider's, or the in-memory one. Public
/// so a test can stage a paste.
pub fn set_clipboard_text(text: &str) {
    CLIPBOARD.with(|c| match &mut *c.borrow_mut() {
        Backend::Memory(held) => *held = text.to_string(),
        Backend::Host(p) => (p.set)(text),
    })
}

fn is_in_memory() -> bool {
    CLIPBOARD.with(|c| matches!(&*c.borrow(), Backend::Memory(_)))
}

/// Publish the clipboard's text for this frame's `clipboard_get()`.
/// [`crate::input::bind_input`] calls this; a host only calls it directly if
/// it binds its inputs some other way. See the module docs for when a
/// provider is actually asked.
pub fn bind_clipboard(env: &mut Env, input: &InputState) {
    let paste_possible = input.was_key_pressed("v") || input.was_key_pressed("insert");
    if !(paste_possible || is_in_memory()) {
        return;
    }
    // Line breaks reach a script as `\n` alone, whatever the platform that
    // filled the clipboard wrote.
    let text = clipboard_text().replace("\r\n", "\n").replace('\r', "\n");
    let sym = env.intern_symbol(SYM_CLIPBOARD);
    // Rebinding an unchanged string would still allocate one per frame.
    if let Some(Value::String(id)) = env.binding(sym)
        && env.heap().get_string(id) == text
    {
        return;
    }
    let value = Value::String(env.heap_mut().alloc_string(text));
    env.set_binding(sym, value);
}

/// Drain what the script wrote with `clipboard_set` during the last run and
/// hand the last of it to the clipboard. Returns the text written, if any.
/// Call after the run, where the host drains its draw commands.
pub fn flush_clipboard(env: &mut Env) -> Option<String> {
    let sym = env.intern_symbol(CLIPBOARD_WRITE_SIGNAL);
    let vals = env.take_output_buffer(sym);
    finish_flush(env, vals)
}

/// [`flush_clipboard`] for a host that runs several stacks in one `Env`.
pub fn flush_clipboard_for(env: &mut Env, stack_id: StackKey) -> Option<String> {
    let sym = env.intern_symbol(CLIPBOARD_WRITE_SIGNAL);
    let vals = env.take_output_buffer_for(stack_id, sym);
    finish_flush(env, vals)
}

fn finish_flush(env: &Env, vals: Vec<Value>) -> Option<String> {
    let text = vals.into_iter().rev().find_map(|v| match v {
        Value::String(id) => Some(env.heap().get_string(id).to_string()),
        _ => None,
    })?;
    set_clipboard_text(&text);
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Headless;
    use std::rc::Rc;

    const SRC: &str = "state seen = \"\"\n\
                       state n = 0\n\
                       n = n + 1\n\
                       if n == 2 then clipboard_set(\"caf\u{e9} \u{65e5}\u{672c}\") end\n\
                       seen = clipboard_get()";

    #[test]
    fn without_a_provider_the_clipboard_is_in_memory() {
        set_clipboard_provider(None);
        set_clipboard_text("staged");
        let mut ui = Headless::new(SRC).unwrap();
        ui.frame().unwrap();
        assert_eq!(ui.state_string("seen").as_deref(), Some("staged"));
        // Frame 2 writes; the write lands after the run, so frame 3 reads it.
        ui.frame().unwrap();
        assert_eq!(clipboard_text(), "caf\u{e9} \u{65e5}\u{672c}");
        ui.frame().unwrap();
        assert_eq!(ui.state_string("seen").as_deref(), Some("caf\u{e9} \u{65e5}\u{672c}"));
    }

    #[test]
    fn a_provider_is_written_to_and_read_on_a_paste_key() {
        let held = Rc::new(RefCell::new(String::from("from the host")));
        let reads = Rc::new(RefCell::new(0));
        let (g, s, r) = (held.clone(), held.clone(), reads.clone());
        set_clipboard_provider(Some(ClipboardProvider {
            get: Box::new(move || {
                *r.borrow_mut() += 1;
                g.borrow().clone()
            }),
            set: Box::new(move |t| *s.borrow_mut() = t.to_string()),
        }));
        let mut ui = Headless::new(SRC).unwrap();
        ui.frames(3).unwrap();
        assert_eq!(*held.borrow(), "caf\u{e9} \u{65e5}\u{672c}", "the write reached the host");
        assert_eq!(*reads.borrow(), 0, "no paste key, so the host was never asked");
        assert_eq!(ui.state_string("seen").as_deref(), Some(""));

        ui.key("v").unwrap();
        assert_eq!(*reads.borrow(), 1);
        assert_eq!(ui.state_string("seen").as_deref(), Some("caf\u{e9} \u{65e5}\u{672c}"));
        assert!(set_clipboard_provider(None).is_some());
    }
}
