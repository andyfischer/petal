//! `petal migrate-old-code` at the library level ([`petal::migrate`]): one
//! fixture per migration, run on its own (`--only`) and with every other
//! migration on; every fixture is idempotent, compiles before and after, and
//! leaves current syntax alone. The CLI half (`--check`, the directory walk,
//! `old-imports`) is cli/tests/migrate_old_code_cli.rs.

use petal::env::Env;
use petal::migrate::{
    AT_REBIND, COLOUR_LITERALS, CONFIG_LET_DEPRECATED, CONST_DECL_DEPRECATED, ELSE_IF_NESTING, ELSIF_TO_ELSE_IF,
    END_LINES, EXPORT_TO_PUB, HASH_COMMENTS, MIGRATIONS, MigrateError, MigrateOptions, MigrateOutcome,
    STATE_KEY_DEPRECATED, migrate_source, migrate_source_checked,
};

/// (migration, old source, migrated source). Each old source compiles (or,
/// for a removed form, compiled before its removal), and is written only in
/// the one old form its migration rewrites.
const FIXTURES: &[(&str, &str, &str)] = &[
    // The old test of the operator (test/at-arg-update), migrated.
    (
        AT_REBIND,
        "fn double(x)\n  x * 2\nend\nfn inc(x)\n  x + 1\nend\n\
         let a = 5\ndouble(@a)\nprint(a) # 10\n\
         let b = 3\nlet r = inc(double(@b))\nprint(b, r) # 6 7\n\
         let c = 4\ndouble(@c)\ndouble(@ c)\nprint(c) # 16\n",
        "fn double(x)\n  x * 2\nend\nfn inc(x)\n  x + 1\nend\n\
         let a = 5\na = double(a)\nprint(a) # 10\n\
         let b = 3\nb = double(b)\nlet r = inc(b)\nprint(b, r) # 6 7\n\
         let c = 4\nc = double(c)\nc = double(c)\nprint(c) # 16\n",
    ),
    // Color tokens only: the string, the raw GLSL and the comments keep their
    // `#`; B16's four color positions (a field value, an argument, after
    // `=`, a string hole) all respell.
    (
        COLOUR_LITERALS,
        "let tint = #f80 /* was #fff */ # was #f80, once\nlet theme = {bg: #10203040, ink: #FFFFFF}\n\
         let css = \"#ff8800\"\nlet glsl = \"\"\"\n#define K 1\nvec3 c = #abc;\n\"\"\"\n\
         fn name(c)\n  if c == #f80 then \"orange\" else \"{#fff.r}\" end\nend\n\
         print(name(#f80), tint.r, theme.bg.a, css, [#000, #abcd][1].a)\n",
        "let tint = :f80 /* was #fff */ # was #f80, once\nlet theme = {bg: :10203040, ink: :FFFFFF}\n\
         let css = \"#ff8800\"\nlet glsl = \"\"\"\n#define K 1\nvec3 c = #abc;\n\"\"\"\n\
         fn name(c)\n  if c == :f80 then \"orange\" else \"{:fff.r}\" end\nend\n\
         print(name(:f80), tint.r, theme.bg.a, css, [:000, :abcd][1].a)\n",
    ),
    // Trivia only: the `//` in the string and the `/* */` comment stay, a
    // directive keeps its meaning, and the comment line in element children
    // becomes a hole (a `#` line there would be text).
    (
        HASH_COMMENTS,
        "// header\n/// doc\nlet a = 1 //one\nlet n = 2 // two\n\
         let url = \"http://x\" /* kept // as is */\n\
         let v = <col>\n  // the rows\n  <row n={a} /> // text\n</col>\n\
         print(a + n, url, v.children.length) //3\n",
        "# header\n## doc\nlet a = 1 # one\nlet n = 2 # two\n\
         let url = \"http://x\" /* kept // as is */\n\
         let v = <col>\n  {/* the rows */}\n  <row n={a} /> // text\n</col>\n\
         print(a + n, url, v.children.length) # 3\n",
    ),
    // An `if` nested in an `else` on the `else`'s line: alone there, it
    // joins the chain and its own `end` goes (a line of its own, or a word
    // of a one-liner); with a statement after it, it moves under the `else`
    // and stays nested. The GLSL and the comment are not tokens.
    (
        ELSE_IF_NESTING,
        "fn sign(n)\n  if n > 0 then\n    1\n  else if n < 0 then\n    2\n  else\n    3\n  end\n  end\nend\n\
         fn grade(n)\n  if n > 8 then \"a\" else if n > 5 then \"b\" else \"c\" end end\nend\n\
         fn step(n)\n  if n > 1 then\n    n\n  else if n > 0 then\n      print(\"one\")\n    end\n    n + 1\n  end\nend\n\
         let glsl = \"\"\"\nif (a) {} else if (b) {} } }\n\"\"\"\n\
         print(sign(0 - 3), grade(6), step(1), glsl.length) # else if x end end\n",
        "fn sign(n)\n  if n > 0 then\n    1\n  else if n < 0 then\n    2\n  else\n    3\n  end\nend\n\
         fn grade(n)\n  if n > 8 then \"a\" else if n > 5 then \"b\" else \"c\" end\nend\n\
         fn step(n)\n  if n > 1 then\n    n\n  else\n    if n > 0 then\n      print(\"one\")\n    end\n    n + 1\n  end\nend\n\
         let glsl = \"\"\"\nif (a) {} else if (b) {} } }\n\"\"\"\n\
         print(sign(0 - 3), grade(6), step(1), glsl.length) # else if x end end\n",
    ),
    // The keyword token only: a field and a record key named `elsif`, the
    // string and the comment stay.
    (
        ELSIF_TO_ELSE_IF,
        "fn sign(n)\n  if n > 0 then\n    1\n  elsif n < 0 then # neg\n    2\n  else\n    3\n  end\nend\n\
         let r = {elsif: 1}\nlet s = \"elsif\" # elsif\n\
         print(sign(0 - 3), if r.elsif > 1 then 1 elsif r.elsif > 0 then 2 else 3 end, s)\n",
        "fn sign(n)\n  if n > 0 then\n    1\n  else if n < 0 then # neg\n    2\n  else\n    3\n  end\nend\n\
         let r = {elsif: 1}\nlet s = \"elsif\" # elsif\n\
         print(sign(0 - 3), if r.elsif > 1 then 1 else if r.elsif > 0 then 2 else 3 end, s)\n",
    ),
    (
        EXPORT_TO_PUB,
        "export fn double(x)\n  x * 2\nend\nexport let limit = 10 # max\nprint(double(limit))\n",
        "pub fn double(x)\n  x * 2\nend\npub let limit = 10 # max\nprint(double(limit))\n",
    ),
    (
        CONST_DECL_DEPRECATED,
        "const BASE = 10\npub const TABLE = map([1, 2], fn(x) -> x * BASE) # computed\n\
         const P = {\n  x: 1,\n  y: [2, 3],\n}\nprint(TABLE, P)\n",
        "let BASE = const(10)\npub let TABLE = const(map([1, 2], fn(x) -> x * BASE)) # computed\n\
         let P = const({\n  x: 1,\n  y: [2, 3],\n})\nprint(TABLE, P)\n",
    ),
    (
        CONFIG_LET_DEPRECATED,
        "config let SPEED = 4.0   # px\nconfig let G = tunable(9.8, 0, 20)\nprint(SPEED, G)\n",
        "let SPEED = tunable(4.0) # px\nlet G = tunable(9.8, 0, 20)\nprint(SPEED, G)\n",
    ),
    (
        STATE_KEY_DEPRECATED,
        "fn width(text)\n  state(text) w = text.length * 8\n  w\nend\nprint(width(\"hi\"))\n",
        "fn width(text)\n  memo(text) w = text.length * 8\n  w\nend\nprint(width(\"hi\"))\n",
    ),
    // Every block construct, closed by its `end`: the `end` lines go, `end)`
    // keeps its bracket, and the file is formatted first (the `fn` body
    // was indented by four). What stays: the `end` of a one-liner, `end,`
    // between two block lambdas, `end |> f`, and the `end` of an empty
    // block. A comment after a removed `end` keeps a line at the block's
    // column; the `end` text in the string and the field named `end` are
    // not block closers.
    (
        END_LINES,
        "enum Shape\n  dot,\n  box(w),\nend\nclass P\n  x: int,\nend\n\
         fn area(s)\n    match s\n      when Shape.dot -> 0\n      when Shape.box(w) ->\n        let a = w * w\n        a\n    end\nend\n\
         fn sign(n)\n  if n > 0 then\n    1\n  else if n < 0 then\n    0 - 1\n  else\n    0\n  end # sign\nend\n\
         let fs = [\n  fn(x)\n    x + 1\n  end,\n  fn(y)\n    y * 2\n  end\n]\n\
         let total = reduce([1, 2], 0, fn(acc, v)\n  acc + v\nend)\n\
         let joined = for f in fs do\n  str(f(1))\nend |> join(\" \")\n\
         fn nothing()\nend\n\
         let span = {start: 1, end: 2}\nlet i = 0\nwhile i < 2 do\n  i += 1\nend\n\
         test \"t\" do\n  expect_eq(1, 1)\nend\n\
         print(area(Shape.box(2)), sign(0 - 3), if i > 1 then \"end\" else \"\" end, total, joined, span.end, nothing())\n",
        "enum Shape\n  dot,\n  box(w),\nclass P\n  x: int,\n\
         fn area(s)\n  match s\n    when Shape.dot -> 0\n    when Shape.box(w) ->\n      let a = w * w\n      a\n\
         fn sign(n)\n  if n > 0 then\n    1\n  else if n < 0 then\n    0 - 1\n  else\n    0\n    # sign\n\
         let fs = [\n  fn(x)\n    x + 1\n  end,\n  fn(y)\n    y * 2\n]\n\
         let total = reduce([1, 2], 0, fn(acc, v)\n  acc + v\n)\n\
         let joined = for f in fs do\n  str(f(1))\nend |> join(\" \")\n\
         fn nothing()\nend\n\
         let span = {start: 1, end: 2}\nlet i = 0\nwhile i < 2 do\n  i += 1\n\
         test \"t\" do\n  expect_eq(1, 1)\n\
         print(area(Shape.box(2)), sign(0 - 3), if i > 1 then \"end\" else \"\" end, total, joined, span.end, nothing())\n",
    ),
];

