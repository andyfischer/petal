//! Tests of petal-ui widgets inside a real Garden panel app, which need
//! Garden's panel stubs to load (`import garden`) and so live here rather
//! than in `core-libs/petal-ui/tests/`.

use petal_ui::clipboard::{clipboard_text, set_clipboard_provider};
use petal_ui::input::{InputEvent, Modifiers};
use petal_ui_headless::harness::Headless;

const CMD: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, cmd: true };

/// Press `key` with `mods` held, then release the modifiers.
fn chord(ui: &mut Headless, mods: Modifiers, key: &str) {
    ui.event(InputEvent::Modifiers(mods));
    ui.key(key).unwrap();
    ui.event(InputEvent::Modifiers(Modifiers::default()));
}

/// The email client's search box is `text_field_update` under the app's own
/// focus string and its own paint: the widget's selection, clipboard and undo
/// work there with no editing code in the app.
#[test]
fn the_email_client_search_field_selects_copies_and_undoes() {
    set_clipboard_provider(None);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = root.join("examples/productivity/email-client/app.ptl");
    let mut ui = petal_ui_corpus::panel(&app, 1268, 778, &[root.join("core-runtime")])
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    let q = |ui: &Headless| ui.state_string("q").unwrap();
    ui.frame().unwrap();
    ui.key("slash").unwrap();
    ui.text("caf\u{e9} from:m\u{e9}l").unwrap();
    assert_eq!(q(&ui), "caf\u{e9} from:m\u{e9}l");

    chord(&mut ui, Modifiers { shift: true, alt: true, ..Default::default() }, "left");
    chord(&mut ui, CMD, "x");
    assert_eq!(q(&ui), "caf\u{e9} ");
    assert_eq!(clipboard_text(), "from:m\u{e9}l");
    chord(&mut ui, CMD, "z");
    assert_eq!(q(&ui), "caf\u{e9} from:m\u{e9}l");

    // The search text starts at x = 278; a double click on "café" selects it,
    // and typing replaces it.
    ui.frames(40).unwrap();
    ui.click(278 + 12, 34).unwrap();
    ui.click(278 + 12, 34).unwrap();
    ui.text("t\u{e9}").unwrap();
    assert_eq!(q(&ui), "t\u{e9} from:m\u{e9}l");
    chord(&mut ui, CMD, "a");
    chord(&mut ui, CMD, "v");
    assert_eq!(q(&ui), "from:m\u{e9}l");
}

