//! `memo(key) name = init` — the content-keyed cache declaration
//! (docs/features/memo.md).
//!
//! A memo's slot is the declaration plus a hash of the key's value, whatever
//! call path reached it, and its initializer runs only on a miss. Unlike
//! `state` it is a cache: immutable, swept when a run does not touch it, and
//! never persisted, dumped, snapshotted or carried across a reload.

use petal::env::Env;
use petal::lint::{LintOptions, RuleFilter, STATE_AS_CONSTANT, lint_source};
use petal::stack::StackKey;

/// A program and its stack, driven one frame at a time like a host does.
struct World {
    env: Env,
    pid: petal::program::ProgramId,
    sid: StackKey,
}

fn world(src: &str) -> World {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load: {e}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

impl World {
    /// Reset, run, and return what the frame printed.
    fn frame(&mut self) -> Vec<String> {
        self.env.reset_stack(self.sid).unwrap();
        self.env.run(self.sid).unwrap();
        self.env.take_output()
    }

    /// How many memo entries the stack holds.
    fn memo_entries(&self) -> usize {
        self.env
            .get_all_state(self.sid)
            .unwrap()
            .keys()
            .filter(|k| k.is_memo())
            .count()
    }

    /// How many state (non-memo) slots the stack holds.
    fn state_slots(&self) -> usize {
        self.env
            .get_all_state(self.sid)
            .unwrap()
            .keys()
            .filter(|k| !k.is_memo())
            .count()
    }
}

/// `slow(n)` prints when it runs, so a miss shows in the output.
const SLOW: &str = "fn slow(n)\n  print(\"computing\", n)\n  n * n\nend\n";

#[test]
fn the_initializer_runs_on_a_miss_and_a_hit_returns_the_cached_value() {
    let src = format!("{SLOW}memo(3) a = slow(3)\nmemo(4) b = slow(4)\nprint(a + b)\n");
    let mut w = world(&src);
    assert_eq!(w.frame(), ["computing 3", "computing 4", "25"]);
    assert_eq!(w.frame(), ["25"], "both keys hit on the second run");
    assert_eq!(w.memo_entries(), 2);
}

#[test]
fn equal_keys_share_one_entry_within_a_run() {
    let src = format!(
        "{SLOW}fn square(n)\n  memo(n) sq = slow(n)\n  sq\nend\n\
         print(square(3), square(4), square(3))\n"
    );
    let mut w = world(&src);
    assert_eq!(w.frame(), ["computing 3", "computing 4", "9 16 9"]);
    assert_eq!(w.memo_entries(), 2);
}

/// A key is hashed by content: two separately built lists or records that
/// are equal name the same entry, and a different one names another.
#[test]
fn keys_are_compared_by_content() {
    let src = format!(
        "{SLOW}fn cell(k, n)\n  memo(k) v = slow(n)\n  v\nend\n\
         print(cell([1, {{a: 2}}], 5))\n\
         print(cell([1, {{a: 2}}], 6))\n\
         print(cell([1, {{a: 3}}], 7))\n\
         print(cell({{x: 1, y: [2]}}, 8))\n\
         print(cell({{x: 1, y: [2]}}, 9))\n"
    );
    let mut w = world(&src);
    assert_eq!(
        w.frame(),
        [
            "computing 5",
            "25",
            "25",
            "computing 7",
            "49",
            "computing 8",
            "64",
            "64"
        ]
    );
}

/// The slot ignores the call path: two callsites, a loop and a recursion all
/// land on the one entry for a key. (A plain `state` would hold one slot per
/// path; this is the difference between the two.)
#[test]
fn the_call_path_does_not_split_an_entry() {
    let src = format!(
        "{SLOW}fn square(n)\n  memo(n) sq = slow(n)\n  sq\nend\n\
         fn wrap(n)\n  square(n)\nend\n\
         fn down(d)\n  if d == 0 then\n    return square(5)\n  end\n  down(d - 1)\nend\n\
         print(square(5))\nprint(wrap(5))\n\
         for i in range(3) do\n  print(square(5))\nend\n\
         print(down(3))\n"
    );
    let mut w = world(&src);
    assert_eq!(
        w.frame(),
        ["computing 5", "25", "25", "25", "25", "25", "25"]
    );
    assert_eq!(w.memo_entries(), 1);
}

/// Two declarations with the same key are two entries: the slot is the
/// declaration *and* the key.
#[test]
fn each_declaration_has_its_own_entries() {
    let src = format!("{SLOW}memo(1) a = slow(2)\nmemo(1) b = slow(3)\nprint(a, b)\n");
    let mut w = world(&src);
    assert_eq!(w.frame(), ["computing 2", "computing 3", "4 9"]);
    assert_eq!(w.memo_entries(), 2);
}

#[test]
fn an_entry_a_run_does_not_reach_is_swept() {
    let src = format!(
        "{SLOW}state var frame = 0\nset frame = get frame + 1\n\
         if get frame != 2 then\n  memo(\"k\") v = slow(7)\n  print(v)\nend\n"
    );
    let mut w = world(&src);
    assert_eq!(w.frame(), ["computing 7", "49"]);
    assert_eq!(w.memo_entries(), 1);
    // Frame 2 skips the declaration: its entry goes with the run.
    assert_eq!(w.frame(), Vec::<String>::new());
    assert_eq!(w.memo_entries(), 0);
    // So frame 3 misses again.
    assert_eq!(w.frame(), ["computing 7", "49"]);
}

/// A memoized scope that is replayed never executes the declaration, but the
/// entry it touched is retained (docs/dev/state-call-paths.md §3.3a), so it
/// survives the sweep and the scope stays replayable.
#[test]
fn a_replayed_scope_keeps_its_entries() {
    let src = "fn work(x)\n  memo(x) sq = x * x\n  let acc = 0\n  \
               for i in range(0, 20) do acc = acc + sq end\n  acc\nend\nprint(work(3))\n";
    let mut w = world(src);
    for _ in 0..4 {
        assert_eq!(w.frame(), ["180"]);
        assert_eq!(w.memo_entries(), 1, "the entry outlives every replay");
    }
    let stats = w.env.memo_stats(w.sid).unwrap();
    assert!(stats.hits >= 2, "the scope was replayed: {stats:?}");
}

#[test]
fn a_memo_binding_cannot_be_reassigned() {
    for src in [
        "memo(1) x = 2\nx = 3\n",
        "memo(1) x = 2\nx += 1\n",
        "memo(1) x = {a: 1}\nx.a = 3\n",
        "memo(1) x = [1]\nx[0] = 3\n",
        "memo(1) x = 2\nset x = 3\n",
        "memo(1) x = 2\nif true then\n  x = 5\nend\n",
        "memo(1) x = 2\nfor i in [1] do\n  x += i\nend\n",
    ] {
        let err = Env::new().load_program(src).expect_err(src).to_string();
        assert!(
            err.contains("`x` is a `memo`, which cannot be reassigned"),
            "{src}: {err}"
        );
    }
    // A new binding of the name is fine; it is not a write.
    let mut w = world("memo(1) x = 2\nlet x = 3\nprint(x)\n");
    assert_eq!(w.frame(), ["3"]);
}

#[test]
fn a_memo_needs_a_key_and_has_no_var_form() {
    let err = Env::new()
        .load_program("memo x = 2\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("`memo` needs a key"), "{err}");
    let err = Env::new()
        .load_program("memo(1) var x = 2\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("no `memo var`"), "{err}");
}

/// A cache is not state: no host-facing state surface shows or stores it.
#[test]
fn entries_are_not_state_to_the_host() {
    let src = "state count = 0\ncount += 1\nfn f(n)\n  memo(n) v = n * 10\n  v\nend\nprint(f(1) + f(2))\n";
    let mut w = world(src);
    w.frame();
    assert_eq!((w.state_slots(), w.memo_entries()), (1, 2));

    let json = w.env.get_state_json(w.pid, w.sid);
    assert_eq!(json.keys().collect::<Vec<_>>(), ["count"]);

    let saved = w.env.save_state_storage(w.pid, w.sid);
    assert_eq!(saved.saved, 1, "{}", saved.document);
    assert!(
        !saved.document.to_string().contains("memo"),
        "{}",
        saved.document
    );

    assert_eq!(w.env.declared_state_names(w.pid), ["count"]);
    assert!(
        w.env
            .set_state_from_json(w.pid, w.sid, "v", &serde_json::json!(5))
            .is_err()
    );

    let snapshot = w.env.snapshot_state(w.sid).unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot.keys().all(|k| !k.is_memo()));
}

/// A storage document round-trips the state and nothing of the cache: the
/// next process recomputes the entries.
#[test]
fn a_storage_round_trip_recomputes_the_cache() {
    let src = format!("{SLOW}state hits = 0\nhits += 1\nmemo(2) v = slow(2)\nprint(hits, v)\n");
    let mut w = world(&src);
    assert_eq!(w.frame(), ["computing 2", "1 4"]);
    let doc = w.env.save_state_storage(w.pid, w.sid).document;

    let mut next = world(&src);
    next.env
        .load_state_storage(next.pid, next.sid, &doc)
        .unwrap();
    assert_eq!(next.frame(), ["computing 2", "2 4"]);
}

/// A reload drops every memo entry and keeps the state: whether an entry is
/// still right depends on everything its initializer calls, which a reload
/// does not track.
#[test]
fn a_reload_drops_the_cache_and_keeps_the_state() {
    let old = format!("{SLOW}state n = 0\nn += 1\nmemo(3) v = slow(3)\nprint(n, v)\n")
        .replace("n * n", "n * n + 0");
    let mut w = world(&old);
    assert_eq!(w.frame(), ["computing 3", "1 9"]);
    assert_eq!(w.frame(), ["2 9"]);

    // A changed literal in the initializer's callee: patched in place, no
    // recompile. The stale entry must not survive it.
    let patched = old.replace("n * n + 0", "n * n + 1");
    let report = w.env.reload_program(w.sid, &patched, None).unwrap();
    assert_eq!(report.outcome.label(), "patched");
    assert_eq!(w.memo_entries(), 0);
    assert_eq!(
        w.frame(),
        ["computing 3", "3 10"],
        "state kept, cache recomputed"
    );

    // A structural edit: recompiled and transferred. Same rule.
    let recompiled = patched.replace("print(n, v)", "print(n, v, \"!\")");
    w.env.reload_program(w.sid, &recompiled, None).unwrap();
    assert_eq!(w.memo_entries(), 0);
    assert_eq!(w.frame(), ["computing 3", "4 10 !"]);
}

/// Swapping a stack onto a program that still declares the memo also drops
/// the entries (`Env::transfer_state`), and they count as neither preserved
/// nor dropped state.
#[test]
fn a_transfer_drops_the_cache_without_counting_it() {
    let src = "state n = 0\nn += 1\nmemo(1) v = 5\nprint(n + v)\n";
    let mut w = world(src);
    w.frame();
    assert_eq!(w.memo_entries(), 1);
    let program = w.env.compile_program(w.pid, src).unwrap();
    let result = w.env.transfer_state(w.sid, program).unwrap();
    assert_eq!((result.state_preserved, result.state_dropped), (1, 0));
    assert_eq!(w.memo_entries(), 0);
}

/// A memo's initializer describes every read of it, so the checker types the
/// binding like a `let` — where a `state` is `any` unless annotated.
#[test]
fn the_binding_is_typed_from_its_initializer() {
    let mut env = Env::new();
    let pid = env
        .load_program("memo(1) x = 5\nlet s: string = x\n")
        .unwrap();
    let warnings: Vec<String> = env
        .get_program(pid)
        .unwrap()
        .warnings
        .iter()
        .map(|w| w.message.clone())
        .collect();
    assert!(
        warnings
            .iter()
            .any(|m| m.contains("`s` declared `string` but assigned `int`")),
        "{warnings:?}"
    );
}

/// `state-as-constant` turns a top-level `state` with a static initializer
/// into a `const`; a memo is not state and is left alone.
#[test]
fn the_state_as_constant_lint_leaves_a_memo_alone() {
    let opts = LintOptions {
        rules: RuleFilter {
            include: Some(vec![STATE_AS_CONSTANT.to_string()]),
            exclude: Vec::new(),
        },
        ..LintOptions::default()
    };
    let src = "memo(1) TILES = [1, 2, 3]\nprint(TILES[0])\n";
    let out = lint_source(src, &opts).unwrap();
    assert_eq!(out.count(STATE_AS_CONSTANT), 0, "{:?}", out.findings);
    assert_eq!(out.output, src);
}

/// The IR keeps the flag, and a hand-written IR may not read or write a memo
/// slot from outside its declaration.
#[test]
fn the_ir_marks_the_declaration() {
    let mut env = Env::new();
    let pid = env.load_program("memo(7) x = 1\nprint(x)\n").unwrap();
    let program = env.get_program(pid).unwrap();
    let memo: Vec<_> = program.terms.iter().filter(|t| t.is_memo).collect();
    assert_eq!(memo.len(), 1);
    assert!(matches!(memo[0].op, petal::program::TermOp::StateInit));
    assert_eq!(memo[0].inputs.len(), 1, "the key is the one input");
    // Not state: the names a host can address leave it out.
    assert!(
        program
            .state_terms()
            .all(|(k, _)| Some(k) != memo[0].state_key)
    );
    let json = serde_json::to_value(program).unwrap();
    assert!(json.to_string().contains("\"is_memo\":true"));
}
