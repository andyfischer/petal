//! Every name the `ui` package exports carries its documentation.
//!
//! The reference for `import ui` is generated from the package's `@doc` and
//! `@module_doc` annotations (`petal doc ui`), so an export with no doc is a
//! widget a game author cannot find, and one that gets written again (see
//! `docs/tasks/moses-debrief.md`, section 5). This used to check that
//! `docs/reference.md` named every export; the prose has since moved onto the
//! declarations, and this checks the declarations instead. `petal lint
//! --rules-enable=undocumented-pub` reports the same over the package in CI
//! (`npm run lint:docs`).

use petal::api_doc::{DocOptions, Item, ModuleApi, document_source};

/// Each module of the package, documented as `petal doc` reads it.
fn modules() -> Vec<ModuleApi> {
    petal_ui::package_modules()
        .into_iter()
        .map(|(name, source)| {
            let module = document_source(
                &format!("ui/{name}"),
                &format!("{name}.ptl"),
                source,
                DocOptions::default(),
            );
            assert!(module.error.is_none(), "ui/{name}: {:?}", module.error);
            module
        })
        .collect()
}

/// The summary an item's doc gives it, when it has a doc with a body.
fn summary(item: &Item) -> Option<&str> {
    item.doc
        .as_ref()
        .and_then(|d| d.summary.as_deref())
        .filter(|s| !s.trim().is_empty())
}

#[test]
fn every_export_has_a_doc() {
    let modules = modules();
    let mut missing = Vec::new();
    let mut count = 0;
    for module in &modules {
        for item in module.items.iter().filter(|i| i.exported) {
            count += 1;
            if summary(item).is_none() {
                missing.push(format!("{}: {} (line {})", module.name, item.name, item.line));
            }
        }
    }
    assert!(count > 300, "the export scan found only {count}");
    assert!(
        missing.is_empty(),
        "these `ui` exports have no `@doc` (or one with no summary sentence):\n  {}\n\
         Give each a `@doc(\"…\")` whose first sentence says what it is; see \
         docs/language-guide.md, Documentation.",
        missing.join("\n  ")
    );
}

#[test]
fn every_module_has_a_module_doc() {
    for module in modules() {
        let summary = module.doc.as_ref().and_then(|d| d.summary.as_deref());
        assert!(
            summary.is_some_and(|s| !s.trim().is_empty()),
            "{} has no `@module_doc`",
            module.name
        );
    }
}

/// Only the package's private natives (`ui/_native`, which the facade does
/// not re-export) are hidden from the generated reference: a hidden public
/// name would be one a script can call and the reference never shows.
#[test]
fn only_internal_names_are_hidden() {
    for module in modules() {
        let hidden_module = module.doc.as_ref().is_some_and(|d| d.hidden);
        assert_eq!(hidden_module, module.name == "ui/_native", "{}", module.name);
        for item in module.items.iter().filter(|i| i.exported && i.hidden) {
            assert!(
                item.name.starts_with('_'),
                "{}: `{}` is hidden but public",
                module.name,
                item.name
            );
        }
    }
}

/// The widgets bloom replaced say so in their docs, so the reference and
/// the `deprecated` lint name the replacement.
#[test]
fn the_moved_widgets_are_deprecated() {
    let widgets = modules()
        .into_iter()
        .find(|m| m.name == "ui/widgets")
        .expect("the package has ui/widgets");
    for item in widgets.items.iter().filter(|i| i.exported) {
        let deprecated = item.doc.as_ref().and_then(|d| d.deprecated.as_deref());
        assert!(
            deprecated.is_some_and(|d| d.contains("bloom")),
            "ui/widgets: `{}` does not name its bloom replacement in `deprecated:`",
            item.name
        );
    }
}
