//! The clipboard natives under a driven frame loop: the in-memory fallback,
//! and a host provider written on `clipboard_set` and read only on a paste
//! key (`src/clipboard.rs`).

use std::cell::RefCell;
use std::rc::Rc;

use petal_ui::clipboard::{ClipboardProvider, clipboard_text, set_clipboard_provider, set_clipboard_text};
use petal_ui_headless::harness::Headless;

const SRC: &str = "state seen = \"\"\n\
                   state n = 0\n\
                   n = n + 1\n\
                   if n == 2 then clipboard_set(\"caf\u{e9} \u{65e5}\u{672c}\") end\n\
                   seen = clipboard_get()";

#[test]
fn without_a_provider_the_clipboard_is_in_memory() {
    set_clipboard_provider(None);
    set_clipboard_text("staged");
    let mut ui = Headless::with_ui(SRC).unwrap();
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
    let mut ui = Headless::with_ui(SRC).unwrap();
    ui.frames(3).unwrap();
    assert_eq!(*held.borrow(), "caf\u{e9} \u{65e5}\u{672c}", "the write reached the host");
    assert_eq!(*reads.borrow(), 0, "no paste key, so the host was never asked");
    assert_eq!(ui.state_string("seen").as_deref(), Some(""));

    ui.key("v").unwrap();
    assert_eq!(*reads.borrow(), 1);
    assert_eq!(ui.state_string("seen").as_deref(), Some("caf\u{e9} \u{65e5}\u{672c}"));
    assert!(set_clipboard_provider(None).is_some());
}