/// Run the migrations named in `only` over `src`, or, with `None`, every
/// migration that rewrites an old form: all but `end-lines`, which lays the
/// whole file out and has its own tests below ([`migrate_all`] runs it too).
fn migrate_with(src: &str, only: Option<&[&str]>) -> MigrateOutcome {
    let opts = MigrateOptions {
        only: only.map(|o| o.iter().map(|s| s.to_string()).collect()),
        except: if only.is_none() { vec![END_LINES.to_string()] } else { Vec::new() },
        ..Default::default()
    };
    migrate_source(src, &opts).unwrap_or_else(|e| panic!("migrate: {e}\n{src}"))
}

/// Every text migration, as `petal migrate-old-code` runs them.
fn migrate_all(src: &str) -> MigrateOutcome {
    let out = migrate_source(src, &MigrateOptions::default()).unwrap_or_else(|e| panic!("migrate: {e}\n{src}"));
    assert_eq!(out.skipped, [], "{src}");
    out
}

/// Whether `src` compiles; a removed form is checked as the operator's
/// desugaring would have seen it, which for `@` is "with the `@`s read as
/// nothing" (the program the old compiler checked had the same names).
fn compiled_before(rule: &str, src: &str) -> bool {
    if rule == AT_REBIND {
        !compiles(src) && compiles(&src.replace('@', " "))
    } else if rule == COLOUR_LITERALS {
        // `#f80` no longer lexes; it was the color `:f80` lexes to.
        !compiles(src) && compiles(&petal::migrate::respell_colour_literals(src).unwrap())
    } else if rule == ELSE_IF_NESTING {
        // `else if … end end` no longer parses; it was an `else` block
        // holding the `if`, which is what a line break after `else` still is.
        !compiles(src) && compiles(&nested_today(src))
    } else {
        compiles(src)
    }
}

