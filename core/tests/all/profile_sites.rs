//! The profiler's per-line sections, through an `Env`: native time by the
//! line that made the call, the calls over a threshold, and allocations by
//! the line that made them. The bookkeeping has unit tests in
//! `src/profile.rs`; this is the VM feeding it. docs/dev/performance.md.

use std::time::Duration;

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::profile::{NativeSite, SourceLine};
use petal::stats::AllocKind;

/// A host native that takes a while: what the outlier list is for.
fn slow_measure(state: &mut PetalCxt) -> NativeResult {
    let n = state.get_int(1)?;
    std::thread::sleep(Duration::from_micros(300));
    state.push_int(n * 2);
    Ok(1)
}

fn quick(state: &mut PetalCxt) -> NativeResult {
    let n = state.get_int(1)?;
    state.push_int(n + 1);
    Ok(1)
}

fn profiled(src: &str) -> Env {
    let mut env = Env::new();
    env.register_native("slow_measure", slow_measure, NativeEffects::EFFECT);
    env.register_native("quick", quick, NativeEffects::EFFECT);
    // On before anything is loaded: the first run is covered.
    env.profile_mut().set_enabled(true);
    let src = format!("native fn slow_measure(n)\nnative fn quick(n)\n{src}");
    let pid = env.load_program(&src).expect("load");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("run");
    env
}

fn site_of<'a>(env: &Env, sites: &'a [NativeSite], name: &str, line: u32) -> &'a NativeSite {
    sites
        .iter()
        .find(|s| env.native_fn_name(s.native) == name && s.at.line == line)
        .unwrap_or_else(|| panic!("no site for {name} on line {line}: {sites:?}"))
}

#[test]
fn native_time_is_attributed_to_the_calling_line() {
    // Lines 1 and 2 are the declarations; the program starts on line 3.
    let env = profiled(
        "var total = 0
for i in range(0, 3) do
  set total += slow_measure(i)
end
for i in range(0, 50) do
  set total += quick(i)
end
print(slow_measure(get total))",
    );
    let sites = env.profile().native_sites();
    let in_loop = site_of(&env, &sites, "slow_measure", 5);
    assert_eq!(in_loop.calls, 3);
    assert!(in_loop.time >= Duration::from_micros(900), "{in_loop:?}");
    assert!(in_loop.max >= Duration::from_micros(300), "{in_loop:?}");
    // The same native from another line is another site.
    let last = site_of(&env, &sites, "slow_measure", 10);
    assert_eq!(last.calls, 1);
    assert_eq!(site_of(&env, &sites, "quick", 8).calls, 50);
    // By time, most first: the loop's three sleeps lead.
    assert_eq!(sites[0].at.line, 5);
    assert_eq!(env.native_fn_name(sites[0].native), "slow_measure");
    // The per-native totals still add up to the sites.
    let total: u64 = sites
        .iter()
        .filter(|s| env.native_fn_name(s.native) == "slow_measure")
        .map(|s| s.calls)
        .sum();
    assert_eq!(total, 4);
}

#[test]
fn the_slow_list_holds_the_sites_over_the_threshold() {
    let src = "var total = 0
for i in range(0, 4) do
  set total += slow_measure(i)
end
for i in range(0, 200) do
  set total += quick(i)
end";
    let env = profiled(src);
    let slow = env.profile().slow_native_sites();
    let s = site_of(&env, &slow, "slow_measure", 5);
    assert_eq!((s.calls, s.slow_calls), (4, 4));
    assert!(s.slow_time >= Duration::from_micros(1200), "{s:?}");
    // 200 calls of a native that adds one: at most a stray slow call or two
    // on a loaded machine, never most of them.
    let quick_slow = slow
        .iter()
        .find(|s| env.native_fn_name(s.native) == "quick")
        .map_or(0, |s| s.slow_calls);
    assert!(quick_slow < 100, "{quick_slow} of 200 quick calls counted slow");

    // A threshold the sleeping native does not reach lists nothing of it.
    let mut env = Env::new();
    env.register_native("slow_measure", slow_measure, NativeEffects::EFFECT);
    env.register_native("quick", quick, NativeEffects::EFFECT);
    env.profile_mut().slow_native = Duration::from_secs(5);
    env.profile_mut().set_enabled(true);
    assert_eq!(env.profile().slow_native, Duration::from_secs(5));
    let pid = env
        .load_program(&format!("native fn slow_measure(n)\nnative fn quick(n)\n{src}"))
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert!(env.profile().slow_native_sites().is_empty());
    assert!(!env.profile().native_sites().is_empty());
}

