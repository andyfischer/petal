use super::*;

fn fmt(src: &str) -> String {
    format_source(src).unwrap()
}

#[test]
fn formats_spacing_indent_comments_and_blank_lines() {
    let src = "\n\nfn f(a,b)\nlet x=a+b      // sum\nlet yy=a*b     // product\n\n\n\nreturn x+yy\nend\n\n\n";
    // The two comments were lined up, and stay lined up however the code
    // before them was respaced.
    assert_eq!(
        fmt(src),
        "fn f(a, b)\n  let x = a + b  // sum\n  let yy = a * b // product\n\n  return x + yy\nend\n"
    );
}

#[test]
fn a_keyword_after_an_operand_is_spaced_from_it() {
    let src = "let r = match v\n  when Op.fight(p,w)if w&&flag->p\n  when [a,b]if a->b\n  when _->0\nend\nif(r>1)then print(r)end\n";
    assert_eq!(
        fmt(src),
        "let r = match v\n  when Op.fight(p, w) if w && flag -> p\n  when [a, b] if a -> b\n  when _ -> 0\nend\nif(r > 1) then print(r) end\n"
    );
}

#[test]
fn else_if_takes_one_space_and_lines_up_with_its_if() {
    let src = "fn f(a)\nif a>1 then\n1\n  else   \t if a>0 then\n2\nelse\n3\nend\nend\n";
    let out = "fn f(a)\n  if a > 1 then\n    1\n  else if a > 0 then\n    2\n  else\n    3\n  end\nend\n";
    assert_eq!(fmt(src), out);
    assert_eq!(fmt(out), out);
    // One-line arms hung under a value keep their column, as `elsif` arms do.
    let arms = "let y = if a then 1\n        else  if b then 2\n        else 3 end\n";
    assert_eq!(fmt(arms), "let y = if a then 1\n        else if b then 2\n        else 3 end\n");
    // The old spelling is laid out as written: respelling it is
    // `petal migrate-old-code`'s.
    let old = "if a then\n  1\nelsif b then\n  2\nend\n";
    assert_eq!(fmt(old), old);
    // A protected line keeps its spacing, inside the token too.
    let kept = "if a then\n  1\n  # petal-fmt-ignore\nelse    if b then\n  2\nend\n";
    assert_eq!(fmt(kept), kept);
}

#[test]
fn is_idempotent_on_its_own_output() {
    let src = "state a=1 // x\nstate bbb   =  2 // y\nif(a>1)then\nprint( \"{a+1}\" )\nend\n";
    let once = fmt(src);
    assert_eq!(fmt(&once), once);
}

#[test]
fn spaces_a_multi_generator_for_header() {
    let src = "for a in xs,b in ys   key  b do\nprint(a)\nend\nx = for a in xs ,  b in ys do a end\n";
    let want = "for a in xs, b in ys key b do\n  print(a)\nend\nx = for a in xs, b in ys do a end\n";
    assert_eq!(fmt(src), want);
    assert_eq!(fmt(want), want);
    assert_eq!(ast_shape(src), ast_shape(want));
}

#[test]
fn a_keyword_used_as_a_label_does_not_close_or_open_a_block() {
    // `end:` names slice's parameter; it is not the `end` of the `if` or `fn`.
    let src = "fn trim(s, t)\n  if len(s) > 0 then set s = slice(s, start: 0, end: len(s) - 1) end\n  if len(t) > 0 then set s = s ++ t end\n  s\nend\n";
    assert_eq!(fmt(src), src);
    let src = "fn span()\n  let r = {\n    if: 1,\n    end: 2,\n  }\n  slice(\n    xs,\n    start: 0,\n    end: 3,\n  )\nend\n";
    assert_eq!(fmt(src), src);
    // `.end` reads a field; it is not the `end` of the `fn`.
    let src = "fn last(row, r)\n  let to = row.end\n  let from = r?.end\n  if to > from then\n    to\n  end\nend\n";
    assert_eq!(fmt(src), src);
}