/// `src` with every `else if` of its code (the fixtures keep other `else if`
/// text on lines that start with `if (` or hold a `#`) read the way it was
/// before `else if` was one token.
fn nested_today(src: &str) -> String {
    let broken = src
        .lines()
        .map(|l| {
            if l.starts_with("if (") || l.contains('#') {
                l.to_string()
            } else {
                l.replace("else if", "else\nif")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    // The `if` and the lines under it are now one level short of where a
    // nested block sits; the formatter indents the file by its `end`s.
    petal::fmt::format_source(&broken).unwrap_or(broken)
}

fn run_out(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_or_else(|e| panic!("{e}\n{src}"));
    env.take_output()
}

fn compiles(src: &str) -> bool {
    let mut env = Env::new();
    env.set_echo(false);
    env.load_program(src).is_ok()
}

fn deprecation_warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).expect("compiles");
    env.get_program(pid)
        .unwrap()
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .filter(|m| m.contains("petal migrate-old-code"))
        .collect()
}

#[test]
fn every_text_migration_has_a_fixture() {
    for m in MIGRATIONS.iter().filter(|m| !m.batch) {
        assert!(
            FIXTURES.iter().any(|(rule, _, _)| *rule == m.name),
            "no fixture for {}",
            m.name
        );
    }
}

#[test]
fn each_migration_rewrites_its_fixture_on_its_own_and_with_the_rest() {
    for (rule, before, after) in FIXTURES {
        assert!(compiled_before(rule, before), "{rule}: the old source does not compile");
        let only = migrate_with(before, Some(&[rule]));
        assert_eq!(only.output, *after, "{rule} (--only)");
        assert!(only.findings.iter().all(|f| f.rule == *rule && f.fixable), "{rule}: {:?}", only.findings);
        assert!(only.rewritten() > 0, "{rule}");
        if *rule != END_LINES {
            let all = migrate_with(before, None);
            assert_eq!(all.output, *after, "{rule} (every migration that rewrites an old form)");
        }
        // With `end-lines` too: what it makes of the migrated text.
        let ended = migrate_with(after, Some(&[END_LINES])).output;
        assert_eq!(migrate_all(before).output, ended, "{rule} (every migration)");
        assert!(compiles(&ended), "{rule}: the migrated source does not compile without its `end` lines");
        assert!(compiles(after), "{rule}: the migrated source does not compile");
        // The compiler warned about the old form, and points nowhere now.
        assert_eq!(deprecation_warnings(after), Vec::<String>::new(), "{rule}");
    }
}

#[test]
fn migrating_twice_changes_nothing() {
    for (rule, before, after) in FIXTURES {
        let once = migrate_with(before, None).output;
        let twice = migrate_with(&once, None);
        assert_eq!(twice.output, once, "{rule}");
        assert!(twice.findings.is_empty(), "{rule}: {:?}", twice.findings);
        assert_eq!(migrate_with(after, None).output, *after, "{rule}");
        let once = migrate_all(before).output;
        let twice = migrate_all(&once);
        assert_eq!(twice.output, once, "{rule}");
        assert!(twice.findings.is_empty(), "{rule}: {:?}", twice.findings);
    }
}

#[test]
fn another_migration_leaves_a_fixture_alone() {
    for (rule, before, _) in FIXTURES {
        // (`end-lines` is not "another migration" of an old form: it lays
        // out any file that has an `end` line.)
        for (other, _, _) in FIXTURES.iter().filter(|(o, _, _)| o != rule && *o != END_LINES) {
            if *rule == AT_REBIND || *rule == COLOUR_LITERALS || *rule == ELSE_IF_NESTING {
                // Without its own migration, an `@` file and an
                // `else if … end end` file do not parse, and a `#f80` file
                // does not lex.
                let opts = MigrateOptions {
                    only: Some(vec![other.to_string()]),
                    ..Default::default()
                };
                assert!(migrate_source(before, &opts).is_err(), "--only {other}");
                continue;
            }
            let out = migrate_with(before, Some(&[other]));
            assert_eq!(out.output, *before, "--only {other} touched the {rule} fixture");
        }
    }
}

#[test]
fn every_old_form_in_one_file() {
    let src = "export const N = 4\n\
               pub config let SPEED = 2.5\n\
               fn w(t)\n  state(t) x = t * 2\n  x\nend\n\
               print(N + SPEED + w(1))\n";
    assert_eq!(deprecation_warnings(src).len(), 3, "{:?}", deprecation_warnings(src));
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "pub let N = const(4)\n\
         pub let SPEED = tunable(2.5)\n\
         fn w(t)\n  memo(t) x = t * 2\n  x\nend\n\
         print(N + SPEED + w(1))\n"
    );
    let rules: Vec<&str> = out.findings.iter().map(|f| f.rule).collect();
    assert_eq!(rules, [EXPORT_TO_PUB, CONST_DECL_DEPRECATED, CONFIG_LET_DEPRECATED, STATE_KEY_DEPRECATED]);
    assert_eq!(deprecation_warnings(&out.output), Vec::<String>::new());
    assert_eq!(migrate_with(&out.output, None).output, out.output);
}

#[test]
fn current_syntax_is_left_exactly_as_written() {
    // Unformatted, too: the tool does not reformat.
    let src = "pub   let N = const( 4 )\nlet  S = tunable(1)\nfn f(k)\n  memo(k) v = k\n    v\nend\nprint(N+S+f(1))\n";
    let out = migrate_with(src, None);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn a_site_that_needs_a_hand_edit_is_reported_and_left() {
    let src = "fn health(id, d)\n  state(id) hp = 100\n  hp -= d\n  hp\nend\nprint(health(1, 2))\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, src);
    assert_eq!((out.rewritten(), out.left()), (0, 1));
    assert_eq!(out.findings[0].rule, STATE_KEY_DEPRECATED);
}

#[test]
fn an_ignore_comment_keeps_the_old_form_and_silences_the_warning() {
    let src = "# petal-lint-ignore const-decl-deprecated -- a fixture of the old form\n\
               const A = 1\nconst B = 2\nprint(A + B)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "# petal-lint-ignore const-decl-deprecated -- a fixture of the old form\n\
         const A = 1\nlet B = const(2)\nprint(A + B)\n"
    );
    assert_eq!(deprecation_warnings(&out.output), Vec::<String>::new());
}

#[test]
fn a_file_that_does_not_parse_is_an_error() {
    assert!(migrate_source("const = \n", &MigrateOptions::default()).is_err());
}

