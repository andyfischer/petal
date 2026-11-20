//! `unique_id` (a native drawing from a counter the VM keeps) and `stable_id`
//! (the std helper that keeps one in a `state`): fresh ids never repeat, and a
//! place's stable id is the same on every run.

use std::collections::HashSet;

use petal::env::Env;

/// Run `src` once per frame on one stack and return each run's output.
fn runs(src: &str, times: usize) -> Vec<Vec<String>> {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{e}"));
    let sid = env.create_stack(pid).unwrap();
    (0..times)
        .map(|i| {
            if i > 0 {
                env.reset_stack(sid).unwrap();
            }
            env.run(sid).unwrap_or_else(|e| panic!("{e}"));
            env.take_output()
        })
        .collect()
}

#[test]
fn unique_id_counts_up_across_runs() {
    let out = runs("print(unique_id(), unique_id())", 3);
    assert_eq!(out, vec![vec!["1 2"], vec!["3 4"], vec!["5 6"]]);
}

#[test]
fn stable_id_is_one_id_per_place_and_the_same_every_run() {
    let src = r#"
fn widget()
  stable_id()
print(stable_id())
print(stable_id())
print(widget())
print(widget())
for i in range(3) do
  print(stable_id())
"#;
    let out = runs(src, 3);
    assert_eq!(out[0], out[1]);
    assert_eq!(out[1], out[2]);
    let ids: HashSet<&String> = out[0].iter().collect();
    assert_eq!(ids.len(), 7, "every call site, call path and iteration has its own: {:?}", out[0]);
}

#[test]
fn a_keyed_loop_keeps_each_item_s_id_when_the_items_reorder() {
    let src = r#"
state var frame = 0
set frame = frame + 1
let items = if frame == 1 then [7, 3] else [3, 7, 5] end
for k in items key k do
  print("{k}={stable_id()}")
"#;
    let out = runs(src, 2);
    let id = |run: &[String], k: &str| run.iter().find(|l| l.starts_with(&format!("{k}="))).cloned().unwrap();
    assert_eq!(id(&out[0], "7"), id(&out[1], "7"));
    assert_eq!(id(&out[0], "3"), id(&out[1], "3"));
    // A new item gets an id no other item has.
    let ids: HashSet<&str> = out[1].iter().map(|l| l.split('=').nth(1).unwrap()).collect();
    assert_eq!(ids.len(), 3, "{out:?}");
}

#[test]
fn stable_ids_never_collide_with_fresh_ones() {
    let src = r#"
let a = stable_id()
let b = unique_id()
print(a != b)
"#;
    let out = runs(src, 3);
    assert!(out.iter().all(|o| o == &vec!["true".to_string()]), "{out:?}");
}

#[test]
fn a_place_that_stops_running_gets_a_new_id_when_it_runs_again() {
    let src = r#"
state var frame = 0
set frame = frame + 1
if frame != 2 then
  print(stable_id())
"#;
    let out = runs(src, 3);
    assert_eq!(out[1], Vec::<String>::new());
    assert_ne!(out[0], out[2]);
}
