//! The reference page lists every name the `ui` package exports.
//!
//! `docs/ui-reference.md` is where a game author looks to learn what the
//! prelude has. A widget that exists and is not on that page is a widget that
//! gets written again (see `docs/tasks/moses-debrief.md`, section 5), so the
//! page is checked against the package rather than trusted to keep up.

use std::collections::BTreeSet;
use std::path::Path;

/// Every `pub fn`, `pub native fn` and `pub let` of a module's source.
fn exports(source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in source.lines() {
        let rest = ["pub native fn ", "pub fn ", "pub let "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix));
        if let Some(rest) = rest {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

/// Whether `page` names `name` in code: `` `name` `` or `` `name(` ``, possibly
/// after other names in the same code span (`` `a()`, `b()` `` is two spans,
/// but `` `mouse_x()` `` and `` `theme` `` both count).
fn documented(page: &str, name: &str) -> bool {
    let mut from = 0;
    while let Some(at) = page[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        let before = page[..start].chars().next_back();
        let after = page[end..].chars().next();
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        if before == Some('`') && after.is_some_and(|c| !ident(c)) {
            return true;
        }
        from = end;
    }
    false
}

#[test]
fn the_reference_page_names_every_export() {
    let page_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-reference.md");
    let page = std::fs::read_to_string(&page_path)
        .unwrap_or_else(|e| panic!("{}: {e}", page_path.display()));
    let mut names = exports(petal_ui::prelude_source());
    names.extend(exports(petal_ui::input_source()));
    assert!(names.len() > 200, "the export scan found only {}", names.len());
    let missing: Vec<&String> = names.iter().filter(|n| !documented(&page, n)).collect();
    assert!(
        missing.is_empty(),
        "docs/ui-reference.md does not mention these `ui` exports: {missing:?}\n\
         Add each to the table it belongs in, as `name(args)` with one line on what it does."
    );
}

#[test]
fn the_scan_finds_what_it_should() {
    let found = exports("pub fn a(x)\nfn _b()\npub native fn c_d() -> int\npub let E = 1\n  pub fn nested()\n");
    assert_eq!(found.into_iter().collect::<Vec<_>>(), ["E", "a", "c_d"]);
    assert!(documented("| `a(x)`, `b()` |", "b"));
    assert!(documented("the `theme` record", "theme"));
    assert!(!documented("`ui_theme()`", "theme"));
    assert!(!documented("plain button text", "button"));
}