#[test]
fn a_file_that_does_not_compile_is_still_migrated() {
    // The import resolves nowhere here (a module of some app): the compile
    // gate has nothing to hold the rewrite to, and the migration applies.
    let src = "import no_such_package_here\nconst N = 1\nprint(N)\n";
    assert!(!compiles(src));
    assert_eq!(
        migrate_with(src, None).output,
        "import no_such_package_here\nlet N = const(1)\nprint(N)\n"
    );
}

#[test]
fn the_registry_lists_old_imports_last_as_a_batch_migration() {
    let last = MIGRATIONS.last().unwrap();
    assert_eq!(last.name, petal::migrate::OLD_IMPORTS);
    assert!(last.batch);
    assert!(MIGRATIONS[..MIGRATIONS.len() - 1].iter().all(|m| !m.batch));
}

// ---------------------------------------------------------------------------
// `at-rebind`: the removed `f(@x)` operator
// ---------------------------------------------------------------------------

#[test]
fn an_at_rebind_is_a_parse_error_that_names_the_tool() {
    let mut env = Env::new();
    env.set_echo(false);
    let err = format!("{}", env.load_program("let a = [1]\nappend(@a, 2)\n").unwrap_err());
    assert!(err.contains("`@a` is not an expression"), "{err}");
    assert!(err.contains("Write `a = f(a)` instead of `f(@a)`"), "{err}");
    assert!(err.contains("petal migrate-old-code"), "{err}");
    // Where a statement starts, `@` opens a declaration annotation
    // (docs/proposals/annotation-syntax.md), and the error says what one is.
    let err = format!("{}", env.load_program("@ 1\n").unwrap_err());
    assert!(err.contains("`@` must be followed by an annotation name"), "{err}");
}

#[test]
fn an_at_rebind_migrates_to_the_program_it_desugared_to() {
    // The values the old operator's test expected: 10, 6 7, 16.
    let (_, _, after) = FIXTURES.iter().find(|(r, _, _)| *r == AT_REBIND).unwrap();
    assert_eq!(run_out(after), ["10", "6 7", "16"]);
}

#[test]
fn an_at_rebind_lifts_within_each_block_and_keeps_its_indentation() {
    let src = "fn double(x)\n  x * 2\nend\nfn add(x, y)\n  x + y\nend\n\
               fn go(a, b)\n  if a > 0 then double(@a) end\n  if b > 0 then\n    \
               let r = add(double(@a), @b) # note\n    print(r)\n  end\n  \
               for i in [1, 2] do\n    a = add(double(@a), 1)\n  end\n  \
               print(\"@a stays {a}\")\n  [a, b]\nend\nprint(go(1, 2))\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "fn double(x)\n  x * 2\nend\nfn add(x, y)\n  x + y\nend\n\
         fn go(a, b)\n  if a > 0 then a = double(a) end\n  if b > 0 then\n    \
         a = double(a)\n    b = add(a, b)\n    let r = b # note\n    print(r)\n  end\n  \
         for i in [1, 2] do\n    a = double(a)\n    a = add(a, 1)\n  end\n  \
         print(\"@a stays {a}\")\n  [a, b]\nend\nprint(go(1, 2))\n"
    );
    assert_eq!((out.rewritten(), out.left()), (3, 0));
    assert_eq!(run_out(&out.output), ["6", "@a stays 19", "[19, 6]"]);
    assert_eq!(migrate_with(&out.output, None).output, out.output);
}

#[test]
fn an_at_rebind_the_operator_could_not_lift_is_left_for_a_hand_edit() {
    for (src, why) in [
        ("fn add(x, y)\n  x + y\nend\nlet a = 1\nlet b = 2\nadd(@a, @b)\n", "two in one call"),
        (
            "fn double(x)\n  x * 2\nend\nlet a = 1\nwhile double(@a) < 10 do\n  print(a)\nend\n",
            "a while condition",
        ),
        (
            "fn double(x)\n  x * 2\nend\nlet a = 1\nlet m = match a\n  when 1 -> double(@a)\n  \
             when _ -> 0\nend\n",
            "a match arm",
        ),
        (
            "fn double(x)\n  x * 2\nend\nlet b = 1\nif true then print(double(@b)) end\n",
            "an assignment that would share its line",
        ),
    ] {
        let out = migrate_with(src, None);
        assert_eq!(out.output, src, "{why}");
        assert!(out.left() > 0 && out.rewritten() == 0, "{why}: {:?}", out.findings);
        assert!(out.findings.iter().all(|f| f.rule == AT_REBIND && !f.fixable), "{why}");
    }
}

#[test]
fn an_at_rebind_and_a_deprecated_form_migrate_together() {
    let src = "fn double(x)\n  x * 2\nend\nexport let n = 2\nconst K = 3\nlet a = 1\ndouble(@a)\nprint(a + n + K)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "fn double(x)\n  x * 2\nend\npub let n = 2\nlet K = const(3)\nlet a = 1\na = double(a)\nprint(a + n + K)\n"
    );
    let rules: Vec<&str> = out.findings.iter().map(|f| f.rule).collect();
    assert_eq!(rules, [EXPORT_TO_PUB, CONST_DECL_DEPRECATED, AT_REBIND]);
    assert_eq!(run_out(&out.output), ["7"]);
}

#[test]
fn an_at_in_a_string_or_a_comment_is_not_a_rebind() {
    let src = "let a = \"@a\" # f(@a)\nprint(a)\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, src);
    assert!(out.findings.is_empty());
}

#[test]
fn a_declaration_annotation_is_not_a_rebind() {
    // `@tunable`, `@rigid` and `@const` annotate a declaration
    // (docs/proposals/annotation-syntax.md); they never were rebinds, since
    // those names cannot be bound. migrate-old-code leaves them alone, also
    // beside a real rebind and a deprecated form it rewrites.
    let src = "@tunable(0, 10) let S = 4.0\n@const\npub let T = [1, 2]\n@rigid state A = 1\n\
               fn f(x)\n  @tunable let g = x\n  g\nend\nprint(S, T, A, f(1))\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);

    let src = "fn double(x)\n  x * 2\nend\nlet a = 1\n@tunable(0, 9) let k = 2\n\
               @rigid const G = 9.8\ndouble(@a)\nprint(a, k, G)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "fn double(x)\n  x * 2\nend\nlet a = 1\n@tunable(0, 9) let k = 2\n\
         @const @rigid let G = 9.8\na = double(a)\nprint(a, k, G)\n"
    );
    assert_eq!(run_out(&out.output), ["2 2 9.8"]);
}

