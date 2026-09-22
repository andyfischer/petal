//! `petal check` cannot see this crate's registrations (the core CLI does not
//! depend on petal-ui), so it carries static lists of the natives each host
//! adds (`petal::typecheck::globals`). These tests hold the lists to the real
//! registrations: a native added here without being listed there would make
//! `check` report every call to it as an unknown function.

use std::collections::BTreeSet;

use petal::env::Env;
use petal::typecheck::globals::{GARDEN_NATIVES, PETAL_UI_NATIVES};

/// The names `register` adds to a bare `Env`, beyond the core builtins.
fn added_by(register: impl FnOnce(&mut Env)) -> BTreeSet<String> {
    let core: BTreeSet<String> = Env::new().native_fn_names().into_iter().collect();
    let mut env = Env::new();
    register(&mut env);
    env.native_fn_names()
        .into_iter()
        .filter(|n| !core.contains(n))
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn petal_ui_list_matches_register_all() {
    let registered = added_by(petal_ui::register_all);
    let listed = set(PETAL_UI_NATIVES);
    let unlisted: Vec<_> = registered.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&registered).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "petal::typecheck::globals::PETAL_UI_NATIVES is out of date.\n  \
         registered but not listed: {unlisted:?}\n  listed but not registered: {stale:?}"
    );
}

/// The panel stubs stand in for Garden's panel natives, so every one of them
/// must be in Garden's list.
#[test]
fn panel_stubs_are_in_the_garden_list() {
    let registered = added_by(petal_ui::panel_stubs::register_panel_stubs);
    let listed = set(GARDEN_NATIVES);
    let unlisted: Vec<_> = registered.difference(&listed).collect();
    assert!(
        unlisted.is_empty(),
        "petal::typecheck::globals::GARDEN_NATIVES lacks panel stubs: {unlisted:?}"
    );
}
