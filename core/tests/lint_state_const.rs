//! `petal lint`'s `state-as-constant` rule (core/src/lint/state_const.rs): a
//! plain top-level `state` that nothing assigns and whose initializer is
//! static becomes a `const`. One test per thing the rule must leave alone.

use petal::lint::{LintOptions, LintOutcome, RuleFilter, STATE_AS_CONSTANT, lint_source};

fn only_this_rule() -> LintOptions {
    LintOptions {
        rules: RuleFilter {
            include: Some(vec![STATE_AS_CONSTANT.to_string()]),
            exclude: Vec::new(),
        },
        ..LintOptions::default()
    }
}

fn lint(src: &str) -> LintOutcome {
    lint_source(src, &only_this_rule()).expect("lint_source should succeed")
}

/// `src` is fixed to `expected`, which lint then leaves alone.
#[track_caller]
fn assert_fixes(src: &str, expected: &str) {
    let out = lint(src);
    assert_eq!(out.output, expected);
    assert!(out.count(STATE_AS_CONSTANT) > 0);
    let again = lint(expected);
    assert_eq!(again.output, expected, "not a fixed point");
    assert_eq!(again.findings.len(), 0, "{:?}", again.findings);
}

/// The rule has nothing to say about `src`.
#[track_caller]
fn assert_left_alone(src: &str) {
    let out = lint(src);
    assert_eq!(out.count(STATE_AS_CONSTANT), 0, "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn a_constant_table_parked_in_state_becomes_a_const() {
    assert_fixes(
        "\
state TILES = [
  {name: \"grass\", cost: 1},
  {name: \"road\", cost: 0},
]
print(TILES[0].name)
",
        "\
const TILES = [
  {name: \"grass\", cost: 1},
  {name: \"road\", cost: 0},
]
print(TILES[0].name)
",
    );
}

#[test]
fn the_finding_names_the_declaration_and_the_fix() {
    let out = lint("let a = 1\nstate LIMIT: int = 10\nprint(a + LIMIT)\n");
    let [f] = out.findings.as_slice() else {
        panic!("one finding: {:?}", out.findings);
    };
    assert_eq!((f.rule, f.line, f.column), (STATE_AS_CONSTANT, 2, 1));
    assert!(f.message.starts_with("`state LIMIT` is never assigned"), "{}", f.message);
    assert!(f.message.contains("`const`"), "{}", f.message);
    assert_eq!(out.output, "let a = 1\nconst LIMIT: int = 10\nprint(a + LIMIT)\n");
}

#[test]
fn it_becomes_a_const_and_not_a_let() {
    let out = lint("state A = [1, 2]\nprint(A)\n");
    assert!(out.output.starts_with("const A"), "{}", out.output);
}

#[test]
fn a_table_built_by_a_static_pure_function_counts() {
    assert_fixes(
        "\
fn row(name, cost)
  {name: name, cost: cost}
end
state ROWS = [row(\"a\", 1), row(\"b\", 2)]
state COSTS = map(ROWS, fn(r) -> r.cost)
print(COSTS)
",
        "\
fn row(name, cost)
  {name: name, cost: cost}
end
const ROWS = [row(\"a\", 1), row(\"b\", 2)]
const COSTS = map(ROWS, fn(r) -> r.cost)
print(COSTS)
",
    );
}

#[test]
fn a_chain_of_constants_is_converted_in_one_run() {
    // `SORTED` is static only once `RAW` is no longer state: the rule
    // repeats until nothing more changes, so one `lint --fix` is enough.
    let out = lint("state RAW = [3, 1, 2]\nstate SORTED = sort(RAW)\nprint(SORTED)\n");
    assert_eq!(out.output, "const RAW = [3, 1, 2]\nconst SORTED = sort(RAW)\nprint(SORTED)\n");
    assert_eq!(out.count(STATE_AS_CONSTANT), 2);
    let lines: Vec<usize> = out.findings.iter().map(|f| f.line).collect();
    assert_eq!(lines, [1, 2], "each reported where it is written");
}

#[test]
fn an_assigned_state_is_left_alone() {
    // Rebound at the top level, written by index, by field, by a compound
    // assignment, and from inside a function.
    assert_left_alone("state count = 0\ncount = count + 1\nprint(count)\n");
    assert_left_alone("state xs = [1, 2]\nxs[0] = 5\nprint(xs)\n");
    assert_left_alone("state r = {a: 1}\nr.a = 2\nprint(r)\n");
    assert_left_alone("state n = 0\nn += 1\nprint(n)\n");
    assert_left_alone("state hits = 0\nfn hit()\n  hits = hits + 1\nend\nhit()\nprint(hits)\n");
    assert_left_alone(
        "state items = []\nfn add(x)\n  items = append(items, x)\nend\nadd(1)\nprint(items)\n",
    );
}

#[test]
fn a_keyed_state_is_left_alone() {
    assert_left_alone("let id = 3\nstate(id) open = false\nprint(open)\n");
}

#[test]
fn a_state_var_is_left_alone() {
    assert_left_alone("state var total = 0\nprint(get total)\n");
}

#[test]
fn a_state_inside_a_function_is_left_alone() {
    assert_left_alone("fn widget()\n  state TABLE = [1, 2, 3]\n  TABLE\nend\nprint(widget())\n");
    assert_left_alone("for i in range(2) do\n  state T = [1, 2]\n  print(T)\nend\n");
}

#[test]
fn a_dynamic_initializer_is_left_alone() {
    assert_left_alone("state started = time()\nprint(started)\n");
    assert_left_alone("state seed = random()\nprint(seed)\n");
    assert_left_alone("state n = 0\nn = n + 1\nstate snapshot = n\nprint(snapshot)\n");
    assert_left_alone("var v = 1\nstate first = get v\nprint(first)\n");
    assert_left_alone(
        "fn noisy()\n  print(\"building\")\n  [1]\nend\nstate T = noisy()\nprint(T)\n",
    );
    // A closure is not a value a constant can hold.
    assert_left_alone("state handlers = [fn(x) -> x]\nprint(len(handlers))\n");
}

#[test]
fn a_host_native_in_the_initializer_is_left_alone() {
    // Lint has no host: the native is dynamic here, so the table is not
    // offered. (Its host may classify it static; lint never claims more.)
    assert_left_alone("native fn tile(n)\nstate TILES = [tile(1), tile(2)]\nprint(TILES)\n");
}

/// An export is read-only to the modules that import it, so the declaring
/// file's own writes are all the writes there are.
#[test]
fn a_pub_state_becomes_a_pub_const() {
    assert_fixes(
        "pub state LIMIT = 10\npub  state ROWS = [[1, LIMIT], [2, LIMIT * 2]]\nprint(LIMIT, ROWS)\n",
        "pub const LIMIT = 10\npub  const ROWS = [[1, LIMIT], [2, LIMIT * 2]]\nprint(LIMIT, ROWS)\n",
    );
    assert_left_alone("pub state count = 0\ncount = count + 1\nprint(count)\n");
    assert_left_alone("pub state seen = []\nfn note(x)\n  seen = append(seen, x)\nend\nnote(1)\n");
}

#[test]
fn the_rule_can_be_silenced_on_a_line_and_excluded() {
    assert_left_alone(
        "// petal-lint-ignore state-as-constant -- the host writes this by name\n\
         state volume = 5\nprint(volume)\n",
    );
    let src = "state volume = 5\nprint(volume)\n";
    let excluded = LintOptions {
        rules: RuleFilter {
            include: None,
            exclude: vec![STATE_AS_CONSTANT.to_string()],
        },
        ..LintOptions::default()
    };
    assert_eq!(lint_source(src, &excluded).unwrap().findings.len(), 0);
    // One silenced and one not: only the other is rewritten.
    let out = lint(
        "state volume = 5 // petal-lint-ignore state-as-constant\nstate LIMIT = 9\nprint(volume, LIMIT)\n",
    );
    assert_eq!(out.count(STATE_AS_CONSTANT), 1);
    assert!(out.output.contains("state volume = 5"), "{}", out.output);
    assert!(out.output.contains("const LIMIT = 9"), "{}", out.output);
}

#[test]
fn the_rewrite_is_a_const_the_compiler_accepts() {
    // Whatever the rule produces must compile without a `const` error: it
    // checks its own rewrite, and drops a declaration that would not be
    // constant where it is written.
    for src in [
        "state A = [1, 2]\nstate B = len(A)\nprint(A, B)\n",
        "state T = build()\nfn build() [1, 2, 3] end\nprint(T)\n",
        "state LATE = helper(2)\nstate n = 0\nn = n + 1\nfn helper(x) x + n end\nprint(LATE)\n",
    ] {
        let out = lint(src);
        let mut env = petal::env::Env::new();
        env.set_echo(false);
        let pid = env.load_program(&out.output).unwrap_or_else(|e| panic!("{src}\n-- {e}"));
        let errors: Vec<_> = env
            .get_program(pid)
            .unwrap()
            .warnings
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        assert!(errors.is_empty(), "{src}\n=> {}\n{errors:?}", out.output);
    }
}

#[test]
fn the_fixed_program_prints_what_the_original_printed() {
    let src = "\
fn sq(x) x * x end
state SQUARES = map([1, 2, 3], sq)
state TOTAL = reduce(SQUARES, 0, fn(a, b) -> a + b)
state frames = 0
frames = frames + 1
print(SQUARES, TOTAL, frames)
";
    let out = lint(src);
    assert_eq!(out.count(STATE_AS_CONSTANT), 2);
    assert!(out.output.contains("state frames = 0"), "{}", out.output);
    let run = |text: &str| {
        let mut env = petal::env::Env::new();
        env.set_echo(false);
        let pid = env.load_program(text).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        env.take_output()
    };
    assert_eq!(run(src), run(&out.output));
}

#[test]
fn the_rule_is_opt_in() {
    // A plain `petal lint` does not run it: the rewrite is a judgement, and
    // a blanket `--fix` must not make it.
    let src = "state LIMIT = 9\nprint(LIMIT)\n";
    let default = lint_source(src, &LintOptions::default()).unwrap();
    assert_eq!(default.findings.len(), 0, "{:?}", default.findings);
    assert_eq!(default.output, src);

    let rules = RuleFilter::default();
    assert!(!rules.enabled(STATE_AS_CONSTANT));
    assert!(rules.enabled(petal::lint::PREFER_LET));
    // Named next to another rule, both run.
    let both = RuleFilter {
        include: Some(vec![STATE_AS_CONSTANT.to_string(), petal::lint::PREFER_LET.to_string()]),
        exclude: Vec::new(),
    };
    assert!(both.enabled(STATE_AS_CONSTANT) && both.enabled(petal::lint::PREFER_LET));
    let info = petal::lint::RULES.iter().find(|r| r.name == STATE_AS_CONSTANT).unwrap();
    assert!(info.opt_in);
    // The other opt-in rule is `int-division`.
    let opt_in: Vec<&str> = petal::lint::RULES.iter().filter(|r| r.opt_in).map(|r| r.name).collect();
    assert_eq!(opt_in, vec![petal::lint::INT_DIVISION, STATE_AS_CONSTANT]);
}