// ---------------------------------------------------------------------------
// `colour-literals`: `#f80` is written `:f80`
// ---------------------------------------------------------------------------

#[test]
fn a_hash_colour_is_an_error_that_names_the_tool_and_the_new_spelling() {
    let mut env = Env::new();
    env.set_echo(false);
    let err = format!("{}", env.load_program("let a = #f80\nlet b = :fff\nprint(a.r + b.r)\n").unwrap_err());
    assert!(err.contains("`#f80` is no longer a color literal; write `:f80`"), "{err}");
    assert!(err.contains("petal migrate-old-code") && err.contains("[colour-literals]"), "{err}");
    // Never a comment: the stale literal after `??` would otherwise leave
    // `a ??` to continue onto the next line, a different program.
    assert!(env.load_program("let a = nil\nlet c = a ?? #fff\nprint(c)\n").is_err());
}

#[test]
fn a_migrated_colour_is_the_same_program() {
    let (_, before, after) = FIXTURES.iter().find(|(r, _, _)| *r == COLOUR_LITERALS).unwrap();
    assert_eq!(migrate_with(before, None).output, *after);
    assert_eq!(run_out(after), ["orange 255 64 #ff8800 221"]);
}

/// B16: where `:` is a separator the migration never produces a colour, and
/// where a colour is written the separator before it is kept.
#[test]
fn colour_separators_are_kept_and_only_colour_tokens_respell() {
    let src = "let r = {x:100, tint:#000, edge: #abc}\nfn f(a:int, c = #fff) a + c.r end\nprint(r.x, f(1, c:#f80))\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "let r = {x:100, tint::000, edge: :abc}\nfn f(a:int, c = :fff) a + c.r end\nprint(r.x, f(1, c::f80))\n"
    );
    assert_eq!(run_out(&out.output), ["100 256"]);
    assert_eq!(out.count(COLOUR_LITERALS), 4);
}

#[test]
fn a_colour_directly_after_a_value_is_reported_and_left() {
    // `fn bg() :fff end` would read `:` as a separator, so this one is a
    // hand edit (`(:fff)`); the rest of the file still migrates.
    let src = "fn bg() #fff end\nlet ink = #000\nprint(bg().r + ink.r)\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, "fn bg() #fff end\nlet ink = :000\nprint(bg().r + ink.r)\n");
    assert_eq!((out.rewritten(), out.left()), (1, 1));
    assert!(out.findings[0].message.contains("(:fff)"), "{:?}", out.findings);
    assert_eq!(run_out("fn bg() (:fff) end\nprint(bg().r)\n"), ["255"]);
}

#[test]
fn a_colour_ignore_comment_keeps_the_hash() {
    // The site stays as written (it no longer compiles: the form is
    // removed), and the rest of the file is still migrated up to the first
    // step that needs the file to lex.
    let src = "let a = #f80 // petal-lint-ignore colour-literals\nexport let b = #fff\nprint(a.r + b.r)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "let a = #f80 # petal-lint-ignore colour-literals\nexport let b = :fff\nprint(a.r + b.r)\n"
    );
    // What is left is the lexer's error, which names the migration.
    let err = migrate_source(&out.output, &MigrateOptions::default()).err().unwrap();
    assert!(err.contains("[colour-literals]"), "{err}");
}

#[test]
fn colours_and_other_old_forms_migrate_together() {
    let src = "export const INK = #102030\nconfig let TINT = #f80 // warm\ndraw(@INK)\nprint(INK.r + TINT.g)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "pub let INK = const(:102030)\nlet TINT = tunable(:f80) # warm\nINK = draw(INK)\nprint(INK.r + TINT.g)\n"
    );
    assert_eq!(deprecation_warnings(&out.output), Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// `hash-comments`: `// note` is written `# note`
// ---------------------------------------------------------------------------

/// The whole legacy surface in one unmigrated file, as a downstream project
/// has it: `#f80` colors, `//` comments (one of them naming a color), an
/// `@` rebind and deprecated declarations. One run migrates all of it.
#[test]
fn a_fully_unmigrated_file_migrates_in_one_run() {
    let src = "// theme\nexport const INK = #102030 //ink\nfn dim(c)\n  // was #fff\n  {...c, r: 0}\nend\n\
               let tint = #f80\ndim(@tint)\nprint(INK.r + tint.r + tint.g) // 152\n";
    assert!(!compiles(src));
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "# theme\npub let INK = const(:102030) # ink\nfn dim(c)\n  # was #fff\n  {...c, r: 0}\nend\n\
         let tint = :f80\ntint = dim(tint)\nprint(INK.r + tint.r + tint.g) # 152\n"
    );
    assert_eq!(run_out(&out.output), ["152"]);
    assert_eq!(deprecation_warnings(&out.output), Vec::<String>::new());
    let again = migrate_with(&out.output, None);
    assert_eq!(again.output, out.output);
    assert!(again.findings.is_empty(), "{:?}", again.findings);
}

#[test]
fn a_migrated_comment_file_is_the_same_program() {
    let (_, before, after) = FIXTURES.iter().find(|(r, _, _)| *r == HASH_COMMENTS).unwrap();
    assert_eq!(run_out(before), run_out(after));
    assert_eq!(run_out(after), ["3 http://x 2"]);
}

#[test]
fn slash_comments_are_reported_once_per_file_and_children_lines_each() {
    let src = "// a\n// b\nlet v = <p>\n  // c\n  <q />\n  // d\n</p> // e\n";
    let out = migrate_with(src, Some(&[HASH_COMMENTS]));
    let found: Vec<(usize, bool)> = out.findings.iter().map(|f| (f.line, f.fixable)).collect();
    assert_eq!(found, [(1, true), (4, true), (6, true)]);
    assert!(out.findings[0].message.contains("3 in this file"), "{}", out.findings[0].message);
    assert_eq!(out.output, "# a\n# b\nlet v = <p>\n  {/* c */}\n  <q />\n  {/* d */}\n</p> # e\n");
}

