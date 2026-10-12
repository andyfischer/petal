//! `match` as the control-flow statement: statement-list arm bodies,
//! or-patterns, the exhaustiveness error, the unreachable-arm warning, the
//! no-arm-matched runtime error, and the jump table a run of literal or
//! variant arms is entered through. One section per rule of
//! docs/language-guide.md (Pattern Matching).

use petal::ast::{ExprKind, Pattern, Stmt, StmtKind};
use petal::env::Env;
use petal::lexer::Lexer;
use petal::parse::Parser;
use petal::policy::RunPolicy;

fn try_run(src: &str, policy: RunPolicy) -> Result<Vec<String>, String> {
    let mut env = Env::new();
    env.set_echo(false);
    env.set_policy(policy);
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid).unwrap();
    env.run(sid)?;
    Ok(env.take_output())
}

/// Run `src` and return what it printed, one entry per `print`. Every program
/// is run with the optimizer on and off, and the two must agree.
#[track_caller]
fn run(src: &str) -> Vec<String> {
    let fast = try_run(src, RunPolicy::FAST).unwrap_or_else(|e| panic!("{src}\n-- failed: {e}"));
    let baseline =
        try_run(src, RunPolicy::BASELINE).unwrap_or_else(|e| panic!("{src}\n-- failed: {e}"));
    assert_eq!(fast, baseline, "optimizer on and off disagree:\n{src}");
    fast
}

/// Compile `src` expecting a compile error; return the message.
#[track_caller]
fn compile_err(src: &str) -> String {
    let mut env = Env::new();
    env.set_echo(false);
    match env.load_program(src) {
        Ok(_) => panic!("expected a compile error, but this compiled:\n{src}"),
        Err(e) => e,
    }
}

/// Run `src` expecting it to compile and then fail, the same way with the
/// optimizer on and off; return the message.
#[track_caller]
fn run_err(src: &str) -> String {
    let fast = try_run(src, RunPolicy::FAST).expect_err("expected a runtime error");
    let baseline = try_run(src, RunPolicy::BASELINE).expect_err("expected a runtime error");
    assert_eq!(fast, baseline, "optimizer on and off disagree:\n{src}");
    fast
}

/// The compiler's warnings for `src` — what `petal check` prints as warnings.
#[track_caller]
fn warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let program = env.get_program(pid).unwrap();
    program.warnings.iter().map(|d| d.message.clone()).collect()
}

/// The program's bytecode, as `petal show-bytecode` prints it.
fn bytecode(src: &str) -> String {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).unwrap();
    env.bytecode_text(pid).unwrap()
}

/// Parse with the hand-written parser alone.
fn parse(src: &str) -> Vec<Stmt> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().unwrap();
    Parser::new(lexer.tokens, lexer.token_spans)
        .parse_program()
        .unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"))
}

fn parse_err(src: &str) -> String {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().unwrap();
    match Parser::new(lexer.tokens, lexer.token_spans).parse_program() {
        Ok(_) => panic!("expected a parse error for {src:?}"),
        Err(e) => e,
    }
}

/// The arms of the `match` that is the program's first statement (or the
/// value of its first `let`).
fn first_match_arms(stmts: &[Stmt]) -> &[petal::ast::MatchArm] {
    let expr = match &stmts[0].kind {
        StmtKind::Expr(e) => e,
        StmtKind::Let { value, .. } => value,
        other => panic!("expected a match, got {other:?}"),
    };
    let ExprKind::Match { arms, .. } = &expr.kind else {
        panic!("expected a match, got {:?}", expr.kind);
    };
    arms
}

const KIND: &str = "\
enum Kind
  park,
  plaza,
  road(lanes),
end
";

const CMD: &str = "\
enum Cmd
  quit,
  move(dx, dy),
  say(text),
end
";

// ── Arm bodies are statement lists ───────────────────────────────