#[test]
fn allocations_are_attributed_to_the_line_that_made_them() {
    let env = profiled(
        "var rows = []
for i in range(0, 40) do
  set rows = append(rows, {id: i, tags: [i, i + 1]})
end
var n = 0
for i in range(0, 40) do
  set n += i * 2
end
let words = for i in range(0, 10) do
  \"w\" ++ str(i)
end
print(len(rows), n, len(words))",
    );
    let sites = env.profile().alloc_sites();
    let at = |line: u32| sites.iter().find(|s| s.at.line == line).copied();
    // Line 5: a record and a list per pass.
    let build = at(5).expect("line 5 allocates");
    assert_eq!(build.of(AllocKind::Map), 40, "{build:?}");
    assert!(build.of(AllocKind::List) >= 40, "{build:?}");
    // Line 9 is arithmetic: nothing.
    assert!(at(9).is_none(), "{:?}", at(9));
    // Line 12 builds strings.
    let strings = at(12).expect("line 12 allocates");
    assert!(strings.of(AllocKind::String) >= 10, "{strings:?}");
    // Most objects first, and the total is every site and the rest.
    assert_eq!(sites[0].at.line, 5);
    let listed: u64 = sites.iter().map(|s| s.total()).sum();
    let elsewhere: u64 = env.profile().allocs_elsewhere().iter().sum();
    assert_eq!(env.profile().total_allocs(), listed + elsewhere);
    assert!(listed > elsewhere, "most allocations have a line: {listed} vs {elsewhere}");
}

#[test]
fn a_callbacks_allocations_are_its_own_lines() {
    let env = profiled(
        "let xs = range(0, 30)
let ys = map(xs, fn(x)
  {v: x}
end)
print(len(ys))",
    );
    let sites = env.profile().alloc_sites();
    let records: u64 = sites
        .iter()
        .filter(|s| s.at.line == 5)
        .map(|s| s.of(AllocKind::Map))
        .sum();
    assert_eq!(records, 30, "{sites:?}");
    // And none of them twice, under the `map` that drove the callback.
    let all_maps: u64 = sites.iter().map(|s| s.of(AllocKind::Map)).sum::<u64>()
        + env.profile().allocs_elsewhere()[3];
    assert_eq!(all_maps, 30);
}

#[test]
fn the_report_names_files_and_lines() {
    let mut env = Env::new();
    env.register_native("slow_measure", slow_measure, NativeEffects::EFFECT);
    env.register_module(
        "hud",
        "native fn slow_measure(n)\npub fn width(n)\n  slow_measure(n)\nend\n",
    );
    env.profile_mut().set_enabled(true);
    let pid = env
        .load_program("import hud\nlet rows = for i in range(0, 3) do\n  [hud.width(i)]\nend\nprint(rows)")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let report = env.profile_report(pid, None, 10);
    assert!(report.contains("natives by calling line:"), "{report}");
    // The call is on line 3 of the module, and is named by its file.
    let line = report
        .lines()
        .find(|l| l.contains("slow_measure") && l.contains(":3"))
        .unwrap_or_else(|| panic!("no slow_measure line in:\n{report}"));
    assert!(line.contains("hud"), "{line}");
    assert!(line.contains("3 calls"), "{line}");
    assert!(report.contains("native calls of 1.0 µs or longer, by calling line"), "{report}");
    assert!(report.contains("allocations by line ("), "{report}");
    // A site keeps the line it was recorded on.
    let site = env
        .profile()
        .native_sites()
        .into_iter()
        .find(|s| env.native_fn_name(s.native) == "slow_measure")
        .unwrap();
    assert_eq!(site.at.line, 3);
    assert_eq!(
        env.profile_file_name(pid, site.at.file).as_deref().map(|f| f.contains("hud")),
        Some(true)
    );
    let _: SourceLine = site.at;
}

/// A one-file program has no file table; its lines are named by the entry
/// name the host gives (`petal run` gives the file's name), not `<entry>`.
#[test]
fn a_single_file_program_is_named_by_its_entry_name() {
    let mut env = Env::new();
    env.register_native("slow_measure", slow_measure, NativeEffects::EFFECT);
    env.profile_mut().set_enabled(true);
    env.profile_mut().entry_name = Some("prof.ptl".to_string());
    let pid = env
        .load_program("native fn slow_measure(n)
let xs = [1, 2]
print(slow_measure(len(xs)))")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let report = env.profile_report(pid, None, 10);
    assert!(!report.contains("<entry>"), "{report}");
    assert!(report.contains("prof.ptl:3"), "{report}");
    assert!(report.contains("prof.ptl:2"), "{report}");
    // The name survives a reset, as the slow-native threshold does.
    env.profile_mut().reset();
    assert_eq!(env.profile().entry_name.as_deref(), Some("prof.ptl"));
}

/// With the profiler off nothing is kept, and the heap's counters stay as
/// they were.
#[test]
fn an_unprofiled_run_records_no_sites() {
    let mut env = Env::new();
    let pid = env.load_program("let xs = [1, 2, 3]\nprint(len(xs))").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert!(env.profile().native_sites().is_empty());
    assert!(env.profile().alloc_sites().is_empty());
    assert_eq!(env.profile().total_allocs(), 0);
}