#[test]
fn ignore_next_line_keeps_it_verbatim() {
    let src = "// petal-fmt-ignore\nquad(x0,y0,z1,   x1,y0,z1)\nquad(x0,y0)\n";
    assert_eq!(
        fmt(src),
        "// petal-fmt-ignore\nquad(x0,y0,z1,   x1,y0,z1)\nquad(x0, y0)\n"
    );
}

#[test]
fn off_on_region_keeps_every_line() {
    let src =
        "let a=1\n// petal-fmt-off\nlet m = [\n  1,0,\n      0,1,\n]\n// petal-fmt-on\nlet b=2\n";
    assert_eq!(
        fmt(src),
        "let a = 1\n// petal-fmt-off\nlet m = [\n  1,0,\n      0,1,\n]\n// petal-fmt-on\nlet b = 2\n"
    );
}

#[test]
fn ignore_file_leaves_everything() {
    let src = "// petal-fmt-ignore-file\nlet a=1\n\n\n\nlet b  =  2";
    assert_eq!(fmt(src), src);
}

/// The directives are read with either comment marker: `#`, and the
/// deprecated `//` the tests above use.
#[test]
fn directives_work_in_hash_comments() {
    let src = "# petal-fmt-ignore\nquad(x0,y0,z1,   x1,y0,z1)\nquad(x0,y0)\n";
    assert_eq!(fmt(src), "# petal-fmt-ignore\nquad(x0,y0,z1,   x1,y0,z1)\nquad(x0, y0)\n");
    let src = "let a=1\n# petal-fmt-off\nlet m = [\n  1,0,\n      0,1,\n]\n# petal-fmt-on\nlet b=2\n";
    assert_eq!(
        fmt(src),
        "let a = 1\n# petal-fmt-off\nlet m = [\n  1,0,\n      0,1,\n]\n# petal-fmt-on\nlet b = 2\n"
    );
    let src = "# petal-fmt-ignore-file\nlet a=1\n\n\n\nlet b  =  2";
    assert_eq!(fmt(src), src);
}

/// A `#` comment is laid out exactly as a `//` one is: a whole-line comment
/// is indented with its block, and a trailing one keeps its column.
#[test]
fn hash_comments_are_laid_out_like_slash_comments() {
    let slash = "fn f()\n// c\nlet a=1   // one\nlet bb =  2 // two\na+bb\nend\n";
    let hash = slash.replace("//", "#");
    // The comment starts in the same column either way, so the layout is
    // the same text but for the marker.
    assert_eq!(fmt(&hash), fmt(slash).replace("//", "#"));
    assert_eq!(fmt(&fmt(&hash)), fmt(&hash));
    assert!(fmt(&hash).contains("\n  # c\n"), "{}", fmt(&hash));
}

#[test]
fn refuses_a_file_that_does_not_parse() {
    assert!(format_source("fn (").is_err());
}

#[test]
fn directive_word_reads_the_first_word() {
    assert_eq!(
        directive_word("// petal-fmt-ignore"),
        Some("petal-fmt-ignore")
    );
    assert_eq!(
        directive_word("//petal-lint-ignore prefer-let"),
        Some("petal-lint-ignore")
    );
    assert_eq!(directive_word("// a note"), None);
    assert_eq!(directive_word("# petal-fmt-ignore"), Some("petal-fmt-ignore"));
    assert_eq!(directive_word("#petal-lint-ignore prefer-let"), Some("petal-lint-ignore"));
    assert_eq!(directive_word("# a note"), None);
    assert_eq!(directive_word("petal-fmt-ignore"), None);
}