#[test]
fn a_children_comment_between_two_text_lines_is_reported_and_left() {
    let src = "let v = <p>\n  one\n  // note\n  two\n</p>\nprint(v.children) // x\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, "let v = <p>\n  one\n  // note\n  two\n</p>\nprint(v.children) # x\n");
    assert_eq!((out.rewritten(), out.left()), (1, 1));
    assert_eq!(run_out(src), run_out(&out.output));
}

#[test]
fn a_comment_ignore_keeps_the_slashes() {
    let src = "// petal-lint-ignore-file hash-comments\nlet a = 1 // one\nprint(a)\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, src);
    assert_eq!(out.rewritten(), 0);
}

#[test]
fn glsl_in_a_raw_string_keeps_its_comments_and_directives() {
    let src = "let frag = \"\"\"\n#version 300 es\n// glsl\n#define K 1 // k\nif (a) {} else if (b) {}\n\"\"\" // shader\nprint(frag.length)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "let frag = \"\"\"\n#version 300 es\n// glsl\n#define K 1 // k\nif (a) {} else if (b) {}\n\"\"\" # shader\nprint(frag.length)\n"
    );
    assert_eq!(run_out(src), run_out(&out.output));
}

// ---------------------------------------------------------------------------
// `else if` (docs/tasks/pythonic-syntax.md §3.6, B17)
// ---------------------------------------------------------------------------

/// B17: read as a chain, the old nesting form has an `end` too many. That is
/// an error, never a different program, and it names the migration.
#[test]
fn a_nested_else_if_is_a_stray_end_error_that_names_the_tool() {
    let mut env = Env::new();
    env.set_echo(false);
    for (src, at) in [
        ("let a = 1\nlet x = if a > 1 then 1 else if a > 0 then 2 else 3 end end\nprint(x)\n", "[line 2, column 57]"),
        ("let a = 1\nif a > 1 then\n  print(1)\nelse if a > 0 then\n  print(2)\nend\nend\n", "[line 7, column 1]"),
    ] {
        let err = env.load_program(src).err().expect("an `end` too many").to_string();
        assert!(err.contains("Unexpected token: `end`"), "{err}");
        assert!(
            err.contains(
                "`else if` continues its `if` chain, and the whole chain takes one `end`: if the `end` at line"
            ),
            "{err}"
        );
        assert!(err.contains("run `petal migrate-old-code` [else-if-nesting]"), "{err}");
        assert!(err.contains(at), "{err}");
    }
    // Inside a function the surplus `end` would close the function, and does
    // not line up with it: the error is at that `end`, with the hint.
    let src = "fn f(a)\n  if a > 1 then\n    1\n  else if a > 0 then\n    2\n  end\n  end\n  3\nend\n";
    let err = env.load_program(src).err().expect("an `end` too many").to_string();
    assert!(err.contains("if the `end` at line 7 closed"), "{err}");
    assert!(err.contains("does not line up with the `fn` block opened at line 1"), "{err}");
    assert!(err.contains("[line 7, column 3]"), "{err}");
    // An unrelated surplus `end` gets no such hint.
    let err = env.load_program("let a = 1\nend\n").err().expect("stray end").to_string();
    assert!(!err.contains("else if"), "{err}");
}

#[test]
fn a_migrated_nesting_file_is_the_program_it_was() {
    let (_, before, after) = FIXTURES.iter().find(|(r, _, _)| *r == ELSE_IF_NESTING).unwrap();
    assert_eq!(run_out(&nested_today(before)), run_out(after));
    assert_eq!(run_out(after), ["one", "2 b 2 30"]);
}

#[test]
fn a_migrated_elsif_file_is_the_same_program() {
    let (_, before, after) = FIXTURES.iter().find(|(r, _, _)| *r == ELSIF_TO_ELSE_IF).unwrap();
    assert_eq!(run_out(before), run_out(after));
    assert_eq!(run_out(after), ["2 2 elsif"]);
}

/// `else` at the end of a line with `if` on the next is an `else` block
/// holding an `if`, as it always was: not a site, and not a chain.
#[test]
fn an_if_on_the_line_after_else_stays_nested() {
    let src = "fn f(a)\n  if a > 1 then\n    1\n  else\n    if a > 0 then\n      2\n    end\n    3\n  end\nend\nprint(f(1))\n";
    let out = migrate_with(src, None);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
    assert_eq!(run_out(src), ["3"]);
}

/// Chains of the old nesting form, three deep, and one inside another's
/// branch: each nested `if` gives up its `end`.
#[test]
fn nested_else_ifs_several_deep_become_one_chain() {
    let src = "fn f(a)\n  if a > 3 then\n    4\n  else if a > 2 then\n    3\n  else if a > 1 then\n    \
               if a > 5 then 9 else if a > 4 then 8 else 2 end end\n  else\n    1\n  end\n  end\n  end\nend\n\
               print(f(4), f(3), f(2), f(1))\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "fn f(a)\n  if a > 3 then\n    4\n  else if a > 2 then\n    3\n  else if a > 1 then\n    \
         if a > 5 then 9 else if a > 4 then 8 else 2 end\n  else\n    1\n  end\nend\n\
         print(f(4), f(3), f(2), f(1))\n"
    );
    assert_eq!(out.count(ELSE_IF_NESTING), 3);
    assert_eq!(run_out(&out.output), ["4 3 2 1"]);
    assert_eq!(run_out(&nested_today(src)), ["4 3 2 1"]);
}

/// A file that still needs every lexical migration: the nesting form is
/// read through `#f80` colors, `//` comments, `elsif` and an `@` rebind.
#[test]
fn a_nested_else_if_migrates_with_every_other_old_form() {
    let src = "// tints\nfn pick(n)\n  if n > 2 then #fff else if n > 1 then #000 else #f80 end end // nested\nend\n\
               fn kind(n)\n  if n > 1 then 1 elsif n > 0 then 2 else 3 end\nend\n\
               fn inc(x)\n  x + 1\nend\nlet k = 0\ninc(@k)\nprint(pick(1).r + kind(k)) // 257\n";
    assert!(!compiles(src));
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "# tints\nfn pick(n)\n  if n > 2 then :fff else if n > 1 then :000 else :f80 end # nested\nend\n\
         fn kind(n)\n  if n > 1 then 1 else if n > 0 then 2 else 3 end\nend\n\
         fn inc(x)\n  x + 1\nend\nlet k = 0\nk = inc(k)\nprint(pick(1).r + kind(k)) # 257\n"
    );
    assert_eq!(run_out(&out.output), ["257"]);
    let again = migrate_with(&out.output, None);
    assert_eq!(again.output, out.output);
    assert!(again.findings.is_empty(), "{:?}", again.findings);
}

