//! Behavioral tests for the `host_data(kind, arg)` pull channel, driven
//! through the headless harness with a host-attached data provider.

use std::cell::RefCell;
use std::rc::Rc;

use petal_ui::harness::Headless;
use petal_ui::host_data::HostData;

#[test]
fn host_data_without_a_provider_answers_nil() {
    // A script that pulls host data still runs under a host that attaches none.
    let src = "state got_nil = 0\n\
               if host_data(\"anything\", \"x\") == nil then got_nil = 1 end";
    let mut ui = Headless::new(src).unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.state_int("got_nil"), Some(1));
}

#[test]
fn host_data_provider_receives_kind_and_arg_and_returns_a_value_tree() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let seen = calls.clone();
    let mut ui = Headless::new(
        "state title = \"\"\n\
         state n = 0\n\
         state tags = 0\n\
         let r = host_data(\"commit\", \"abc\")\n\
         title = r.title\n\
         n = r.n\n\
         tags = len(r.taglist)",
    )
    .unwrap();
    ui.set_data_provider(Box::new(move |kind, arg| {
        seen.borrow_mut().push((kind.to_string(), arg.to_string()));
        HostData::Record(vec![
            ("title".into(), HostData::Str(format!("commit {arg}"))),
            ("n".into(), HostData::Int(42)),
            (
                "taglist".into(),
                HostData::List(vec![HostData::Str("a".into()), HostData::Str("b".into())]),
            ),
        ])
    }));
    ui.frame().unwrap();

    // The provider was called once with the script's (kind, arg).
    assert_eq!(*calls.borrow(), vec![("commit".to_string(), "abc".to_string())]);
    // The Record converted to a Petal record whose fields the script read out.
    assert_eq!(ui.state().get("title").and_then(|v| v.as_str()), Some("commit abc"));
    assert_eq!(ui.state_int("n"), Some(42));
    assert_eq!(ui.state_int("tags"), Some(2), "the nested List became a Petal list");
}

#[test]
fn host_data_answers_vary_by_arg_for_lazy_fetch() {
    // The lazy-data pattern: same kind, different arg → different answer, so a
    // script can bake an index and fetch one item's detail on demand.
    let mut ui = Headless::new(
        "state a = 0\n\
         state b = 0\n\
         state flag = 0\n\
         a = host_data(\"len\", \"x\")\n\
         b = host_data(\"len\", \"xyz\")\n\
         if host_data(\"flag\", \"\") then flag = 1 end",
    )
    .unwrap();
    ui.set_data_provider(Box::new(|kind, arg| match kind {
        "len" => HostData::Int(arg.len() as i64),
        "flag" => HostData::Bool(true),
        _ => HostData::Nil,
    }));
    ui.frame().unwrap();
    assert_eq!(ui.state_int("a"), Some(1));
    assert_eq!(ui.state_int("b"), Some(3));
    assert_eq!(ui.state_int("flag"), Some(1), "Bool converts to a truthy Petal value");
}

#[test]
fn provider_survives_across_frames_so_it_can_cache() {
    // The harness swaps the provider back out after each run, so its FnMut state
    // (a cache, a counter) persists — proving the swap round-trips ownership.
    let mut ui = Headless::new("state count = 0\ncount = host_data(\"tick\", \"\")").unwrap();
    let mut calls = 0i64;
    ui.set_data_provider(Box::new(move |_kind, _arg| {
        calls += 1;
        HostData::Int(calls)
    }));
    ui.frame().unwrap();
    assert_eq!(ui.state_int("count"), Some(1));
    ui.frame().unwrap();
    assert_eq!(ui.state_int("count"), Some(2), "the same provider instance runs each frame");
}
