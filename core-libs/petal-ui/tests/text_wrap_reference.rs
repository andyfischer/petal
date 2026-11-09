//! The native `text_wrap_rows` (`src/text.rs`) against its specification in
//! Petal, `wrap_rows_reference` (`core-runtime/text-layout/src/wrap_reference.ptl`):
//! every row (`text`, `start`, `end`) must match over the shared corpus —
//! about 900 (document, style, width) cases. A change to either side that
//! changes a break fails here; change the other to match.

#[path = "support/text_wrap_corpus.rs"]
mod corpus;

use petal_ui_headless::harness::Headless;

#[test]
fn the_native_wrap_answers_what_its_petal_specification_does() {
    let mut ui = Headless::with_size(&corpus::equivalence_source(), 1200, 800)
        .unwrap_or_else(|e| panic!("compile: {e}"));
    // A proportional advance table, so a break depends on which characters
    // are in a word and not only on how many.
    petal_ui::text::bind_text_advance_table(&mut ui.env, &corpus::ratios());
    ui.frame().unwrap_or_else(|e| panic!("frame: {e}"));
    let checked = ui.state_int("checked").unwrap_or(0);
    let bad = ui.state_int("bad").unwrap_or(-1);
    assert_eq!(
        checked,
        corpus::corpus().len() as i64 * 4 * 11,
        "every (document, style, width) case ran"
    );
    assert_eq!(
        bad,
        0,
        "{bad} of {checked} cases differ; first: {}",
        ui.state_string("first").unwrap_or_default()
    );
}