/// `else if` text that is not code: GLSL in a raw string (the cheesecake
/// shaders), a string and a comment are left exactly as written.
#[test]
fn else_if_text_outside_code_is_untouched() {
    let src = "let frag = \"\"\"\nif (a) {} else if (b) {} elsif\n\"\"\"\nlet s = \"else if c then 1 end end\"\n\
               # else if c then 1 end end; elsif\nprint(frag.length + s.length)\n";
    let out = migrate_with(src, None);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
}

// ---------------------------------------------------------------------------
// `end-lines`: indentation closes the block
// ---------------------------------------------------------------------------

fn end_lines(src: &str) -> MigrateOutcome {
    migrate_with(src, Some(&[END_LINES]))
}

/// The whole legacy surface of a downstream project in one unmigrated file:
/// `#f80` colors, `//` comments, `elsif`, an `else if … end end` nesting,
/// an `@` rebind, deprecated declarations, and `end`s with the rest of a
/// block left flush under an `else` (indentation was cosmetic). One run of
/// every migration gives the current syntax, and a second changes nothing.
#[test]
fn a_fully_legacy_file_migrates_to_the_end_less_form_in_one_run() {
    let src = "// theme\nexport const INK = #102030 //ink\n\
               fn grade(n)\n  if n > 8 then\n    \"a\"\n  elsif n > 5 then\n    \"b\"\n  else if n > 3 then\n    \"c\"\n  else\n    \"d\"\n  end\n  end\nend\n\
               fn dim(c)\n  if c.r > 200 then\n    return c\n  else\n  // was #fff\n  let d = {...c, r: 0}\n  d\n  end\nend\n\
               let tint = #f80\ndim(@tint)\nprint(INK.r + tint.r + tint.g, grade(4)) // 407 c\n";
    assert!(!compiles(src));
    let out = migrate_all(src);
    assert_eq!(
        out.output,
        "# theme\npub let INK = const(:102030) # ink\n\
         fn grade(n)\n  if n > 8 then\n    \"a\"\n  else if n > 5 then\n    \"b\"\n  else if n > 3 then\n    \"c\"\n  else\n    \"d\"\n\
         fn dim(c)\n  if c.r > 200 then\n    return c\n  else\n    # was #fff\n    let d = {...c, r: 0}\n    d\n\
         let tint = :f80\ntint = dim(tint)\nprint(INK.r + tint.r + tint.g, grade(4)) # 407 c\n"
    );
    assert_eq!(run_out(&out.output), ["407 c"]);
    assert_eq!(deprecation_warnings(&out.output), Vec::<String>::new());
    assert_eq!((out.ends_removed, out.ends_kept), (4, 0));
    let rules: Vec<&str> = out.findings.iter().map(|f| f.rule).collect();
    for rule in [COLOUR_LITERALS, HASH_COMMENTS, ELSE_IF_NESTING, ELSIF_TO_ELSE_IF, AT_REBIND, EXPORT_TO_PUB, END_LINES] {
        assert!(rules.contains(&rule), "{rule}: {rules:?}");
    }
    let again = migrate_all(&out.output);
    assert_eq!(again.output, out.output);
    assert!(again.findings.is_empty(), "{:?}", again.findings);
}

#[test]
fn end_lines_is_the_same_program_and_runs_the_same() {
    for (_, _, after) in FIXTURES {
        let out = end_lines(after);
        let shape = |s: &str| petal::rewrite::ast_shape(&petal::rewrite::parse_ast(s).unwrap().1).unwrap();
        assert_eq!(shape(after), shape(&out.output), "{after}");
        assert_eq!(run_out(after), run_out(&out.output), "{after}");
        // A fixed point, of the migration and of the formatter.
        assert_eq!(end_lines(&out.output).output, out.output);
        if out.output != *after {
            assert_eq!(petal::fmt::format_source(&out.output).unwrap(), out.output);
        }
    }
}

/// Plan §6.5: deleting the `end` lines reloads as a layout-only change, so
/// `state` survives the migration of a running program.
#[test]
fn a_migrated_file_reloads_as_a_layout_only_change() {
    let (_, before, after) = FIXTURES.iter().find(|(rule, _, _)| *rule == END_LINES).unwrap();
    let diff = petal::source_diff::diff_source(before, after, petal::source_map::FileId(0));
    assert_eq!(diff.change.label(), "none");
    let src = "fn tick()\n  state n = 0\n  n += 1\n  if n > 1 then\n    print(n)\n  end\nend\ntick()\n";
    let out = end_lines(src);
    assert_eq!(out.output, "fn tick()\n  state n = 0\n  n += 1\n  if n > 1 then\n    print(n)\ntick()\n");
    assert_eq!(petal::source_diff::diff_source(src, &out.output, petal::source_map::FileId(0)).change.label(), "none");
}

