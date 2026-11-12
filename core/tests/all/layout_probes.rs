//! Adversarial probes of the layout parser and `petal fmt`
//! (docs/tasks/pythonic-syntax.md §3, §5.1), written against the plan
//! independently of the fixtures in `layout.rs`: the shapes that sit between
//! two rules (a block inside nested brackets, a one-liner continued by a hung
//! `else`, text and comments in front of a line's first token), real files
//! with their `end` lines stripped, and real files misindented at random.
//!
//! Every parse goes through [`petal::cst::parse_source`], whose debug build
//! also checks that the AST projected from the syntax tree is the parser's
//! own.

use petal::source_map::ENTRY_FILE;

fn try_shape(src: &str) -> Result<String, String> {
    let (tree, stmts) = petal::cst::parse_source(src, ENTRY_FILE)?;
    assert_eq!(tree.text(), src, "the syntax tree lost text");
    Ok(petal::rewrite::ast_shape(&stmts)?.to_string())
}

fn by_end(src: &str) -> Result<String, String> {
    let (_tree, stmts) = petal::cst::parse_source_by_end(src)?;
    Ok(petal::rewrite::ast_shape(&stmts)?.to_string())
}

/// A small deterministic generator (no dependency, reproducible failures).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A file of the repo, as it was written with its `end`s. The repo's own
/// `.ptl` lost their `end` lines to `petal migrate-old-code` (`end-lines`),
/// and these probes are about code that still has them, so [`REAL_FILES`]
/// are read from the copies taken just before that migration
/// (`core/tests/fixtures/with-ends/`, named by path, `.txt` so that no tool
/// walks them). A file with no copy ([`MORE_REAL_FILES`], for the ignored
/// sweeps) is read from the repo as it stands.
fn repo_file(rel: &str) -> String {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let copy = core.join("tests/fixtures/with-ends").join(format!("{}.txt", rel.replace('/', "__")));
    if let Ok(text) = std::fs::read_to_string(&copy) {
        return text;
    }
    let root = core.parent().unwrap();
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// `src` with the indentation of `count` random lines changed.
fn misindent(src: &str, rng: &mut Rng, count: usize) -> String {
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    for _ in 0..count {
        let at = rng.below(lines.len());
        let line = &lines[at];
        let body = line.trim_start_matches(' ');
        if body.is_empty() {
            continue;
        }
        let indent = line.len() - body.len();
        let new = match rng.below(6) {
            0 => 0,
            1 => indent + 1,
            2 => indent + 2,
            3 => indent.saturating_sub(1),
            4 => indent.saturating_sub(2),
            _ => indent + 4,
        };
        lines[at] = format!("{}{body}", " ".repeat(new));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn shape(src: &str) -> String {
    try_shape(src).unwrap_or_else(|e| panic!("parse failed: {e}\n--- source ---\n{src}"))
}

/// Every source parses, and to the same program as the first.
#[track_caller]
fn same(sources: &[&str]) {
    let first = shape(sources[0]);
    for src in &sources[1..] {
        assert_eq!(shape(src), first, "\n--- differs from the first ---\n{src}");
    }
}

/// `src` fails with a message that holds every one of `needles`.
#[track_caller]
fn err_has(src: &str, needles: &[&str]) {
    let e = match try_shape(src) {
        Ok(_) => panic!("expected a parse error, but it parsed:\n{src}"),
        Err(e) => e,
    };
    for needle in needles {
        assert!(e.contains(needle), "expected {needle:?} in the error: {e}\n--- source ---\n{src}");
    }
}

/// `src` formatted: the same program, and a fixed point of the formatter.
#[track_caller]
fn fmt(src: &str) -> String {
    let out = petal::fmt::format_source(src).unwrap_or_else(|e| panic!("fmt failed: {e}\n{src}"));
    assert_eq!(shape(&out), shape(src), "fmt changed the program:\n{out}");
    let again = petal::fmt::format_source(&out).unwrap_or_else(|e| panic!("fmt failed on its output: {e}\n{out}"));
    assert_eq!(again, out, "fmt is not idempotent");
    out
}

const MISSING_BLOCK: &str = "expected an indented block after";
const UNEXPECTED_INDENT: &str = "unexpected indent";
const PARTIAL_DEDENT: &str = "this line is indented less than its block, but more than the enclosing one";
const NOT_LINED_UP: &str = "does not line up with";
const TAB: &str = "a tab in the indentation of this line";

// ── B15: a `/* … */` comment in front of a line's first token ────────────

#[test]
fn a_line_that_starts_with_a_block_comment_is_indented_to_the_comment() {
    same(&[
        "fn f()\n  a()\n  b()\nend\n",
        "fn f()\n  /* x */ a()\n  b()\n",
        "fn f()\n  a()\n  /* x */ /* y */ b()\n",
        // The comment began on an earlier line, which it starts.
        "fn f()\n  a()\n  /* two\n  lines */ b()\n",
        "fn f()\n  /* two\nlines */ a()\n  b()\nend\n",
    ]);
    // It used to be measured at the token: the body ended at `b()`, which
    // became a statement of the file when the snippet had no other.
    same(&["let q = 1\nfn f()\n  /* x */ a()\n  b()\nc()\n", "let q = 1\nfn f()\n  a()\n  b()\nend\nc()\n"]);
    // The closing keywords too.
    same(&[
        "fn f()\n  if c then a() else b() end\n  d()\nend\n",
        "fn f()\n  if c then\n    /* 1 */ a()\n  /* 2 */ else\n    b()\n  /* 3 */ end\n  d()\n",
    ]);
    // The base indent (B12).
    same(&["let a = 1\nlet b = 2\n", "    /* lead */ let a = 1\n    let b = 2\n"]);
    // The comment's column is the line's, for the errors as well.
    err_has("fn f()\n  a()\n    /* x */ b()\n", &[UNEXPECTED_INDENT, "column 5", "column 3"]);
    err_has("let q = 1\nfn f()\n  if c then\n      a()\n    /* x */ b()\n", &[PARTIAL_DEDENT]);
    err_has("let q = 1\nfn f()\n  if c then\n  /* x */     a()\n", &[MISSING_BLOCK]);
    err_has("fn f()\n  for x in xs do\n    a()\n    /* x */ end\n", &[NOT_LINED_UP]);
    // A tab in front of the comment is a tab in the indentation (B11).
    err_has("fn f()\n  a()\n\t/* x */ b()\n", &[TAB]);
    // …which fmt turns into spaces with the rest of a tab-indented file,
    // unless the width of a tab decides what the file means.
    assert_eq!(
        fmt_tabs("fn f()\n\t/* x */ a()\n\tif c then\n\t\t/* y */ b()\n\td()\n"),
        "fn f()\n  /* x */ a()\n  if c then\n    /* y */ b()\n  d()\n"
    );
    assert!(petal::fmt::format_source("fn f()\n\t/* x */ a()\n        b()\n").unwrap_err().contains(TAB));
    // A comment after code on an earlier line starts no line.
    err_has("fn f()\n  a() /* two\n  lines */ b()\n  c()\n", &[UNEXPECTED_INDENT]);
    // fmt moves the line as a whole.
    assert_eq!(
        fmt("fn f()\n      /* x */ a()\n      if c then\n          /* y */ b()\n      /* z */ else\n          d()\n"),
        "fn f()\n  /* x */ a()\n  if c then\n    /* y */ b()\n  /* z */ else\n    d()\n"
    );
}

// ── B6: a hole after element text on its line ────────────────────────────

#[test]
fn a_hole_after_text_on_its_line_is_measured_where_the_text_starts() {
    let one_line = "\
fn view(m)
  <p>
    text first {if m.a then <a /> else <b /> end} and more
  </p>
end
";
    let block = "\
fn view(m)
  <p>
    text first {if m.a then
      <a />
    else
      <b />
    } and more
  </p>
";
    let with_end = "\
fn view(m)
  <p>
    text first {if m.a then
      <a />
    else
      <b />
    end} and more
  </p>
end
";
    same(&[one_line, block, with_end]);
    assert_eq!(fmt(block), block);
    // A second hole on the line is measured from the same place.
    same(&[
        "let v = <p>\n  a {x} b {for y in ys do\n    <i>{y}</i>\n  } c\n</p>\n",
        "let v = <p>\n  a {x} b {for y in ys do <i>{y}</i> end} c\n</p>\n",
    ]);
    // Text on the tag's own line: the line is the tag's.
    same(&[
        "fn view(m)\n  <p>text {if m.a then\n      <a />\n    else\n      <b />\n  }</p>\n",
        "fn view(m)\n  <p>text {if m.a then <a /> else <b /> end}</p>\nend\n",
    ]);
    // The body must still be indented past the text.
    err_has(
        "fn view(m)\n  <p>\n    text first {if m.a then\n    <a />\n    }\n  </p>\n",
        &[MISSING_BLOCK, "`then`", "column 5"],
    );
}

// ── B3: the message for a one-line `fn` ──────────────────────────────────

#[test]
fn b3_names_the_one_line_body_of_a_fn() {
    for src in ["fn f() a(); b()\n", "let g = fn() a(); b()\n"] {
        err_has(src, &["`;` after a one-line `fn` body is ambiguous"]);
    }
}

// ── fmt: shapes the indentation pass got wrong ───────────────────────────

#[test]
fn fmt_lays_out_an_arm_block_inside_an_element_hole() {
    // The lexer drops line breaks in a hole, so the `->` that ends a line
    // there is followed by no line-break token; the arm's body went to the
    // arm's own column, which is no body at all.
    let with_end = "\
fn view(m)
  <g>
    {match m.k
      when 1 -> <a />
      when _ ->
        let q = 2
        <b q={q} />
    end}
  </g>
end
";
    assert_eq!(fmt(with_end), with_end);
    let without = "\
fn view(m)
  <g>
    {match m.k
      when 1 -> <a />
      when _ ->
        let q = 2
        <b q={q} />
    }
  </g>
";
    assert_eq!(fmt(without), without);
    same(&[with_end, without]);
    let mangled = "\
fn view(m)
    <g>
        {match m.k
            when 1 -> <a />
            when _ ->
                    let q = 2
                    <b q={q} />
        }
    </g>
";
    assert_eq!(fmt(mangled), without);
    // An `if` in the hole whose body is a `match` with a block arm.
    fmt("let v = <g>\n  {if k then\n    match k\n      when _ ->\n        q\n  }\n</g>\n");
}

#[test]
fn fmt_measures_a_block_opened_after_a_string_from_the_line_the_string_began_on() {
    // A line that begins inside a string is not a line start: the block
    // lambda opened on the string's last line is measured from the line
    // the string began on, so its body goes a step in from there. fmt put
    // it a step in from the string's last line, which the parser refuses.
    let with_end = "fn f()\n  g(\"a\nb\", fn(x)\n    x + 1\n  end)\n  h()\nend\n";
    let without = "fn f()\n  g(\"a\nb\", fn(x)\n    x + 1\n  )\n  h()\n";
    assert_eq!(fmt(with_end), with_end);
    assert_eq!(fmt(without), without);
    same(&[with_end, without]);
    assert_eq!(fmt("fn f()\n      g(\"a\nb\", fn(x)\n            x + 1\n      )\n      h()\n"), without);
    // A raw string, and an `if` / a `match` after it.
    let raw = "\
fn f()
  if c then
    emit(\"\"\"
raw
\"\"\", for x in xs do
      x
    )
  let s = \"
\" ++ match k
    when 1 -> \"a\"
    when _ ->
      \"b\"
  s
";
    assert_eq!(fmt(raw), raw);
    // At the top level the two lines share a column, as before.
    let top = "let s = \"\"\"\nx\n\"\"\" ++ (if c then\n  \"a\"\nelse\n  \"b\"\n)\n";
    assert_eq!(fmt(top), top);
}

// ── Probes that passed: kept as fixtures ─────────────────────────────────

#[test]
fn one_line_bodies_inside_brackets_end_at_the_separator() {
    same(&[
        "[if c then 1 else 2 end, fn(v) v * 2 end, for x in xs do x end]\n",
        "[if c then 1 else 2, fn(v) v * 2, for x in xs do x]\n",
    ]);
    same(&["{a: if c then 1 else 2 end, b: fn(v) v end}\n", "{a: if c then 1 else 2, b: fn(v) v}\n"]);
    same(&["f(if c then 1 else 2 end, 3)\n", "f(if c then 1 else 2, 3)\n"]);
    same(&["map(xs, fn(v) v * 2 end).len()\n", "map(xs, fn(v) v * 2).len()\n"]);
    same(&[
        "let x = if c then a else b end[0]\n",
        "let x = (if c then a else b)[0]\n",
    ]);
    same(&[
        "for x in xs do if x then print(x) else print(0) end end\nprint(1)\n",
        "for x in xs do if x then print(x) else print(0)\nprint(1)\n",
    ]);
    // An inner `end` on the line closes the inner block only.
    same(&["let x = if a then if b then 1 end else 2\n", "let x = if a then (if b then 1) else 2\n"]);
    same(&["if c then a() end; b()\n", "if c then a() end\nb()\n"]);
}

#[test]
fn an_else_on_its_own_line_finds_its_if_by_column() {
    // The one-line `if` in a loop takes the `else` at its own column…
    same(&[
        "for x in xs do\n  if x then a()\n  else b()\nc()\n",
        "for x in xs do\n  if x then a() else b() end\nend\nc()\n",
    ]);
    // …and one left of it belongs to the `if` around the loop.
    same(&[
        "if a then\n  for x in xs do\n    if x then p()\n  q()\nelse\n  r()\n",
        "if a then\n  for x in xs do\n    if x then p() end\n  end\n  q()\nelse\n  r()\nend\n",
    ]);
    same(&[
        "if a then\n  if b then c()\nelse d()\n",
        "if a then\n  if b then c() end\nelse\n  d()\nend\n",
    ]);
    // A hung chain inside a block, then the block goes on.
    same(&[
        "fn f()\n  if a then\n    let q = if b then 1\n        else 2\n    q\n",
        "fn f()\n  if a then\n    let q = if b then 1 else 2 end\n    q\n  end\nend\n",
    ]);
    // A hung chain as an argument, and as an arm's body.
    same(&["f(if a then 1\n  else 2, 3)\n", "f(if a then 1 else 2 end, 3)\n"]);
    same(&[
        "let z = 1\nmatch x\n  when 1 -> if c then a\n    else b\n  when 2 -> d\n",
        "let z = 1\nmatch x\n  when 1 -> if c then a else b end\n  when 2 -> d\nend\n",
    ]);
    // `else` at the end of a line opens a block that holds the next `if`
    // (core-runtime/bloom/src/menu.ptl:256).
    same(&[
        "let ink = if m.disabled then t.dim else\n  if m.danger then t.red else t.fg\nprint(ink)\n",
        "let ink = if m.disabled then t.dim else\n  if m.danger then t.red else t.fg end\nend\nprint(ink)\n",
        "let ink = if m.disabled then t.dim else (if m.danger then t.red else t.fg)\nprint(ink)\n",
    ]);
    // An `else` with no `if` at or left of it.
    err_has("fn f()\n  let q = if b then 1\nelse 2\n", &["Unexpected token: `else`"]);
}

#[test]
fn blocks_nest_through_brackets() {
    let without = "\
let z = 1
let v = [
  {
    on: fn(e)
      if e.down then
        handle(e)
      else
        skip(e)
    ,
    k: 2
  },
  3
]
";
    let with_ends = "\
let z = 1
let v = [
  {
    on: fn(e)
      if e.down then
        handle(e)
      else
        skip(e)
      end
    end,
    k: 2
  },
  3
]
";
    same(&[with_ends, without]);
    fmt(without);
    // Two block lambdas in one call, every way the plan allows.
    same(&[
        "let z = 1\ntwice(fn()\n  a()\nend, fn()\n  b()\nend)\n",
        "let z = 1\ntwice(fn()\n  a()\n, fn()\n  b()\n)\n",
        "let z = 1\ntwice(fn()\n  a()\nend, fn()\n  b()\n)\n",
        "let z = 1\ntwice(\n  fn()\n    a()\n  ,\n  fn()\n    b()\n)\n",
        "let z = 1\ntwice(\n  fn()\n    a()\n  end,\n  fn()\n    b()\n  end\n)\n",
    ]);
    // A block lambda in a block lambda, each closed by its call's `)`.
    same(&[
        "fn outer()\n  each(xs, fn(x)\n    each(ys, fn(y)\n      if x == y then\n        hit(x)\n    )\n    done(x)\n  )\n  finish()\n",
        "fn outer()\n  each(xs, fn(x)\n    each(ys, fn(y)\n      if x == y then\n        hit(x)\n      end\n    end)\n    done(x)\n  end)\n  finish()\nend\n",
    ]);
    // The block is measured from the line its construct is on, not from
    // the bracket.
    same(&[
        "let z = 1\nlet h = wrap(1, 2,\n             fn(x)\n               x + 1\n             )\n",
        "let z = 1\nlet h = wrap(1, 2, fn(x) x + 1 end)\n",
    ]);
    err_has("let z = 1\nlet h = wrap(1, 2,\n    fn(x)\n  x + 1\n)\n", &[MISSING_BLOCK, "column 5"]);
    // After the call closes the block, the expression goes on.
    same(&["let z = 1\nxs |> map(fn(x)\n  x * 2\n) |> sum()\n", "let z = 1\nxs |> map(fn(x) x * 2 end) |> sum()\n"]);
    same(&["let z = 1\nlet q = xs[if a then\n  0\nelse\n  1\n]\n", "let z = 1\nlet q = xs[if a then 0 else 1 end]\n"]);
    // An `end` cannot close a construct outside the brackets it is in.
    err_has("fn f()\n  g([\n    1\nend\n  ])\n", &["Expected ','"]);
}

#[test]
fn match_arms_nest_and_line_up() {
    same(&[
        "let z = 1\nmatch x\n  when 1 ->\n    match y\n      when 2 -> a\n      when 3 -> b\n  when 4 -> c\n",
        "let z = 1\nmatch x\n  when 1 ->\n    match y\n    when 2 -> a\n    when 3 -> b\n  when 4 -> c\n",
        "let z = 1\nmatch x\n  when 1 -> match y\n    when 2 -> a\n    when 3 -> b\n  when 4 -> c\n",
        "let z = 1\nmatch x\n  when 1 -> match y when 2 -> a when 3 -> b end\n  when 4 -> c\nend\n",
    ]);
    same(&[
        "fn f(x)\n  match x\n  when 1 -> a\n  when 2 -> b\n  c\n",
        "fn f(x)\n  match x\n    when 1 -> a\n    when 2 -> b\n  end\n  c\nend\n",
    ]);
    err_has("let z = 1\nmatch x\n    when 1 -> a\n  when 2 -> b\n", &[PARTIAL_DEDENT]);
    err_has("let z = 1\nmatch x\n  when 1 -> a\n    when 2 -> b\n", &[UNEXPECTED_INDENT]);
    err_has("let z = 1\nmatch x\n  when 1 -> a\n  end\n", &[NOT_LINED_UP, "`match`"]);
    err_has("let z = 1\nmatch x\n  when 1 ->\n    a\n    end\n", &[NOT_LINED_UP]);
    err_has("let z = 1\nmatch x\n  when 1 -> a()\n    b()\n", &[UNEXPECTED_INDENT]);
}

#[test]
fn strings_and_line_ends_do_not_move_blocks() {
    // Text at column 1 inside a block, in both kinds of string; GLSL that
    // spells `end`, `else if`, `//` and `#`.
    let with_ends = "\
fn shader()
  let src = \"line one
line two at col 0
      line three deep\"
  let raw = \"\"\"
#define X 1
// glsl comment
if (a) { b; } else if (c) { d; }
end
\"\"\"
  compile(src, raw)
end
";
    let without = with_ends.strip_suffix("end\n").unwrap();
    same(&[with_ends, without]);
    assert_eq!(fmt(without), without);
    // The statement after a string that closes at column 1.
    same(&[
        "fn f()\n  let a = \"\"\"\n  x\n\"\"\" ++ \"y\"\n  let b = 2\n  a\n",
        "fn f()\n  let a = \"\"\"\n  x\n\"\"\" ++ \"y\"\n  let b = 2\n  a\nend\n",
    ]);
    // A one-line `then` whose value spans lines, then a hung `else`.
    same(&[
        "fn f(c)\n  let s = if c then \"a\nb\"\n  else \"c\"\n  s\n",
        "fn f(c)\n  let s = if c then \"a\nb\" else \"c\" end\n  s\nend\n",
    ]);
    // CRLF line ends.
    let crlf = "fn f(c)\r\n  if c then\r\n    a()\r\n  else\r\n    b()\r\n\r\n  # comment\r\n  match c\r\n    when 1 -> x\r\n    when _ ->\r\n      y\r\n  d()\r\n";
    same(&[
        "fn f(c)\n  if c then\n    a()\n  else\n    b()\n  end\n  match c\n    when 1 -> x\n    when _ ->\n      y\n  end\n  d()\nend\n",
        crlf,
    ]);
    err_has("fn f(c)\r\n  if c then\r\n  a()\r\n", &[MISSING_BLOCK]);
}

#[test]
fn headers_and_comments_at_any_column() {
    same(&[
        "fn g()\n  for x in range(0, 10) do\n    print(x)\n  end\n  while a || b do\n    step()\n  end\n  done()\nend\n",
        "fn g()\n  for x in range(\n    0,\n    10\n  ) do\n    print(x)\n  while a ||\nb do\n    step()\n  done()\n",
    ]);
    same(&["fn g()\n  if a then\n    c()\n  end\n  d()\nend\n", "fn g()\n  if a\n  then\n    c()\n  d()\n"]);
    err_has("fn g()\n  if (a &&\n        b) then\n  c()\n", &[MISSING_BLOCK, "column 3"]);
    let commented = "\
fn f(c)   # header
  # leading comment
  if c then # opener comment

        # weird column comment
    a()
# col 0 comment
    b()
      # deeper comment
  else # else comment
# another
    d()
  /* block
comment */
  e()
";
    same(&["fn f(c)\n  if c then\n    a()\n    b()\n  else\n    d()\n  end\n  e()\nend\n", commented]);
    fmt(commented);
    // A body of comments only is no body.
    err_has("fn f()\n  # nothing\nprint(1)\n", &[MISSING_BLOCK]);
    same(&["fn f()\n  # nothing\nend\nprint(1)\n", "fn f() end\nprint(1)\n"]);
}

#[test]
fn fmt_keeps_blocks_with_no_end_at_any_indent_width() {
    let want = "\
fn f(xs)
  for x in xs do
    if x then
      a()
    else if y then
      b()
    else
      c()
    d()
  e()
g()
";
    for width in [1usize, 3, 4, 8] {
        let wide: String = want
            .lines()
            .map(|l| {
                let indent = l.len() - l.trim_start().len();
                format!("{}{}\n", " ".repeat(indent / 2 * width), l.trim_start())
            })
            .collect();
        assert_eq!(fmt(&wide), want, "width {width}");
    }
    // A file indented with tabs alone.
    assert_eq!(fmt_tabs(&want.replace("  ", "\t")), want);
    for src in [
        "fn f(m)\n    each(xs, fn(x)\n        each(ys, fn(y)\n            hit(x, y)\n        )\n    )\n    twice(fn()\n        a()\n    , fn()\n        b()\n    )\n    done()\n",
        "fn f(k)\n    match k\n    when 1 -> a\n    when 2 ->\n        b\n        c\n    when _ ->\n        match j\n            when 3 -> d\n    e()\n",
        "fn f(c)\n      if c then a()\n      else\n            b()\n      for x in xs do print(x)\n      while c do\n            step()\n",
        "fn f(c)\n    g(if c then\n        1\n    else\n        2\n    , 3)\n    h([\n        for x in xs do\n            x * 2\n        ,\n        4\n    ])\n",
        "fn f(c)\n    let a = for x in xs do\n        let b = if x then\n            1\n        else\n            2\n        b * 2\n    a\n",
        "class P\n      x: int,\n      y: int\nenum S\n   a,\n   b(w)\ntest \"t\" do\n     expect_eq(1, 1)\nlet q = 1\n",
        "fn view(m)\n    <g>\n            {if m.a then\n                    <rect x={0} />\n            else\n                    <circle r={2} />\n            }\n        {for r in m.rows do\n          <row>\n           {if r then <a /> else <b />}\n          </row>\n        }\n    </g>\n",
    ] {
        fmt(src);
    }
}

/// [`fmt`] for a source the parser refuses for its tabs.
fn fmt_tabs(src: &str) -> String {
    err_has(src, &[TAB]);
    let out = petal::fmt::format_source(src).unwrap_or_else(|e| panic!("fmt failed: {e}\n{src}"));
    assert_eq!(petal::fmt::format_source(&out).unwrap(), out);
    out
}

// ── Real files with their `end` lines stripped ───────────────────────────

/// Files with many nested blocks, from `examples/` and `core-runtime/`.
const REAL_FILES: &[&str] = &[
    "examples/games/snake/app.ptl",
    "examples/games/2048/2048.ptl",
    "examples/games/minesweeper/minesweeper.ptl",
    "examples/games/pong/app.ptl",
    "examples/productivity/settings/widgets.ptl",
    "core-runtime/bloom/src/menu.ptl",
    "core-runtime/bloom/src/overlay.ptl",
    "core-runtime/bloom/src/button.ptl",
    "core-runtime/bloom/src/panes.ptl",
    "core-runtime/bloom/src/theme.ptl",
];

const MORE_REAL_FILES: &[&str] = &[
    "examples/productivity/file-browser/app.ptl",
    "examples/dashboards/network-graph/app.ptl",
    "examples/custom-apps/petal-fantasy-nes/carts/petal_quest/game.ptl",
    "core-runtime/glsl/src/glsl.ptl",
    "examples/productivity/restaurant-pos/app.ptl",
    "examples/productivity/video-timeline/app.ptl",
    "examples/productivity/notes/app.ptl",
    "examples/productivity/music-sequencer/app.ptl",
    "examples/games/fractal-explorer/app.ptl",
    "examples/games/tower-defense/app.ptl",
    "examples/dashboards/finance-dashboard/app.ptl",
    "examples/custom-apps/petal-fantasy-nes/prelude/nes_sound.ptl",
    "examples/games/physics-playground/app.ptl",
    "examples/productivity/photo-adjust/app.ptl",
    "examples/productivity/command-palette/app.ptl",
    "examples/productivity/email-client/mailbox.ptl",
    "examples/dashboards/server-monitoring/app.ptl",
    "examples/dashboards/analytics-dashboard/app.ptl",
    "examples/games/breakout/app.ptl",
    "examples/games/particle-sandbox/app.ptl",
    "examples/games/asteroids/app.ptl",
    "core-runtime/bloom/src/fields.ptl",
    "examples/productivity/settings/preview.ptl",
    "examples/productivity/stock-trading/look.ptl",
    "examples/custom-apps/petal-fps/examples/fps_game.ptl",
    "examples/games/boids/boids.ptl",
];

/// `src` without the lines that hold only an `end` (and a comment), and
/// with `end)` / `end]` / `end}` at the start of a line cut to the bracket.
/// A throwaway of the `end-lines` migration: no parse, no gate.
fn strip_end_lines(src: &str) -> (String, usize) {
    let mut out = String::new();
    let mut removed = 0;
    for line in src.lines() {
        let body = line.trim_start();
        let rest = body.strip_prefix("end").unwrap_or("x");
        let tail = rest.trim_start();
        if tail.is_empty() || tail.starts_with('#') {
            removed += 1;
            continue;
        }
        if rest.starts_with([')', ']', '}']) {
            removed += 1;
            out.push_str(&line[..line.len() - body.len()]);
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    (out, removed)
}

/// Returns how many `end`s were stripped from the files that still read as
/// the program they were.
fn strip_sweep(files: &[&str]) -> usize {
    let mut removed = 0;
    for rel in files {
        let src = repo_file(rel);
        let original = shape(&src);
        let (stripped, n) = strip_end_lines(&src);
        // Stripping blindly may leave something that does not parse (an
        // empty block, a `;` in a one-line body). It must never leave a
        // different program.
        let Ok(after) = try_shape(&stripped) else { continue };
        assert_eq!(after, original, "{rel}: a different program without its `end` lines");
        removed += n;
        for text in [&src, &stripped] {
            let out = petal::fmt::format_source(text).unwrap_or_else(|e| panic!("{rel}: fmt failed: {e}"));
            assert_eq!(shape(&out), original, "{rel}: fmt changed the program");
            assert_eq!(petal::fmt::format_source(&out).unwrap(), out, "{rel}: fmt is not idempotent");
        }
    }
    removed
}

#[test]
fn real_files_are_the_same_programs_without_their_end_lines() {
    let removed = strip_sweep(REAL_FILES);
    let had: usize = REAL_FILES.iter().map(|rel| strip_end_lines(&repo_file(rel)).1).sum();
    assert!(had > 300, "the copies with their `end`s are gone: {had} `end` lines");
    assert!(removed * 10 >= had * 9, "stripped {removed} of {had} `end` lines");
}

#[test]
#[ignore = "expensive: 26 large files, each parsed and formatted several times"]
fn more_real_files_are_the_same_programs_without_their_end_lines() {
    let removed = strip_sweep(MORE_REAL_FILES);
    let had: usize = MORE_REAL_FILES.iter().map(|rel| strip_end_lines(&repo_file(rel)).1).sum();
    assert!(removed * 10 >= had * 9, "stripped {removed} of {had} `end` lines");
}

// ── §5.1: real files, misindented at random ──────────────────────────────

/// Misindent `rounds` copies of each file. A copy its `end`s still read
/// (the reading from before indentation delimited blocks) must be an error
/// now, or the same program: never another one.
fn mutation_sweep(files: &[&str], rounds: usize) -> (usize, usize) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut errors, mut same) = (0, 0);
    for rel in files {
        let src = repo_file(rel);
        for round in 0..rounds {
            let mutated = misindent(&src, &mut rng, 1 + round % 3);
            let Ok(old) = by_end(&mutated) else { continue };
            match try_shape(&mutated) {
                Err(_) => errors += 1,
                Ok(new) => {
                    let changed: Vec<String> = src
                        .lines()
                        .zip(mutated.lines())
                        .enumerate()
                        .filter(|(_, (a, b))| a != b)
                        .map(|(i, (_, b))| format!("line {}: {b:?}", i + 1))
                        .collect();
                    assert_eq!(new, old, "{rel}: misindented into another program\n{}", changed.join("\n"));
                    same += 1;
                }
            }
        }
    }
    (errors, same)
}

#[test]
fn misindented_real_files_are_errors_or_the_same_programs() {
    let (errors, same) = mutation_sweep(REAL_FILES, 8);
    // Both outcomes occur: most misindentations are errors, and a
    // continuation line or a comment moves without consequence.
    assert!(errors > 0 && same > 0, "errors {errors}, same {same}");
}

#[test]
#[ignore = "expensive: thousands of parses of large files"]
fn misindented_real_files_are_errors_or_the_same_programs_at_length() {
    let files: Vec<&str> = REAL_FILES.iter().chain(MORE_REAL_FILES).copied().collect();
    let (errors, same) = mutation_sweep(&files, 120);
    assert!(errors > 0 && same > 0, "errors {errors}, same {same}");
}

// ── fmt on real files with no `end`s, misindented but still valid ────────

/// Misindent `rounds` copies of each file's `end`-less spelling. Whatever
/// still parses (a moved comment, a continuation line, a hung `else`, a
/// whole block moved with its last line) fmt must lay out as the same
/// program, and its output must be a fixed point.
fn fmt_mutation_sweep(files: &[&str], rounds: usize) -> usize {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut formatted = 0;
    for rel in files {
        let (src, _) = strip_end_lines(&repo_file(rel));
        if try_shape(&src).is_err() {
            continue;
        }
        for round in 0..rounds {
            let mutated = misindent(&src, &mut rng, 1 + round % 2);
            let Ok(before) = try_shape(&mutated) else { continue };
            let changed = || {
                src.lines()
                    .zip(mutated.lines())
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(i, (_, b))| format!("line {}: {b:?}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let out = petal::fmt::format_source(&mutated)
                .unwrap_or_else(|e| panic!("{rel}: fmt failed: {e}\n{}", changed()));
            assert_eq!(shape(&out), before, "{rel}: fmt changed the program\n{}", changed());
            let again = petal::fmt::format_source(&out).unwrap();
            assert_eq!(again, out, "{rel}: fmt is not idempotent\n{}", changed());
            formatted += 1;
        }
    }
    formatted
}

#[test]
fn fmt_lays_out_misindented_real_files_with_no_ends() {
    let rounds: usize = std::env::var("ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    assert!(fmt_mutation_sweep(REAL_FILES, rounds) > 0);
}