/// The AST of `src` as `show-ast` prints it, without positions — what must
/// survive formatting.
fn ast_shape(src: &str) -> String {
    // The input of a formatting test may be indented against its `end`s.
    let (_tree, stmts, _by_end) = crate::rewrite::parse_ast_or_by_end(src).unwrap();
    crate::ast_display::display_stmts(&stmts)
        .lines()
        .map(|line| line.split(" @").next().unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Enum declarations (payload fields, constant data, one variant over several
/// lines), qualified variants, or-patterns and statement-list arms: laid out
/// canonically, a fixed point, and the same program.
#[test]
fn formats_enums_and_match_arms() {
    let src = r#"enum Use
road,green
end
pub enum Kind
parkway(shape:"route",use:Use.road),
  park( shape : "area" , use : Use.green ),
      plaza(
    shape: "area",
  use: Use.green,
      ),
end
enum Op
fight(place,won,tone:"bad"),gift(amount,tone:"good"),
end
fn f(k: Kind, op) -> Kind
let x = match k
when Kind.park|Kind.plaza -> 1
when Kind.parkway ->
let a = 1
if a > 0 then
print(a)
end
a + 1
end
match op
when Op.fight(place, won) if [1].any(fn(v) -> v == place) ->
print(place)
print(won)
when Op.gift(1|2)|Op.gift(3) -> nil
when Op.gift(_)
|Op.fight(_,_) -> nil
end
match x
when [1|2, _] -> 2
when {a: "p"|"q"} -> 3
when _ ->
match k
when Kind.park -> 1
when _ ->
print(1)
2
end
end
k
end
print(f(Kind.all[0], Op.gift(1)), Kind.from("park").shape)
"#;
    let want = r#"enum Use
  road, green
end
pub enum Kind
  parkway(shape: "route", use: Use.road),
  park(shape: "area", use: Use.green),
  plaza(
    shape: "area",
    use: Use.green,
  ),
end
enum Op
  fight(place, won, tone: "bad"), gift(amount, tone: "good"),
end
fn f(k: Kind, op) -> Kind
  let x = match k
    when Kind.park | Kind.plaza -> 1
    when Kind.parkway ->
      let a = 1
      if a > 0 then
        print(a)
      end
      a + 1
  end
  match op
    when Op.fight(place, won) if [1].any(fn(v) -> v == place) ->
      print(place)
      print(won)
    when Op.gift(1 | 2) | Op.gift(3) -> nil
    when Op.gift(_)
    | Op.fight(_, _) -> nil
  end
  match x
    when [1 | 2, _] -> 2
    when {a: "p" | "q"} -> 3
    when _ ->
      match k
        when Kind.park -> 1
        when _ ->
          print(1)
          2
      end
  end
  k
end
print(f(Kind.all[0], Op.gift(1)), Kind.from("park").shape)
"#;
    let out = fmt(src);
    assert_eq!(out, want);
    assert_eq!(fmt(&out), out, "not a fixed point");
    assert_eq!(ast_shape(&out), ast_shape(src), "formatting changed the AST");

    // The input is indented against its `end`s, which the language rejects
    // (indentation delimits blocks): only the formatted text compiles. The
    // AST comparison above is what holds the two to one program.
    use crate::ir_equiv::compile_for_compare;
    assert!(compile_for_compare(src, &[], None).is_err());
    compile_for_compare(&out, &[], None).unwrap();
}

/// The safety property, over every `.ptl` in the repo: formatting leaves the
/// compiled IR identical (it is whitespace, and Petal is not
/// whitespace-significant), and formatting is a fixed point.
#[test]
#[ignore = "expensive corpus sweep: run with `make expensive_lint_tests`"]
fn fmt_is_ir_equal_and_idempotent_over_repo_corpus() {
    use crate::ir_equiv::{compile_for_compare, ir_equivalent};

    let files = crate::test_corpus::repo_ptl_files();
    let includes = crate::test_corpus::host_package_dirs();
    // One file per worker: each file fmt changes compiles twice, which is
    // the whole cost.
    let checked = crate::test_corpus::par_map(&files, |path| {
        let Ok(src) = std::fs::read_to_string(path) else {
            return Swept::Skipped;
        };
        let formatted = match format_source(&src) {
            Ok(t) => t,
            // A file that does not parse (a negative test) is refused, which
            // is the correct outcome. Any other refusal is a formatter bug.
            Err(e) if e.starts_with("fmt bug") => panic!("{}: {}", path.display(), e),
            Err(_) => return Swept::Skipped,
        };
        assert_eq!(
            format_source(&formatted).unwrap(),
            formatted,
            "fmt not idempotent for {}",
            path.display()
        );
        // Already canonical: the IR comparison would be of a text with
        // itself. Most of the repo is (`petal migrate-old-code` formatted
        // every file it took the `end` lines from), so one such file in
        // five is compared with itself indented as a whole instead, which
        // fmt must undo and which is the same program (the first statement
        // sets the left margin).
        let (src, formatted) = if formatted == src {
            let sampled = path.to_string_lossy().bytes().map(usize::from).sum::<usize>() % 5 == 0;
            let Some(mangled) = mangle_indentation(&src).filter(|_| sampled) else {
                return Swept::Unchanged;
            };
            if mangled.contains(OFF) || mangled.contains(IGNORE_NEXT) || format_source(&mangled).ok().as_ref() != Some(&src) {
                return Swept::Unchanged; // the mangle sweep's to judge
            }
            (mangled, src)
        } else {
            (src, formatted)
        };
        let Ok((env_a, pid_a)) = compile_for_compare(&src, &includes, Some(path)) else {
            return Swept::Skipped; // doesn't compile standalone; nothing to compare
        };
        let (env_b, pid_b) = compile_for_compare(&formatted, &includes, Some(path))
            .unwrap_or_else(|e| panic!("fmt broke compilation for {}: {}", path.display(), e));
        let (a, b) = (
            env_a.get_program(pid_a).expect("original program"),
            env_b.get_program(pid_b).expect("formatted program"),
        );
        if let Err(diff) = ir_equivalent(a, b) {
            panic!("fmt changed IR for {}:\n{}", path.display(), diff);
        }
        Swept::Compared
    });
    let formatted = checked.iter().filter(|c| **c != Swept::Skipped).count();
    let compared = checked.iter().filter(|c| **c == Swept::Compared).count();
    assert!(formatted > 150, "expected a real corpus, formatted {formatted}");
    assert!(
        compared >= 10,
        "only {compared} files were changed by fmt and compared by IR; if the repo \
         has been formatted wholesale, feed this test unformatted fixtures"
    );
}

/// What the corpus sweep did with one file.
#[derive(PartialEq)]
enum Swept {
    /// Unreadable, unparseable, or does not compile on its own.
    Skipped,
    /// Formatting is a fixed point and changed nothing.
    Unchanged,
    /// Formatting changed the text, and the IR of both was compared.
    Compared,
}

/// Re-indentation must undo any indentation, not just leave clean files alone:
/// every repo file is *mangled* first — three extra spaces on the front of
/// every non-empty line — and formatting the mangled text must give the same
/// result as formatting the original. Files with a multi-line token (raw
/// string, JSX text) are skipped: there the extra spaces would be content.
#[test]
#[ignore = "expensive corpus sweep: run with `make expensive_lint_tests`"]
fn fmt_undoes_mangled_indentation_over_repo_corpus() {
    let files = crate::test_corpus::repo_ptl_files();
    let checked = crate::test_corpus::par_map(&files, |path| {
        let Ok(src) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(formatted) = format_source(&src) else {
            return false;
        };
        let Some(mangled) = mangle_indentation(&src) else {
            return false;
        };
        if mangled.contains(OFF) || mangled.contains(IGNORE_NEXT) {
            return false; // fenced lines keep the mangle, by design
        }
        let remangled = format_source(&mangled)
            .unwrap_or_else(|e| panic!("fmt failed on mangled {}: {}", path.display(), e));
        assert_eq!(
            remangled,
            formatted,
            "fmt did not undo the mangle for {}",
            path.display()
        );
        true
    });
    let checked = checked.into_iter().filter(|c| *c).count();
    assert!(
        checked > 150,
        "expected most of the corpus to be manglable, got {checked}"
    );
}

/// Add three spaces to the front of every non-empty line. Returns `None` when
/// the file has a token spanning more than one line.
fn mangle_indentation(src: &str) -> Option<String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().ok()?;
    let multiline = lexer.tokens_with_spans().any(|(token, span)| {
        !matches!(token, Token::Newline | Token::Eof) && span.end.line > span.start.line
    });
    if multiline {
        return None;
    }
    let mut out = String::with_capacity(src.len() + src.lines().count() * 3);
    for line in src.lines() {
        if !line.trim().is_empty() {
            out.push_str("   ");
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

#[test]
fn a_block_comment_keeps_its_inner_lines_as_written() {
    let src = "fn f()\n/* first\n     second\n\n\n   third */\nlet x = 1 /* in */\nend\n";
    let want = "fn f()\n  /* first\n     second\n\n\n   third */\n  let x = 1 /* in */\nend\n";
    assert_eq!(fmt(src), want);
    assert_eq!(fmt(want), want);
}

/// `@` annotations (docs/proposals/annotation-syntax.md) keep the layout
/// they were written in: on the declaration's line or on lines of their own.
/// The `@` binds to its name, the argument list is spaced like a call's, and
/// an own-line annotation is indented with its declaration.
#[test]
fn annotations_keep_their_lines() {
    let src = "@tunable( 0,10 ) let S = 4.0\n@const\n\n// table\npub let T = {\na: 1,\n}\nfn f()\n@rigid\n    let g=2\n  @tunable(min:1,max:5) let h = 3\ng+h\nend\n";
    let want = "@tunable(0, 10) let S = 4.0\n@const\n\n// table\npub let T = {\n  a: 1,\n}\nfn f()\n  @rigid\n  let g = 2\n  @tunable(min: 1, max: 5) let h = 3\n  g + h\nend\n";
    assert_eq!(fmt(src), want);
    assert_eq!(fmt(want), want);
    assert_eq!(ast_shape(src), ast_shape(want));
}

/// Indentation delimits blocks, so re-indenting is no longer free: the
/// output must be the program the input was.
#[test]
fn a_block_closed_by_indentation_keeps_its_structure() {
    // Already in canonical layout: unchanged.
    let src = "fn f(c)\n  if c then\n    print(1)\n  print(2)\nprint(f(true))\n";
    assert_eq!(fmt(src), src);
    // Spacing only: the same program, respaced.
    let spaced = "fn f(c)\n  if c then\n    print( 1 )\n  print(2)\nprint(f(true))\n";
    assert_eq!(fmt(spaced), src);
    // Any indent width becomes two columns, level by level.
    let wide = "fn f(c)\n    if c then\n          print(1)\n    print(2)\nprint(f(true))\n";
    assert_eq!(fmt(wide), src);
    assert_eq!(ast_shape(wide), ast_shape(src));
}

/// Every construct, with no `end` anywhere, written four columns deep.
#[test]
fn lays_out_every_construct_without_its_end() {
    let src = "\
fn clamp_all(xs, lo, hi)
    for x in xs do
        if x < lo then lo
        else if x > hi then hi
        else x
fn g(c)
    if c then
        print(1)
    else if c == 2 then
        print(3)
    else
        print(2)
    match c
        when 1 ->
            let a = 1
            a
        when 2 -> 3
        when 3 do
            4
    let f = fn(x)
        x * 2
    defer_paint(fn()
        draw(1)
    )
    while c do
        c -= 1
    let r = for i in [1, 2] do i * i
    let h = fn(v) v * 2
    r
class P
    x: int,
    y: int,
enum Shape
    circle(radius),
    rect(w, h),
test \"t\" do
    expect_eq(1 + 1, 2)
fn box(w) \"{w}\"
print(box(1))
";
    let want = "\
fn clamp_all(xs, lo, hi)
  for x in xs do
    if x < lo then lo
    else if x > hi then hi
    else x
fn g(c)
  if c then
    print(1)
  else if c == 2 then
    print(3)
  else
    print(2)
  match c
    when 1 ->
      let a = 1
      a
    when 2 -> 3
    when 3 do
      4
  let f = fn(x)
    x * 2
  defer_paint(fn()
    draw(1)
  )
  while c do
    c -= 1
  let r = for i in [1, 2] do i * i
  let h = fn(v) v * 2
  r
class P
  x: int,
  y: int,
enum Shape
  circle(radius),
  rect(w, h),
test \"t\" do
  expect_eq(1 + 1, 2)
fn box(w) \"{w}\"
print(box(1))
";
    assert_eq!(fmt(src), want);
    assert_eq!(fmt(want), want);
    assert_eq!(ast_shape(src), ast_shape(want));
}

/// A file may close some blocks with `end` and others by indentation.
#[test]
fn lays_out_a_mix_of_ended_and_unended_blocks() {
    let src = "\
fn f(c)
      if c then
            for i in [1] do
                  print(i)
            end
      else
            match c
                when 1 -> 2
                when _ ->
                    if c then 3 else 4 end
      let xs = map([1], fn(v)
            v + 1
      end)
      xs
end
fn g()
   let y = if a then 1
           else 2
   y
";
    let want = "\
fn f(c)
  if c then
    for i in [1] do
      print(i)
    end
  else
    match c
      when 1 -> 2
      when _ ->
        if c then 3 else 4 end
  let xs = map([1], fn(v)
    v + 1
  end)
  xs
end
fn g()
  let y = if a then 1
          else 2
  y
";
    assert_eq!(fmt(src), want);
    assert_eq!(fmt(want), want);
    assert_eq!(ast_shape(src), ast_shape(want));
}

/// A comment after a block that indentation closed belongs to the level it
/// was lined up with, whatever the indent width.
#[test]
fn a_comment_after_an_unended_block_keeps_its_level() {
    let src = "\
fn f(c)
    if c then
        a()
        # still in the if
    # the rest of f
    b()
    # the last line of f
# about g
fn g()
    1
    # in g, at the end of the file
";
    let want = "\
fn f(c)
  if c then
    a()
    # still in the if
  # the rest of f
  b()
  # the last line of f
# about g
fn g()
  1
  # in g, at the end of the file
";
    assert_eq!(fmt(src), want);
    assert_eq!(fmt(want), want);
}

/// Tabs in indentation are an error to the language (B11) and fmt's to
/// convert: by the file's `end`s when it has them, and otherwise when every
/// reading of a tab's width gives the same program.
#[test]
fn converts_tabs_in_indentation() {
    let ended = "fn f(c)\n\tif c then\n\t\tprint(1)\n\tend\nend\n";
    assert_eq!(fmt(ended), "fn f(c)\n  if c then\n    print(1)\n  end\nend\n");
    let unended = "fn f(c)\n\tif c then\n\t\tprint(1)\n\tprint(2)\nf(1)\n";
    assert!(crate::rewrite::parse_ast(unended).is_err());
    let want = "fn f(c)\n  if c then\n    print(1)\n  print(2)\nf(1)\n";
    assert_eq!(fmt(unended), want);
    assert_eq!(ast_shape(want), ast_shape(&unended.replace('\t', "    ")));
    // A tab that is one level or two depending on its width: refused, with
    // the language's own error.
    let unclear = "fn f(c)\n  if c then\n    print(1)\n\tprint(2)\nf(1)\n";
    let e = format_source(unclear).unwrap_err();
    assert!(e.contains("tab"), "{e}");
}

/// What `petal fmt` cannot give a meaning to, it refuses: nothing is ever
/// guessed from a layout the parser rejects.
#[test]
fn refuses_unended_blocks_whose_indentation_does_not_parse() {
    let src = "fn f(c)\n  if c then\n      print(1)\n    print(2)\n";
    assert!(format_source(src).is_err());
}

#[test]
fn remove_block_ends_drops_the_redundant_ones() {
    let src = "\
fn f(c)
  if c then
    for i in [1] do
      print(i)
    end # each
  else
    print(0)
  end
  let xs = map([1], fn(v)
    v + 1
  end)
  let g = fn()
  end
  let y = if c then 1 else 2 end
  let z = for i in [1] do
    i
  end |> len
  let w = if c then
    [1]
  else
    [2]
  end
    ++ [3]
  slice(xs, start: 0, end: 1)
end
";
    let want = "\
fn f(c)
  if c then
    for i in [1] do
      print(i)
      # each
  else
    print(0)
  let xs = map([1], fn(v)
    v + 1
  )
  let g = fn()
  end
  let y = if c then 1 else 2 end
  let z = for i in [1] do
    i
  end |> len
  let w = if c then
    [1]
  else
    [2]
  end
    ++ [3]
  slice(xs, start: 0, end: 1)
";
    let out = remove_block_ends(src).unwrap();
    assert_eq!(out.text, want);
    assert_eq!(out.removed, 4);
    assert_eq!(out.kept, vec![13, 17, 22]);
    assert_eq!(ast_shape(src), ast_shape(want));
    // Laid out already, and nothing left to remove.
    assert_eq!(fmt(want), want);
    let again = remove_block_ends(want).unwrap();
    assert_eq!((again.text.as_str(), again.removed), (want, 0));
}

/// An `end` the rules would drop but the program needs is found and kept.
#[test]
fn remove_block_ends_keeps_an_end_the_parse_needs() {
    // `if c then a; b` with no `end` is ambiguous (B3).
    let src = "fn f(c)\n  if c then a(); b()\n  end\n  c\nend\n";
    let out = remove_block_ends(src).unwrap();
    assert_eq!(out.text, "fn f(c)\n  if c then a(); b()\n  end\n  c\n");
    assert_eq!((out.removed, out.kept), (1, vec![3]));
}

/// The `end`-less spelling of every repo file: formatted, then stripped of
/// its redundant `end`s. It must be the same program, already laid out, and
/// what formatting gives back from any other indentation of it.
#[test]
#[ignore = "expensive corpus sweep: run with `make expensive_lint_tests`"]
fn fmt_lays_out_blocks_without_ends_over_repo_corpus() {
    // The repo's own files have lost their `end` lines to the migration
    // that calls this transform; the copies of ten of them taken with their
    // `end`s are what still gives it something to remove.
    let mut files = crate::test_corpus::repo_ptl_files();
    files.extend(crate::test_corpus::with_ends_fixtures());
    let checked = crate::test_corpus::par_map(&files, |path| {
        let Ok(src) = std::fs::read_to_string(path) else {
            return (0, 0);
        };
        let Ok(formatted) = format_source(&src) else {
            return (0, 0);
        };
        if formatted.contains(OFF) || formatted.contains(IGNORE_NEXT) {
            return (0, 0);
        }
        let stripped = remove_block_ends(&formatted)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let text = stripped.text;
        assert_eq!(ast_shape(&formatted), ast_shape(&text), "{}", path.display());
        let again = format_source(&text)
            .unwrap_or_else(|e| panic!("fmt failed without `end`s: {}: {e}", path.display()));
        assert_eq!(again, text, "not a fixed point without `end`s: {}", path.display());
        // Indented as a whole (B12), and with every indent doubled.
        if let Some(mangled) = mangle_indentation(&text) {
            let back = format_source(&mangled)
                .unwrap_or_else(|e| panic!("fmt failed on mangled {}: {e}", path.display()));
            assert_eq!(back, text, "fmt did not undo the mangle for {}", path.display());
            let doubled: String = text
                .lines()
                .map(|l| {
                    let indent = l.len() - l.trim_start_matches(' ').len();
                    format!("{}{l}\n", " ".repeat(indent))
                })
                .collect();
            let back = format_source(&doubled)
                .unwrap_or_else(|e| panic!("fmt failed on doubled {}: {e}", path.display()));
            assert_eq!(ast_shape(&back), ast_shape(&text), "{}", path.display());
            assert_eq!(format_source(&back).unwrap(), back, "{}", path.display());
        }
        (1, stripped.removed)
    });
    let files = checked.iter().map(|c| c.0).sum::<usize>();
    let removed = checked.iter().map(|c| c.1).sum::<usize>();
    assert!(files > 150, "expected a real corpus, checked {files}");
    assert!(removed > 300, "expected the `end` lines of the copies to go, removed {removed}");
}