#[test]
fn a_file_with_no_end_line_is_not_reformatted() {
    // Unformatted, with a one-liner's `end` and nothing else: nothing to do.
    let src = "let  a = if true then 1 else 2 end\nfn f(x)   x * 2\nprint( a , f(2) )\n";
    let out = migrate_all(src);
    assert_eq!(out.output, src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
}

#[test]
fn the_ends_the_program_needs_are_kept_and_counted() {
    // `;` after a one-line `then` needs its `end` (B3); a line that starts
    // with an operator would continue the block's last statement (B13); an
    // empty block has nothing for indentation to close; `end,` and
    // `end.f()` have code after them.
    let src = "fn f(c)\n  if c then print(1); print(2)\n  end\n  let s = for x in [1, 2] do\n    x\n  end\n  ++ [3]\n  s\nend\n\
               fn g()\nend\n\
               let n = [1, 2].map(fn(x)\n  x + 1\nend).length\n\
               f(true)\nprint(n)\n";
    let out = end_lines(src);
    assert_eq!(
        out.output,
        "fn f(c)\n  if c then print(1); print(2)\n  end\n  let s = for x in [1, 2] do\n    x\n  end\n  ++ [3]\n  s\n\
         fn g()\nend\n\
         let n = [1, 2].map(fn(x)\n  x + 1\n).length\n\
         f(true)\nprint(n)\n"
    );
    assert_eq!((out.ends_removed, out.ends_kept), (2, 3));
    assert_eq!(run_out(src), run_out(&out.output));
    assert_eq!(end_lines(&out.output).output, out.output);
}

#[test]
fn a_block_in_a_bracket_keeps_the_bracket_at_the_statement_column() {
    let src = "fn show(items)\n  each_item(items, fn(it)\n    print(it)\n  end)\n  let v = [\n    for i in items do\n      i * 2\n    end\n  ]\n  v\nend\n\
               fn each_item(xs, f)\n  for x in xs do\n    f(x)\n  end\nend\nprint(show([1, 2]))\n";
    let out = end_lines(src);
    assert_eq!(
        out.output,
        "fn show(items)\n  each_item(items, fn(it)\n    print(it)\n  )\n  let v = [\n    for i in items do\n      i * 2\n  ]\n  v\n\
         fn each_item(xs, f)\n  for x in xs do\n    f(x)\nprint(show([1, 2]))\n"
    );
    assert_eq!(run_out(src), run_out(&out.output));
}

#[test]
fn an_ignore_file_comment_keeps_the_ends() {
    let src = "# petal-lint-ignore-file end-lines -- generated\nfn f()\n    1\nend\nprint(f())\n";
    let out = migrate_all(src);
    assert_eq!(out.output, src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
}

/// A file `end-lines` cannot convert keeps what the migrations before it
/// wrote, and is reported as skipped: here the formatter may not touch the
/// file, whose indentation disagrees with its `end`s.
#[test]
fn a_file_that_fails_the_end_lines_gate_keeps_the_other_migrations_and_is_reported() {
    let src = "# petal-fmt-ignore-file\nexport fn f(c)\n  if c then\n  1\n  else\n  2\n  end\nend\nprint(f(true))\n";
    let out = migrate_source(src, &MigrateOptions::default()).unwrap();
    assert_eq!(
        out.output,
        "# petal-fmt-ignore-file\npub fn f(c)\n  if c then\n  1\n  else\n  2\n  end\nend\nprint(f(true))\n"
    );
    assert_eq!(out.skipped.len(), 1, "{:?}", out.skipped);
    assert_eq!(out.skipped[0].migration, END_LINES);
    assert_eq!(out.ends_removed, 0);
    assert!(out.findings.iter().all(|f| f.rule == EXPORT_TO_PUB), "{:?}", out.findings);
}

#[test]
fn errors_say_whether_the_source_was_unreadable_or_a_gate_failed() {
    let err = migrate_source_checked("const = \n", &MigrateOptions::default()).err().unwrap();
    assert!(matches!(err, MigrateError::Unreadable(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// Fragments and Markdown (`--markdown`)
// ---------------------------------------------------------------------------

#[test]
fn a_fragment_that_does_not_parse_gets_the_token_migrations() {
    let src = "// elided\nfn f(x)\n  if x then ... \n  elsif y then #f80\n";
    assert!(migrate_source(src, &MigrateOptions::default()).is_err());
    let opts = MigrateOptions { fragment: true, ..Default::default() };
    let out = migrate_source(src, &opts).unwrap();
    assert!(out.fragment);
    assert_eq!(out.output, "# elided\nfn f(x)\n  if x then ... \n  else if y then :f80\n");
    // One that parses is migrated in full, fragment or not.
    let whole = migrate_source("fn f()\n  1\nend\n", &opts).unwrap();
    assert!(!whole.fragment);
    assert_eq!(whole.output, "fn f()\n  1\n");
    // One that does not lex is unreadable either way.
    assert!(matches!(
        migrate_source_checked("let s = \"open\n", &opts),
        Err(MigrateError::Unreadable(_))
    ));
}

const MARKDOWN_OLD: &str = "\
# Guide

Write `// expected` after a print. An `end` closes a block:

```petal
let tint = #f80 // warm
fn double(x)
  x * 2
end
print(double(2)) // 4
```

- In a list, the fence is indented:

  ```petal ignore
  fn step(input)
    ... // elided
    if a then b elsif c then d end
  end
  ```

```petal no-migrate
print(1) // the old comment marker, on purpose
```

```petal ignore
rule ::= \"an open string
```

```rust
// not Petal: fn f() { end }
```

````md
```petal
print(1) // inside a longer fence: not a fence of this file
```
````
";

const MARKDOWN_NEW: &str = "\
# Guide

Write `// expected` after a print. An `end` closes a block:

```petal
let tint = :f80 # warm
fn double(x)
  x * 2
print(double(2)) # 4
```

- In a list, the fence is indented:

  ```petal ignore
  fn step(input)
    ... # elided
    if a then b else if c then d end
  end
  ```

```petal no-migrate
print(1) // the old comment marker, on purpose
```

```petal ignore
rule ::= \"an open string
```

```rust
// not Petal: fn f() { end }
```

````md
```petal
print(1) // inside a longer fence: not a fence of this file
```
````
";

#[test]
fn markdown_fences_are_migrated_and_everything_else_is_copied() {
    use petal::migrate::markdown::migrate_markdown;
    let out = migrate_markdown(MARKDOWN_OLD, &MigrateOptions::default());
    assert_eq!(out.output, MARKDOWN_NEW);
    assert_eq!((out.fences, out.rewritten, out.fragments), (4, 2, 1));
    assert_eq!(out.unreadable.len(), 1, "{:?}", out.unreadable);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.ends_removed, 1);
    // Findings carry the line of the Markdown file.
    let colour = out.findings.iter().find(|f| f.rule == COLOUR_LITERALS).unwrap();
    assert_eq!((colour.line, colour.column), (6, 12));
    // Idempotent.
    let again = migrate_markdown(MARKDOWN_NEW, &MigrateOptions::default());
    assert_eq!(again.output, MARKDOWN_NEW);
    assert_eq!(again.rewritten, 0);
    // `--only` selects, as for a file.
    let opts = MigrateOptions { only: Some(vec![HASH_COMMENTS.to_string()]), ..Default::default() };
    let only = migrate_markdown("```petal\nif a then\n  print(1) // 1\nend\n```\n", &opts);
    assert_eq!(only.output, "```petal\nif a then\n  print(1) # 1\nend\n```\n");
}