#[test]
fn an_arm_body_is_a_statement_list_and_its_last_expression_is_the_value() {
    let src = format!(
        "{CMD}
var x = 0
var y = 0
fn step(cmd)
  match cmd
    when Cmd.quit -> \"bye\"
    when Cmd.move(dx, dy) ->
      set x += dx
      set y += dy
      \"moved to {{get x}},{{get y}}\"
    when Cmd.say(text) ->
      let loud = text ++ \"!\"
      loud
  end
end
print(step(Cmd.move(1, 2)))
print(step(Cmd.move(10, 20)))
print(step(Cmd.say(\"hi\")))
print(step(Cmd.quit))
"
    );
    assert_eq!(run(&src), ["moved to 1,2", "moved to 11,22", "hi!", "bye"]);
}

#[test]
fn a_body_runs_to_the_next_when_or_to_the_end() {
    // The body of the first arm is three statements; nothing of it leaks into
    // the second arm, and the last arm's body stops at the match's `end`.
    let src = "
fn f(n)
  var log = []
  match n
    when 1 ->
      set log = push(get log, \"a\")
      set log = push(get log, \"b\")
      set log = push(get log, \"c\")
    when 2 ->
      set log = push(get log, \"d\")
    when _ ->
      set log = push(get log, \"e\")
      set log = push(get log, \"f\")
  end
  set log = push(get log, \"after\")
  get log
end
print(f(1))
print(f(2))
print(f(3))
";
    assert_eq!(
        run(src),
        [
            "[\"a\", \"b\", \"c\", \"after\"]",
            "[\"d\", \"after\"]",
            "[\"e\", \"f\", \"after\"]"
        ]
    );
}

#[test]
fn a_one_expression_arm_is_what_it_always_was() {
    // The AST of a one-expression arm is that expression, not a block; a
    // longer body is a block of its statements.
    let stmts = parse("match x\n  when 1 -> \"one\"\n  when 2 ->\n    let a = 1\n    a\nend\n");
    let arms = first_match_arms(&stmts);
    assert!(matches!(arms[0].body.kind, ExprKind::Literal(_)));
    let ExprKind::Block(body) = &arms[1].body.kind else {
        panic!("expected a block body, got {:?}", arms[1].body.kind);
    };
    assert_eq!(body.len(), 2);

    // On one line, on the line below the arrow, and several arms to a line.
    let src = "
let a = match 2 when 1 -> \"one\" when 2 -> \"two\" when _ -> \"many\" end
let b = match 2
  when 1 ->
    \"one\"
  when 2 ->
    \"two\"
  when _ -> \"many\"
end
print(a, b)
";
    assert_eq!(run(src), ["two two"]);
}

#[test]
fn a_do_end_arm_still_works() {
    let src = "
let r = match 2
  when 1 do
    \"one\"
  end
  when 2 do
    let t = \"tw\"
    t ++ \"o\"
  end
  when _ -> \"many\"
end
print(r)
";
    assert_eq!(run(src), ["two"]);
}

#[test]
fn a_single_statement_body_need_not_be_an_expression() {
    // `set`, `let`, `return`, `break`: a statement after the arrow is a body.
    let src = "
var n = 0
fn bump(k)
  match k
    when \"up\" -> set n += 1
    when \"down\" -> set n -= 1
    when _ -> return \"ignored\"
  end
  get n
end
print(bump(\"up\"), bump(\"up\"), bump(\"down\"), bump(\"sideways\"))
";
    assert_eq!(run(src), ["1 2 1 ignored"]);
}

#[test]
fn an_arm_needs_a_body() {
    let err = parse_err("match x\n  when 1 ->\n  when 2 -> 3\nend\n");
    assert!(err.contains("Expected the arm's body after '->'"), "{err}");
    assert!(err.contains("-> nil"), "{err}");
    let err = parse_err("match x\n  when 1 ->\nend\n");
    assert!(err.contains("Expected the arm's body after '->'"), "{err}");
}

#[test]
fn a_match_has_no_else_arm() {
    let err = parse_err("match x\n  when 1 -> \"a\"\n  else -> \"b\"\nend\n");
    assert!(err.contains("a `match` has no `else` arm"), "{err}");
    assert!(err.contains("when _ ->"), "{err}");
    // An `if` inside a body keeps its own `else`.
    let src = "print(match 1\n  when 1 ->\n    if false then \"a\"\n    else \"b\" end\n  when _ -> \"c\"\nend)\n";
    assert_eq!(run(src), ["b"]);
}

#[test]
fn match_works_in_statement_and_in_value_position() {
    let src = "
fn describe(n)
  // Statement position: the value is dropped, the effects are not.
  match n
    when 0 ->
      print(\"zero\")
      print(\"nothing\")
    when _ -> print(\"some\")
  end
  // Value position, as an argument and as a `let`.
  print(match n
    when 0 -> \"none\"
    when _ ->
      let s = str(n)
      s ++ \" of them\"
  end)
  let size = match n
    when 0 | 1 -> \"small\"
    when _ ->
      let big = n > 100
      if big then \"huge\" else \"medium\" end
  end
  size
end
print(describe(0))
print(describe(500))
";
    assert_eq!(
        run(src),
        ["zero", "nothing", "none", "small", "some", "500 of them", "huge"]
    );
}

#[test]
fn a_match_nests_inside_an_arm_body() {
    // The inner match takes its own `when`s and its own `end`; the outer
    // arm's body goes on after it.
    let src = format!(
        "{KIND}
fn f(k, n)
  match k
    when Kind.park ->
      let size = match n
        when 1 -> \"small\"
        when _ ->
          let m = n * 2
          \"size {{m}}\"
      end
      match n
        when 1 -> print(\"one\")
        when _ -> print(\"more\")
      end
      \"park, \" ++ size
    when Kind.plaza -> \"plaza\"
    when Kind.road(lanes) ->
      match lanes
        when 1 | 2 -> \"lane\"
        when _ -> \"highway\"
      end
  end
end
print(f(Kind.park, 1))
print(f(Kind.park, 4))
print(f(Kind.plaza, 0))
print(f(Kind.road(2), 0))
print(f(Kind.road(6), 0))
"
    );
    assert_eq!(
        run(&src),
        ["one", "park, small", "more", "park, size 8", "plaza", "lane", "highway"]
    );
}

#[test]
fn break_continue_and_return_in_an_arm_behave_as_in_an_if_branch() {
    // The same loop written with `match` and with `if`: the two agree.
    let with_match = "
fn scan(items)
  var seen = []
  for item in items do
    match item
      when \"skip\" -> continue
      when \"stop\" ->
        set seen = push(get seen, \"stopped\")
        break
      when \"bail\" -> return \"bailed with {len(get seen)}\"
      when _ -> set seen = push(get seen, item)
    end
    set seen = push(get seen, \"|\")
  end
  join(get seen, \"\")
end
";
    let with_if = "
fn scan(items)
  var seen = []
  for item in items do
    if item == \"skip\" then
      continue
    elsif item == \"stop\" then
      set seen = push(get seen, \"stopped\")
      break
    elsif item == \"bail\" then
      return \"bailed with {len(get seen)}\"
    else
      set seen = push(get seen, item)
    end
    set seen = push(get seen, \"|\")
  end
  join(get seen, \"\")
end
";
    let calls = "
print(scan([\"a\", \"skip\", \"b\", \"stop\", \"c\"]))
print(scan([\"a\", \"bail\", \"b\"]))
print(scan([\"skip\", \"skip\"]))
";
    let expected = ["a|b|stopped", "bailed with 2", ""];
    assert_eq!(run(&format!("{with_match}{calls}")), expected);
    assert_eq!(run(&format!("{with_if}{calls}")), expected);

    // In a `while`, and from a match nested in an arm of another.
    let src = "
fn first_big(limit)
  var i = 0
  var hits = 0
  while true do
    set i += 1
    match get i % 3
      when 0 -> continue
      when _ ->
        match get i > limit
          when true -> return \"stopped at {get i} after {get hits}\"
          when false -> set hits += 1
        end
    end
  end
end
print(first_big(7))
";
    assert_eq!(run(src), ["stopped at 8 after 5"]);
}

#[test]
fn a_name_bound_in_an_arm_body_is_local_to_the_arm() {
    let src = "
let label = \"outer\"
let r = match 1
  when 1 ->
    let label = \"inner\"
    label
  when _ -> label
end
print(r, label)
";
    assert_eq!(run(src), ["inner outer"]);
    // A `let` of one arm is not in scope in the next.
    let err = run_err("match 2\n  when 1 ->\n    let a = 1\n    a\n  when _ -> a\nend\n");
    assert!(err.contains("Undefined variable: a"), "{err}");
}

#[test]
fn a_binding_reassigned_in_an_arm_body_carries_out_of_the_match() {
    // Top-level `let`s rebound by assignment inside an arm are joined after
    // the match, exactly as after an `if`.
    let src = "
let total = 0
let count = 0
for n in [1, 2, 3, 4] do
  match n % 2
    when 0 ->
      total = total + n
      count = count + 1
    when _ -> total = total - 1
  end
end
print(total, count)
";
    assert_eq!(run(src), ["4 2"]);
}

#[test]
fn the_parser_and_the_cst_projection_agree_on_the_new_syntax() {
    let sources = [
        "match x\n  when 1 -> \"one\"\n  when _ -> nil\nend\n",
        "match x\n  when 1 ->\n    let a = 1\n    a + 1\n  when 2 -> set y = 2\n  when _ ->\n    nil\nend\n",
        "let r = match x when 1 | 2 -> \"a\" when \"b\" | \"c\" | \"d\" -> \"e\" when _ -> nil end\n",
        "match k\n  when Kind.park | Kind.plaza -> 1\n  when mod.Kind.road(n) | mod.Kind.lane(n) if n > 1 ->\n    print(n)\n    n\n  when _ -> 0\nend\n",
        "match v\n  when [1 | 2, x] -> x\n  when {kind: \"a\" | \"b\", n: n} -> n\n  when Op.fight(\"x\" | \"y\", _) -> 0\n  when _ -> nil\nend\n",
        "match x\n  when \"a\"\n     | \"b\"\n     | \"c\" -> 1\n  when 1 |\n       2 -> 2\n  when _ -> 3\nend\n",
        "for i in xs do\n  match i\n    when 1 -> continue\n    when 2 ->\n      print(i)\n      break\n    when _ ->\n      match i\n        when 3 -> return 3\n        when _ -> nil\n      end\n  end\nend\n",
        "match x\n  when 1 do\n    a\n  end\n  when 2 -> b\nend\n",
    ];
    for src in sources {
        let direct = parse(src);
        let (_, projected) = petal::cst::parse_source(src, petal::source_map::ENTRY_FILE)
            .unwrap_or_else(|e| panic!("{src}\n-- {e}"));
        assert_eq!(
            format!("{direct:#?}"),
            format!("{projected:#?}"),
            "the CST projection diverged from the parser for:\n{src}"
        );
    }
}

// ── Or-patterns ──────────────────────────────────────────────────

#[test]
fn an_or_pattern_matches_any_of_its_alternatives() {
    let stmts = parse("match x\n  when \"a\" | \"b\" | \"c\" -> 1\n  when _ -> 2\nend\n");
    let Pattern::Or(alts) = &first_match_arms(&stmts)[0].pattern else {
        panic!("expected an or-pattern");
    };
    assert_eq!(alts.len(), 3);

    let src = format!(
        "{KIND}
fn shade(k)
  match k
    when Kind.park | Kind.plaza -> \"open\"
    when Kind.road(1) | Kind.road(2) -> \"street\"
    when Kind.road(_) -> \"highway\"
  end
end
fn vowel(s)
  match s
    when \"a\" | \"e\" | \"i\" | \"o\" | \"u\" -> true
    when _ -> false
  end
end
fn small(n)
  match n
    when 0 | 1 | 2 -> \"small\"
    when nil | false -> \"nothing\"
    when _ -> \"other\"
  end
end
print(shade(Kind.park), shade(Kind.plaza), shade(Kind.road(2)), shade(Kind.road(8)))
print(vowel(\"e\"), vowel(\"x\"))
print(small(2), small(3), small(nil), small(false))
"
    );
    assert_eq!(
        run(&src),
        ["open open street highway", "true false", "small other nothing nothing"]
    );
}

#[test]
fn an_or_pattern_may_bind_and_may_be_nested_and_guarded() {
    let src = "
enum Op
  fight(place, won),
  truce(place),
  gift(amount),
end
fn place_of(op)
  match op
    // Each alternative binds `place`, so the body may use it.
    when Op.fight(place, _) | Op.truce(place) -> place
    when Op.gift(_) -> \"nowhere\"
  end
end
fn classify(v)
  match v
    // Alternatives inside a list, a record and a payload.
    when [0 | 1, rest] -> \"bit then {rest}\"
    when {kind: \"a\" | \"b\", n: n} -> \"ab {n}\"
    when Op.fight(\"north\" | \"south\", won) -> \"pole, won={won}\"
    // A guard applies to the whole pattern, whichever alternative matched.
    when Op.gift(n) | Op.truce(n) if n == 5 -> \"five\"
    when _ -> \"other\"
  end
end
print(place_of(Op.fight(\"east\", true)), place_of(Op.truce(\"west\")), place_of(Op.gift(1)))
print(classify([1, \"x\"]))
print(classify([2, \"x\"]))
print(classify({kind: \"b\", n: 7}))
print(classify(Op.fight(\"south\", false)))
print(classify(Op.fight(\"east\", false)))
print(classify(Op.gift(5)), classify(Op.truce(5)), classify(Op.gift(6)))
";
    assert_eq!(
        run(src),
        [
            "east west nowhere",
            "bit then x",
            "other",
            "ab 7",
            "pole, won=false",
            "other",
            "five five other"
        ]
    );
}

#[test]
fn an_or_pattern_may_span_lines() {
    let src = "
fn f(s)
  match s
    when \"a\"
       | \"b\"
       | \"c\" -> 1
    when \"d\" |
         \"e\" -> 2
    when _ -> 3
  end
end
print(f(\"a\"), f(\"c\"), f(\"e\"), f(\"z\"))
";
    assert_eq!(run(src), ["1 1 2 3"]);
}

#[test]
fn every_alternative_must_bind_the_same_names() {
    let op = "enum Op\n  fight(place, won),\n  truce(place),\nend\n";
    let err = compile_err(&format!(
        "{op}fn f(op)\n  match op\n    when Op.fight(place, won) | Op.truce(place) -> place\n  end\nend\n"
    ));
    assert!(
        err.contains("every alternative of an or-pattern must bind the same names"),
        "{err}"
    );
    assert!(err.contains("`Op.fight(place, won)` binds `won`"), "{err}");
    assert!(err.contains("`Op.truce(place)` does not"), "{err}");
    // The error points at the pattern.
    assert!(err.contains("line 7, column 10"), "{err}");

    // A binding beside a literal, and a nested or-pattern, are held to it too.
    let err = compile_err("match 1\n  when 1 | n -> 0\nend\n");
    assert!(err.contains("`n` binds `n` and `1` does not"), "{err}");
    let err = compile_err("match [1, 2]\n  when [a | 1, _] -> 0\n  when _ -> 1\nend\n");
    assert!(err.contains("`a` binds `a` and `1` does not"), "{err}");

    // The same names in a different order are the same names.
    let src = "
fn f(v)
  match v
    when [\"ab\", a, b] | [\"ba\", b, a] -> \"{a}{b}\"
    when _ -> \"?\"
  end
end
print(f([\"ab\", 1, 2]), f([\"ba\", 1, 2]))
";
    assert_eq!(run(src), ["12 21"]);
}

#[test]
fn a_lone_bar_is_not_an_operator() {
    let err = parse_err("let a = 1 | 2\n");
    assert!(err.contains("'|'"), "{err}");
}

// ── Exhaustiveness ───────────────────────────────────────────────

#[test]
fn a_match_over_an_enum_must_cover_every_variant() {
    let err = compile_err(&format!(
        "{KIND}fn f(k)\n  match k\n    when Kind.park -> 1\n  end\nend\n"
    ));
    assert!(
        err.contains("does not cover every variant of `Kind`"),
        "{err}"
    );
    assert!(err.contains("no arm for `Kind.plaza` and `Kind.road`"), "{err}");
    assert!(err.contains("when _ ->"), "{err}");
    // It points at the match.
    assert!(err.contains("line 7, column 3"), "{err}");

    // One missing variant, with or-alternatives counted.
    let err = compile_err(&format!(
        "{KIND}fn f(k)\n  match k\n    when Kind.park | Kind.road(_) -> 1\n  end\nend\n"
    ));
    assert!(err.contains("no arm for `Kind.plaza` ("), "{err}");

    // Covered: every variant, in any order and grouping.
    let src = format!(
        "{KIND}
fn f(k)
  match k
    when Kind.road(n) -> n
    when Kind.plaza | Kind.park -> 0
  end
end
print(f(Kind.road(3)), f(Kind.park))
"
    );
    assert_eq!(run(&src), ["3 0"]);
}

#[test]
fn a_guarded_arm_covers_nothing() {
    let err = compile_err(&format!(
        "{KIND}fn f(k, on)
  match k
    when Kind.park if on -> 1
    when Kind.plaza -> 2
    when Kind.road(_) -> 3
  end
end
"
    ));
    assert!(err.contains("no arm for `Kind.park`"), "{err}");
    assert!(err.contains("an arm with an `if` guard"), "{err}");

    // The same variant again without the guard covers it.
    let src = format!(
        "{KIND}
fn f(k, on)
  match k
    when Kind.park if on -> 1
    when Kind.park -> 0
    when Kind.plaza -> 2
    when Kind.road(_) -> 3
  end
end
print(f(Kind.park, true), f(Kind.park, false))
"
    );
    assert_eq!(run(&src), ["1 0"]);
}

#[test]
fn a_payload_arm_covers_its_variant_only_when_its_sub_patterns_cannot_fail() {
    let err = compile_err(&format!(
        "{KIND}fn f(k)
  match k
    when Kind.park | Kind.plaza -> 0
    when Kind.road(1) -> 1
    when Kind.road(2) -> 2
  end
end
"
    ));
    assert!(err.contains("no arm for `Kind.road`"), "{err}");
    assert!(err.contains("whose payload patterns can fail"), "{err}");

    // A binding, a wildcard, or an or-pattern with one of them cannot fail.
    for road in ["Kind.road(n)", "Kind.road(_)", "Kind.road(1 | _)"] {
        let src = format!(
            "{KIND}fn f(k)
  match k
    when Kind.park | Kind.plaza -> 0
    when Kind.road(1) -> 1
    when {road} -> 9
  end
end
print(f(Kind.road(1)), f(Kind.road(5)))
"
        );
        assert_eq!(run(&src), ["1 9"], "{road}");
    }
}

#[test]
fn an_arm_for_everything_else_makes_any_match_exhaustive() {
    for rest in ["when _ -> 9", "when other -> 9", "when Kind.plaza | _ -> 9"] {
        let src = format!(
            "{KIND}fn f(k)
  match k
    when Kind.park -> 1
    {rest}
  end
end
print(f(Kind.park), f(Kind.road(2)))
"
        );
        assert_eq!(run(&src), ["1 9"], "{rest}");
    }
    // A guarded catch-all is not one: the arms are still not all variants of
    // one enum, so nothing is checked, and the gap shows up at run time.
    let src = format!(
        "{KIND}fn f(k, on)
  match k
    when Kind.park -> 1
    when _ if on -> 9
  end
end
print(f(Kind.plaza, true))
print(f(Kind.plaza, false))
"
    );
    let err = run_err(&src);
    assert!(err.contains("No matching pattern for value: Kind.plaza"), "{err}");
}

#[test]
fn only_a_match_whose_arms_are_all_variants_of_one_enum_is_checked() {
    // A literal arm, or variants of two enums: the subject is not known to be
    // a `Kind`, so the match is not held to covering `Kind`.
    let src = format!(
        "{KIND}enum Use
  green,
  paved,
end
fn f(v)
  match v
    when Kind.park -> \"park\"
    when Use.green -> \"green\"
  end
end
fn g(v)
  match v
    when Kind.park -> \"park\"
    when \"park\" -> \"a string\"
  end
end
print(f(Use.green), g(\"park\"))
"
    );
    assert_eq!(run(&src), ["green a string"]);
    // Strings, ints and lists are never checked.
    assert_eq!(run("print(match 2\n  when 1 -> \"a\"\n  when 2 -> \"b\"\nend)\n"), ["b"]);
}

#[test]
fn exhaustiveness_sees_through_a_module() {
    let mut env = Env::new();
    env.set_echo(false);
    env.register_module("shapes", "pub enum Shape\n  dot,\n  line(len),\nend\n");
    let err = env
        .load_program("import shapes\nfn f(s)\n  match s\n    when shapes.Shape.dot -> 0\n  end\nend\n")
        .expect_err("a missing variant is a compile error");
    assert!(err.contains("no arm for `Shape.line`"), "{err}");
}

// ── Unreachable arms (a warning) ─────────────────────────────────

#[test]
fn an_arm_after_one_that_matches_everything_is_unreachable() {
    let w = warnings("fn f(x)\n  match x\n    when 1 -> \"a\"\n    when _ -> \"b\"\n    when 2 -> \"c\"\n    when y -> \"d\"\n  end\nend\n");
    let unreachable: Vec<&String> = w.iter().filter(|m| m.contains("can never run")).collect();
    assert_eq!(unreachable.len(), 2, "{w:?}");
    assert!(
        unreachable[0].contains("an earlier arm of the match already matches every value"),
        "{w:?}"
    );

    // A binding matches everything too — which is how a bare variant name in
    // a pattern goes wrong.
    let w = warnings(&format!(
        "{KIND}fn f(k)\n  match k\n    when park -> 1\n    when Kind.plaza -> 2\n  end\nend\n"
    ));
    assert!(w.iter().any(|m| m.contains("can never run")), "{w:?}");
}

#[test]
fn a_repeated_literal_or_variant_is_unreachable() {
    let w = warnings("fn f(x)\n  match x\n    when \"a\" -> 1\n    when \"b\" -> 2\n    when \"a\" -> 3\n    when _ -> 4\n  end\nend\n");
    assert!(
        w.iter()
            .any(|m| m.contains("this arm can never run: an earlier arm already matches `\"a\"`")),
        "{w:?}"
    );

    // `2` and `2.0` match the same values.
    let w = warnings("fn f(x)\n  match x\n    when 2 -> 1\n    when 2.0 -> 2\n    when _ -> 3\n  end\nend\n");
    assert!(w.iter().any(|m| m.contains("can never run")), "{w:?}");

    let w = warnings(&format!(
        "{KIND}fn f(k)
  match k
    when Kind.park | Kind.plaza -> 1
    when Kind.road(n) -> n
    when Kind.plaza -> 2
    when Kind.road(1) -> 3
  end
end
"
    ));
    let dead: Vec<&String> = w.iter().filter(|m| m.contains("can never run")).collect();
    assert_eq!(dead.len(), 2, "{w:?}");
    assert!(dead[0].contains("`Kind.plaza`"), "{w:?}");
    assert!(dead[1].contains("`Kind.road(1)`"), "{w:?}");
}

#[test]
fn a_repeated_alternative_is_reported_on_its_own() {
    let w = warnings("fn f(x)\n  match x\n    when \"a\" | \"b\" -> 1\n    when \"b\" | \"c\" -> 2\n    when \"d\" | \"d\" -> 3\n    when _ -> 4\n  end\nend\n");
    assert!(
        w.iter().any(|m| m.contains("`\"b\"` can never match here")),
        "{w:?}"
    );
    assert!(
        w.iter().any(|m| m.contains("`\"d\"` can never match here")),
        "{w:?}"
    );
    assert!(!w.iter().any(|m| m.contains("this arm can never run")), "{w:?}");
}

#[test]
fn an_arm_that_can_still_run_draws_no_warning() {
    // A guarded arm takes nothing for certain, and a payload pattern that can
    // fail leaves its variant open: what follows them is reachable.
    let w = warnings(&format!(
        "{KIND}fn f(k, on, s)
  let a = match k
    when Kind.park if on -> 1
    when Kind.park -> 2
    when Kind.road(1) -> 3
    when Kind.road(n) -> n
    when Kind.plaza -> 4
  end
  let b = match s
    when \"a\" if on -> 1
    when \"a\" -> 2
    when x if on -> 3
    when [1, _] -> 4
    when [1, 2] -> 5
    when _ -> 6
  end
  a + b
end
"
    ));
    assert!(
        !w.iter().any(|m| m.contains("can never")),
        "unexpected unreachable-arm warning: {w:?}"
    );
}

// ── No arm matched (run time) ────────────────────────────────────

#[test]
fn a_match_no_arm_of_which_matched_is_a_runtime_error() {
    // In value position: the error names the value and where the match is.
    let err = run_err("let x = 3\nlet r = match x\n  when 1 -> \"one\"\n  when 2 -> \"two\"\nend\nprint(r)\n");
    assert!(err.contains("No matching pattern for value: 3"), "{err}");
    assert!(err.contains("line 2, column 9"), "{err}");

    // In statement position, where the value was never wanted, all the same.
    let err = run_err("fn f(s)\n  match s\n    when \"a\" -> print(\"a\")\n  end\n  print(\"after\")\nend\nf(\"zebra\")\n");
    assert!(err.contains("No matching pattern for value: zebra"), "{err}");
    assert!(err.contains("line 2, column 3"), "{err}");

    // A variant, printed as it is written; a failed guard is not a match.
    let err = run_err(&format!(
        "{KIND}fn f(k)\n  match k\n    when Kind.road(n) if n > 2 -> n\n    when [a] -> a\n  end\nend\nf(Kind.road(1))\n"
    ));
    assert!(err.contains("No matching pattern for value: Kind.road(1)"), "{err}");

    // Written out, the fallback is what yields nil.
    assert_eq!(
        run("print(match 3\n  when 1 -> \"one\"\n  when _ -> nil\nend)\n"),
        ["nil"]
    );
}

// ── Literal dispatch ─────────────────────────────────────────────

/// A `pick` function with one arm per entry of `arms`, called on each of
/// `subjects`; prints the list of results.
fn dispatch_program(prelude: &str, arms: &[&str], subjects: &[&str]) -> String {
    let mut src = format!("{prelude}fn pick(x)\n  match x\n");
    for arm in arms {
        src.push_str(&format!("    when {arm}\n"));
    }
    src.push_str("  end\nend\n");
    src.push_str(&format!(
        "print([{}])\n",
        subjects
            .iter()
            .map(|s| format!("pick({s})"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    src
}

#[test]
fn literal_arms_are_entered_through_a_jump_table() {
    let src = dispatch_program(
        "",
        &["\"a\" -> 1", "\"b\" -> 2", "\"c\" -> 3", "_ -> 0"],
        &["\"b\"", "\"z\""],
    );
    let code = bytecode(&src);
    assert!(
        code.contains("match_switch") && code.contains("\"a\" =>") && code.contains("\"c\" =>"),
        "expected a string jump table:\n{code}"
    );
    assert_eq!(run(&src), ["[2, 0]"]);

    let src = dispatch_program("", &["10 -> 1", "20 -> 2", "30 | 40 -> 3", "_ -> 0"], &["40", "5"]);
    let code = bytecode(&src);
    assert!(
        code.contains("match_switch") && code.contains("40 =>"),
        "expected an int jump table:\n{code}"
    );
    assert_eq!(run(&src), ["[3, 0]"]);

    // Payload-free variants dispatch the same way; a payload variant's case
    // lands on its arm's own test (`->`), which binds the payload.
    let src = dispatch_program(
        KIND,
        &["Kind.park -> 1", "Kind.plaza -> 2", "Kind.road(n) -> n"],
        &["Kind.plaza", "Kind.road(7)"],
    );
    let code = bytecode(&src);
    assert!(
        code.contains("Kind.park =>") && code.contains("Kind.road ->"),
        "expected a variant jump table:\n{code}"
    );
    assert_eq!(run(&src), ["[2, 7]"]);

    // One literal arm is not worth a table.
    let src = dispatch_program("", &["\"a\" -> 1", "_ -> 0"], &["\"a\""]);
    assert!(!bytecode(&src).contains("match_switch"));
}

#[test]
fn dispatch_is_first_match_wins() {
    // Every subject is run through a match with duplicates, guards, a float
    // literal splitting two runs, a binding arm in the middle, and literals
    // after it. `run` holds the optimizer on and off to the same answer.
    let arms = [
        "\"a\" | \"b\" -> 1",
        "2 -> 2",
        "\"a\" -> 3",          // dead: "a" was taken above
        "\"c\" if false -> 4", // never taken: falls to the later \"c\"
        "\"c\" -> 5",
        "3 if false -> 6",
        "2.5 -> 7",            // not keyed: ends the first run
        "3 -> 8",
        "\"d\" -> 9",
        "[_] -> 10",
        "n if n == 4 -> 11",   // a binding, tried in its place
        "4 -> 12",             // dead for 4, which the arm above takes
        "\"e\" -> 13",
        "_ -> 0",
    ];
    let subjects = [
        "\"a\"", "\"b\"", "\"c\"", "\"d\"", "\"e\"", "\"zz\"", "2", "2.0", "3", "3.0", "4", "4.0",
        "2.5", "5", "nil", "true", "[1]", "[1, 2]", "\"\"",
    ];
    let src = dispatch_program("", &arms, &subjects);
    assert!(bytecode(&src).contains("match_switch"));
    assert_eq!(
        run(&src),
        ["[1, 1, 5, 9, 13, 0, 2, 2, 8, 8, 11, 11, 7, 0, 0, 0, 10, 0, 0]"]
    );
}

#[test]
fn variant_dispatch_is_first_match_wins() {
    let prelude = "\
enum K
  a,
  b,
  c(n),
  d(x, y),
end
enum Other
  a,
end
";
    let arms = [
        "K.a -> 1",
        "K.c(n) if n > 10 -> 2",
        "K.c(0) -> 3",
        "\"K.a\" -> 4",   // a string that spells a variant is still a string
        "K.d(1, y) -> y",
        "K.a -> 5",       // dead
        "K.c(n) -> n",
        "Other.a -> 6",
        "K.d(_, _) -> 7",
        "K.b | 8 -> 8",
        "_ -> 0",
    ];
    let subjects = [
        "K.a", "K.b", "K.c(50)", "K.c(0)", "K.c(4)", "K.d(1, 99)", "K.d(2, 99)", "Other.a",
        "\"K.a\"", "8", "\"K.b\"", "nil",
    ];
    let src = dispatch_program(prelude, &arms, &subjects);
    assert!(bytecode(&src).contains("match_switch"));
    assert_eq!(run(&src), ["[1, 8, 2, 3, 4, 99, 7, 6, 4, 8, 0, 0]"]);
}

#[test]
fn a_subject_the_table_does_not_name_reaches_the_arms_after_it() {
    // No fallback arm: a miss in the table is still the no-match error, and a
    // hit past the table (a list arm) is still found.
    let src = dispatch_program("", &["\"a\" -> 1", "\"b\" -> 2", "[x] -> x"], &["\"b\"", "[7]"]);
    assert_eq!(run(&src), ["[2, 7]"]);
    let src = dispatch_program("", &["\"a\" -> 1", "\"b\" -> 2", "[x] -> x"], &["\"q\""]);
    assert!(run_err(&src).contains("No matching pattern for value: q"));
    let src = dispatch_program("", &["1 -> 1", "2 -> 2"], &["3"]);
    assert!(run_err(&src).contains("No matching pattern for value: 3"));
}

#[test]
fn a_dispatched_match_keeps_its_bodies_bindings_and_carried_values() {
    // The jump lands in a body that rebinds an outer name, inside a loop: the
    // value carried out of the match and around the loop is the same as with
    // the arms tested one by one.
    let src = "
let total = 0
let trail = \"\"
for word in [\"add\", \"double\", \"add\", \"reset\", \"add\", \"other\", \"double\"] do
  match word
    when \"add\" ->
      total = total + 1
      trail = trail ++ \"+\"
    when \"double\" -> total = total * 2
    when \"reset\" ->
      total = 0
      trail = \"\"
    when _ -> trail = trail ++ \"?\"
  end
end
print(total, trail)
";
    assert_eq!(run(src), ["2 +?"]);
}

#[test]
fn leaving_a_loop_from_a_dispatched_arm_carries_the_loop_values_out() {
    // `break` and `continue` from a body the jump table lands in, with `let`s
    // rebound before them: the values that leave the loop are the latest.
    let src = "
fn tally(words)
  let count = 0
  let last = \"none\"
  for word in words do
    match word
      when \"skip\" ->
        last = \"skipped\"
        continue
      when \"stop\" ->
        last = \"stopped\"
        break
      when \"twice\" -> count = count + 2
      when _ -> count = count + 1
    end
    last = word
  end
  \"{count} {last}\"
end
print(tally([\"a\", \"twice\", \"skip\"]))
print(tally([\"a\", \"skip\", \"b\", \"stop\", \"c\"]))
print(tally([]))
";
    assert_eq!(run(src), ["3 skipped", "2 stopped", "0 none"]);
}

#[test]
fn a_forty_arm_string_match_agrees_with_a_chain_of_comparisons() {
    let names: Vec<String> = (0..40).map(|i| format!("cmd_{i:02}")).collect();
    let mut src = String::from("fn by_match(k)\n  match k\n");
    for (i, n) in names.iter().enumerate() {
        src.push_str(&format!("    when \"{n}\" -> {i}\n"));
    }
    src.push_str("    when _ -> -1\n  end\nend\nfn by_chain(k)\n");
    for (i, n) in names.iter().enumerate() {
        src.push_str(&format!("  if k == \"{n}\" then return {i} end\n"));
    }
    src.push_str("  -1\nend\nvar bad = 0\nvar sum = 0\n");
    src.push_str(&format!(
        "let keys = [{}, \"nope\", \"\", \"cmd_4\"]\n",
        names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
    ));
    src.push_str(
        "for i in range(0, 500) do\n  let k = keys[i % len(keys)]\n  if by_match(k) != by_chain(k) then set bad += 1 end\n  set sum += by_match(k)\nend\nprint(get bad, get sum)\n",
    );
    // 500 = 11 rounds of 43 keys + 27: 11 * (780 - 3) + (0 + … + 26) = 8898.
    assert_eq!(run(&src), ["0 8898"]);
}
