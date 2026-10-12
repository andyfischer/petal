//! Enums: variants are members of their enum, may carry payload fields and
//! constant data, and are read, compared, printed and reflected on through
//! it. One section per rule of docs/language-guide.md (Enums), compile errors
//! included. Variant *patterns* are covered here in their qualified form;
//! the rest of `match` has its own tests.

use petal::ast::{ExprKind, Literal, Stmt, StmtKind};
use petal::env::Env;
use petal::lexer::Lexer;
use petal::parse::Parser;

fn env_with(modules: &[(&str, &str)]) -> Env {
    let mut env = Env::new();
    env.set_echo(false);
    for (name, source) in modules {
        env.register_module(name, source);
    }
    env
}

/// Run `src` and return what it printed, one entry per `print`.
fn run(src: &str) -> Vec<String> {
    run_with(&[], src)
}

fn run_with(modules: &[(&str, &str)], src: &str) -> Vec<String> {
    let mut env = env_with(modules);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_or_else(|e| panic!("{src}\n-- failed to run: {e}"));
    env.take_output()
}

/// Compile `src` expecting a compile error; return the message.
fn compile_err(src: &str) -> String {
    compile_err_with(&[], src)
}

fn compile_err_with(modules: &[(&str, &str)], src: &str) -> String {
    match env_with(modules).load_program(src) {
        Ok(_) => panic!("expected a compile error, but this compiled:\n{src}"),
        Err(e) => e,
    }
}

/// Run `src` expecting it to compile and then fail; return the message.
fn run_err(src: &str) -> String {
    let mut env = env_with(&[]);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let sid = env.create_stack(pid).unwrap();
    match env.run(sid) {
        Ok(_) => panic!("expected a runtime error, but this ran:\n{src}"),
        Err(e) => e,
    }
}

/// The compiler's warnings for `src` — what `petal check` prints as warnings.
fn warnings(src: &str) -> Vec<String> {
    let mut env = env_with(&[]);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let program = env.get_program(pid).unwrap();
    program.warnings.iter().map(|d| d.message.clone()).collect()
}

fn parse(src: &str) -> Vec<Stmt> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().unwrap();
    Parser::new(lexer.tokens, lexer.token_spans)
        .parse_program()
        .unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"))
}

const COLOR: &str = "\
enum Color
  red,
  green,
  custom(r, g, b),
end
";

const KIND: &str = "\
enum Kind
  parkway(shape: \"route\", use: Use.road),
  park(shape: \"area\", use: Use.green),
end
enum Use
  road,
  green,
end
";

const OP: &str = "\
enum Op
  fight(place, won, tone: \"bad\"),
  gift(amount, tone: \"good\"),
end
";

// ── Declaration ──────────────────────────────────────────────────

#[test]
fn a_variant_declares_payload_fields_and_constant_data() {
    let stmts = parse(OP);
    let StmtKind::EnumDecl { name, variants } = &stmts[0].kind else {
        panic!("expected an EnumDecl");
    };
    assert_eq!(name, "Op");
    let fight = &variants[0];
    assert_eq!(fight.name, "fight");
    // A bare identifier is a payload field; `key: expr` is constant data.
    assert_eq!(fight.fields, ["place", "won"]);
    assert_eq!(fight.data.len(), 1);
    assert_eq!(fight.data[0].key, "tone");
    assert!(matches!(
        &fight.data[0].value.kind,
        ExprKind::Literal(Literal::String(s)) if s == "bad"
    ));
    assert_eq!(variants[1].fields, ["amount"]);
    // The variant's own span, for a diagnostic about it.
    assert_eq!((fight.span.start.line, fight.span.start.column), (2, 3));
}

#[test]
fn a_variant_name_is_any_identifier() {
    let src = "enum Mixed\n  lower,\n  Upper,\n  snake_case,\n  X1(v),\nend\n\
               print(Mixed.lower, Mixed.Upper, Mixed.snake_case, Mixed.X1(1))";
    assert_eq!(run(src), ["Mixed.lower Mixed.Upper Mixed.snake_case Mixed.X1(1)"]);
}

#[test]
fn every_variant_declares_the_same_data_keys() {
    // A key most variants have is missing from the one without it…
    let e = compile_err("enum K\n  a(s: 1, u: 2),\n  b(s: 1),\n  c(s: 3, u: 4),\nend");
    assert!(
        e.contains("variant `K.b` is missing the data key `u`, which `K.a` declares"),
        "{e}"
    );
    assert!(e.contains("line 3"), "{e}");
    // …and a key only one has is extra on that one.
    let e = compile_err("enum K\n  a(s: 1),\n  b(s: 1, w: 2),\n  c(s: 3),\nend");
    assert!(e.contains("variant `K.b` has the extra data key `w`"), "{e}");
    assert!(!e.contains("missing"), "{e}");
    // A variant with no parens at all is missing every key.
    let e = compile_err("enum K\n  a(s: 1),\n  b,\nend");
    assert!(e.contains("variant `K.b` is missing the data key `s`"), "{e}");
    // Payload fields are not data keys: they may differ freely.
    assert_eq!(run(&format!("{OP}print(Op.gift(3).tone)")), ["good"]);
}

#[test]
fn constant_data_is_evaluated_once_per_declaration() {
    // Not per read and not per construction: the initializer's `print` runs
    // once however often the data is read or the variant built.
    let src = "\
fn noisy(v)
  print(\"evaluated\", v)
  v
end
enum Op
  fight(place, tone: noisy(\"bad\")),
  rest(tone: noisy(\"calm\")),
end
let a = Op.fight(\"x\")
let b = Op.fight(\"y\")
print(a.tone, b.tone, a.tone, Op.rest.tone, Op.rest.tone)
";
    assert_eq!(
        run(src),
        ["evaluated bad", "evaluated calm", "bad bad bad calm calm"]
    );
}

#[test]
fn constant_data_may_name_a_variant_of_any_enum_in_any_order() {
    // `Use` is declared below `Kind`, whose data names its variants.
    let src = format!("{KIND}print(Kind.parkway.use, Kind.park.use == Use.green)");
    assert_eq!(run(&src), ["Use.road true"]);
    // Two enums may refer to each other.
    let src = "\
enum A
  x(other: B.y),
end
enum B
  y(other: A.x),
end
print(A.x.other, B.y.other, A.x.other.other == A.x)
";
    assert_eq!(run(src), ["B.y A.x true"]);
    // Hoisted constants and functions are available too.
    let src = "\
enum Tile
  grass(cost: BASE),
  swamp(cost: double(BASE)),
end
let BASE = 2
fn double(n)
  n * 2
end
print(Tile.grass.cost, Tile.swamp.cost)
";
    assert_eq!(run(src), ["2 4"]);
}

#[test]
fn constant_data_cannot_read_what_the_file_computes_as_it_runs() {
    let src = "fn compute()\n  3\nend\nlet x = compute()\nenum K\n  a(v: x),\nend\nprint(K.a.v)";
    let e = run_err(src);
    assert!(
        e.contains("`x` cannot be used in an enum's constant data"),
        "{e}"
    );
}

#[test]
fn reserved_and_duplicate_names_are_compile_errors() {
    // A variant may not be named `all` or `from`.
    let e = compile_err("enum E\n  all,\nend");
    assert!(e.contains("a variant may not be named `all`"), "{e}");
    let e = compile_err("enum E\n  from(x),\nend");
    assert!(e.contains("a variant may not be named `from`"), "{e}");
    // A payload field or data key may not be named `name`.
    let e = compile_err("enum E\n  a(name),\nend");
    assert!(
        e.contains("variant `E.a` may not have a payload field named `name`"),
        "{e}"
    );
    let e = compile_err("enum E\n  a(name: 1),\nend");
    assert!(
        e.contains("variant `E.a` may not have a data key named `name`"),
        "{e}"
    );
    // Payload fields and data keys of one variant are distinct.
    for body in ["a(x, x)", "a(k: 1, k: 2)", "a(x, x: 1)"] {
        let e = compile_err(&format!("enum E\n  {body},\nend"));
        assert!(e.contains("variant `E.a` declares"), "{body}: {e}");
        assert!(e.contains("twice"), "{body}: {e}");
    }
    // So are the variants of one enum.
    let e = compile_err("enum E\n  a,\n  a,\nend");
    assert!(e.contains("enum `E` declares the variant `a` twice"), "{e}");
}

#[test]
fn a_variant_field_takes_no_default() {
    let mut lexer = Lexer::new("enum Shape\n  circle(r = 1),\nend\n");
    lexer.tokenize().unwrap();
    let e = Parser::new(lexer.tokens, lexer.token_spans)
        .parse_program()
        .unwrap_err();
    assert!(e.contains("cannot have default values"), "{e}");
    assert!(e.contains("key: value"), "{e}");
}

#[test]
fn an_enum_is_hoisted_like_a_class() {
    // Used above its declaration, at the top level and inside a function.
    let src = "\
print(Color.red, pick())
fn pick()
  Color.custom(1, 2, 3)
end
enum Color
  red,
  custom(r, g, b),
end
";
    assert_eq!(run(src), ["Color.red Color.custom(1, 2, 3)"]);
}

#[test]
fn an_enum_is_declared_at_the_top_level_only() {
    let e = compile_err("fn f()\n  enum Inner\n    a,\n  end\nend");
    assert!(
        e.contains("enum `Inner` must be declared at the top level of a file"),
        "{e}"
    );
    let e = compile_err("if true then\n  enum Inner\n    a,\n  end\nend");
    assert!(e.contains("must be declared at the top level"), "{e}");
}

#[test]
fn duplicate_enum_names_follow_the_class_rules() {
    let e = compile_err("enum E\n  a,\nend\nenum E\n  b,\nend");
    assert!(e.contains("enum `E` is already declared"), "{e}");
    // Across modules, naming both files.
    let e = compile_err_with(
        &[("lib", "pub enum E\n  a,\nend")],
        "import lib\nenum E\n  b,\nend",
    );
    assert!(e.contains("enum `E` is already declared in `lib"), "{e}");
    assert!(e.contains("may not"), "{e}");
    // A class and an enum share the one namespace of declared type names.
    let e = compile_err("class E\n  x,\nend\nenum E\n  a,\nend");
    assert!(e.contains("enum `E` has the same name as class `E`"), "{e}");
    let e = compile_err("enum int\n  a,\nend");
    assert!(
        e.contains("enum `int` collides with the built-in type name `int`"),
        "{e}"
    );
}

#[test]
fn an_enum_name_is_a_type_name() {
    let src = format!(
        "{COLOR}fn keep(c: Color) -> Color\n  c\nend\nlet c: Color = Color.red\nprint(keep(c))"
    );
    assert!(warnings(&src).is_empty(), "{:?}", warnings(&src));
    assert_eq!(run(&src), ["Color.red"]);
}

// ── Variants are members, never globals ──────────────────────────

#[test]
fn a_variant_is_a_value_a_construction_or_a_constructor() {
    let src = format!(
        "{COLOR}\
         let make = Color.custom\n\
         print(Color.red)\n\
         print(Color.custom(1, 2, 3))\n\
         print(make(4, 5, 6))\n\
         print(map([1, 2], fn(n) Color.custom(n, n, n) end))\n\
         print(Color.custom(b: 3, r: 1, g: 2))"
    );
    assert_eq!(
        run(&src),
        [
            "Color.red",
            "Color.custom(1, 2, 3)",
            "Color.custom(4, 5, 6)",
            "[Color.custom(1, 1, 1), Color.custom(2, 2, 2)]",
            "Color.custom(1, 2, 3)",
        ]
    );
}

#[test]
fn a_construction_is_held_to_the_payload_it_declares() {
    let e = compile_err(&format!("{COLOR}print(Color.custom(1, 2))"));
    assert!(
        e.contains("Color.custom takes 3 arguments (r, g, b), got 2"),
        "{e}"
    );
    let e = compile_err(&format!("{COLOR}print(Color.red(1))"));
    assert!(
        e.contains("Color.red has no payload, so it takes no arguments, got 1"),
        "{e}"
    );
    // Empty parens on a payload-free variant are the variant, as in its
    // declaration.
    let src = "enum Event\n  none(),\n  add(text),\nend\nprint(Event.none(), Event.none() == Event.none)";
    assert_eq!(run(src), ["Event.none true"]);
}

#[test]
fn a_bare_variant_does_not_resolve() {
    // The undefined-name error says which enum declares it.
    let e = run_err(&format!("{COLOR}print(red)"));
    assert!(
        e.contains("Undefined variable: red — did you mean Color.red?"),
        "{e}"
    );
    let e = run_err(&format!("{COLOR}print(custom(1, 2, 3))"));
    assert!(
        e.contains("Unknown function: custom — did you mean Color.custom?"),
        "{e}"
    );
    // Inside a function too, and when two enums declare the name.
    let e = run_err(&format!(
        "{COLOR}enum Light\n  red,\nend\nfn f()\n  red\nend\nprint(f())"
    ));
    assert!(e.contains("did you mean Color.red? (also Light.red)"), "{e}");
    // A name no enum declares gets no such hint.
    let e = run_err(&format!("{COLOR}print(blue)"));
    assert!(e.contains("Undefined variable: blue"), "{e}");
    assert!(!e.contains("did you mean"), "{e}");
}

#[test]
fn an_unknown_variant_of_a_known_enum_is_a_compile_error() {
    let e = compile_err(&format!("{COLOR}print(Color.rde)"));
    assert!(
        e.contains("enum `Color` has no variant `rde` — did you mean Color.red?"),
        "{e}"
    );
    let e = compile_err(&format!("{COLOR}print(Color.custm(1, 2, 3))"));
    assert!(e.contains("did you mean Color.custom?"), "{e}");
    // Nothing close: the variants are listed instead.
    let e = compile_err(&format!("{COLOR}print(Color.zzzzzz)"));
    assert!(
        e.contains("enum `Color` has no variant `zzzzzz` (its variants: red, green, custom)"),
        "{e}"
    );
    // Never reached, still an error: it is a compile error, not a deferred one.
    let e = compile_err(&format!("{COLOR}fn unused()\n  Color.rde\nend"));
    assert!(e.contains("has no variant `rde`"), "{e}");
}

#[test]
fn a_local_that_shadows_the_enum_name_is_an_ordinary_value() {
    let src = format!(
        "{COLOR}fn f(Color)\n  Color.rde\nend\nprint(f({{rde: 7}}))\nlet E = Color\nprint(E.red, E.custom(1, 2, 3))"
    );
    assert_eq!(run(&src), ["7", "Color.red Color.custom(1, 2, 3)"]);
}

const SHAPES: &str = "\
pub enum Shape
  circle(r, sides: 0),
  dot(sides: 0),
  square(side, sides: 4),
end
enum Hidden
  h,
end
";

#[test]
fn an_enum_is_exported_and_imported_by_its_name() {
    // Through the module's qualifier.
    let out = run_with(
        &[("shapes", SHAPES)],
        "import shapes as s\n\
         print(s.Shape.dot, s.Shape.circle(2), s.Shape.square(3).sides)\n\
         print(s.Shape.all, s.Shape.from(\"dot\"))",
    );
    assert_eq!(
        out,
        ["Shape.dot Shape.circle(2) 4", "[Shape.dot] Shape.dot"]
    );
    // Bound directly, by a selective import and by a plain one; all three
    // spellings name the same enum.
    let out = run_with(
        &[("shapes", SHAPES)],
        "import shapes: Shape\nimport shapes as s\nprint(Shape.dot == s.Shape.dot)",
    );
    assert_eq!(out, ["true"]);
    assert_eq!(
        run_with(&[("shapes", SHAPES)], "import shapes\nprint(Shape.dot)"),
        ["Shape.dot"]
    );
    // A variant is not an export: only the enum's name is.
    let e = compile_err_with(&[("shapes", SHAPES)], "import shapes: dot");
    assert!(e.contains("dot"), "{e}");
    // An enum without `pub` is private.
    let e = compile_err_with(&[("shapes", SHAPES)], "import shapes: Hidden");
    assert!(e.contains("Hidden"), "{e}");
    // A misspelt variant through a module is the same compile error.
    let e = compile_err_with(&[("shapes", SHAPES)], "import shapes as s\nprint(s.Shape.dto)");
    assert!(e.contains("did you mean Shape.dot?"), "{e}");
}

#[test]
fn two_enums_may_declare_the_same_variant_name() {
    let src = "\
enum Kind
  park,
  road,
end
enum Use
  park,
  green,
end
print(Kind.park == Kind.park, Kind.park == Use.park, Kind.park != Use.park)
print(Kind.park, Use.park)
";
    assert_eq!(run(src), ["true false true", "Kind.park Use.park"]);
    // …and `petal check` has nothing to say about it, nor about a variant
    // named like another top-level declaration.
    assert!(warnings(src).is_empty(), "{:?}", warnings(src));
    let src = "let red = 5\nfn green()\n  1\nend\nenum Color\n  red,\n  green,\nend\nprint(red, green(), Color.red)";
    assert!(warnings(src).is_empty(), "{:?}", warnings(src));
    assert_eq!(run(src), ["5 1 Color.red"]);
}

#[test]
fn equality_is_enum_variant_and_payload() {
    let src = format!(
        "{COLOR}\
         print(Color.custom(1, 2, 3) == Color.custom(1, 2, 3))\n\
         print(Color.custom(1, 2, 3) == Color.custom(1, 2, 4))\n\
         print(Color.red == Color.green, Color.red == \"red\", Color.red == nil)\n\
         print(Color.custom(1, 2.0, 3) == Color.custom(1.0, 2, 3))\n\
         print(index_of([Color.green, Color.red], Color.red))"
    );
    assert_eq!(run(&src), ["true", "false", "false false false", "true", "1"]);
}

// ── Reading a variant ────────────────────────────────────────────

#[test]
fn a_variant_reads_its_data_its_payload_and_its_name() {
    let src = format!(
        "{KIND}{OP}\
         let k = Kind.park\n\
         let op = Op.fight(\"bridge\", true)\n\
         print(k.shape, k.use, k.name)\n\
         print(op.place, op.won, op.tone, op.name)\n\
         print(Op.gift(5).amount, Op.gift(5).tone)\n\
         print(for v in Kind.all do v.shape end)"
    );
    assert_eq!(
        run(&src),
        [
            "area Use.green park",
            "bridge true bad fight",
            "5 good",
            "[\"route\", \"area\"]",
        ]
    );
}

#[test]
fn an_unknown_field_errors_as_it_does_on_a_record() {
    let e = run_err(&format!("{KIND}print(Kind.park.colour)"));
    assert!(
        e.contains("No field 'colour' on enum variant Kind.park"),
        "{e}"
    );
    // A payload field of another variant is not a field of this one.
    let e = run_err(&format!("{OP}print(Op.gift(1).place)"));
    assert!(e.contains("No field 'place' on enum variant Op.gift"), "{e}");
    // The tolerant read is tolerant here too.
    assert_eq!(
        run(&format!("{KIND}print(Kind.park.colour ?? \"none\", Kind.park?.shape)")),
        ["none area"]
    );
}

#[test]
fn type_is_the_enums_name_and_str_is_the_qualified_variant() {
    let src = format!(
        "{KIND}{OP}\
         print(type(Kind.park), type(Op.fight(\"x\", true)))\n\
         print(str(Kind.park), str(Op.fight(\"x\", true)))\n\
         print(\"{{Kind.park}} / {{Op.gift(2)}}\")\n\
         print([Kind.park, Op.gift(2.5)], {{k: Kind.parkway}})"
    );
    assert_eq!(
        run(&src),
        [
            "Kind Op",
            "Kind.park Op.fight(\"x\", true)",
            "Kind.park / Op.gift(2)",
            "[Kind.park, Op.gift(2.5)] { k: Kind.parkway }",
        ]
    );
}

// ── Reflection ───────────────────────────────────────────────────

#[test]
fn all_lists_the_payload_free_variants_in_declaration_order() {
    assert_eq!(
        run(&format!("{KIND}print(Kind.all)\nprint(len(Use.all), Use.all[1])")),
        ["[Kind.parkway, Kind.park]", "2 Use.green"]
    );
    // A payload variant is not in the list.
    assert_eq!(
        run(&format!("{COLOR}{OP}print(Color.all, Op.all)")),
        ["[Color.red, Color.green] []"]
    );
}

#[test]
fn from_finds_a_payload_free_variant_by_name() {
    let src = format!(
        "{KIND}{COLOR}\
         print(Kind.from(\"park\"), Kind.from(\"lake\"), Kind.from(\"park\") == Kind.park)\n\
         print(Color.from(\"custom\"), Color.from(3), Color.from(nil))\n\
         print(Kind.from(\"parkway\").shape)\n\
         print(map([\"park\", \"nope\"], Kind.from))"
    );
    assert_eq!(
        run(&src),
        [
            "Kind.park nil true",
            "nil nil nil",
            "route",
            "[Kind.park, nil]",
        ]
    );
}

// ── Patterns ─────────────────────────────────────────────────────

#[test]
fn a_variant_pattern_is_qualified() {
    let src = format!(
        "{KIND}{OP}\
         fn describe(v)\n\
         \x20 match v\n\
         \x20   when Kind.park -> \"a park\"\n\
         \x20   when Kind.parkway -> \"a parkway\"\n\
         \x20   when Op.fight(place, true) -> \"won at {{place}}\"\n\
         \x20   when Op.fight(place, _) -> \"lost at {{place}}\"\n\
         \x20   when Op.gift(Kind.park) -> \"a park, as a gift\"\n\
         \x20   when [Kind.park, x] -> \"a park and {{x}}\"\n\
         \x20   when _ -> \"other\"\n\
         \x20 end\n\
         end\n\
         print(describe(Kind.park), describe(Kind.parkway))\n\
         print(describe(Op.fight(\"x\", true)), describe(Op.fight(\"y\", false)))\n\
         print(describe(Op.gift(Kind.park)), describe([Kind.park, 2]))\n\
         print(describe(Use.road), describe(\"park\"), describe(Op.gift(1)))"
    );
    assert_eq!(
        run(&src),
        [
            "a park a parkway",
            "won at x lost at y",
            "a park, as a gift a park and 2",
            "other other other",
        ]
    );
}

#[test]
fn a_same_named_variant_of_another_enum_does_not_match() {
    let src = "\
enum Kind
  park,
end
enum Use
  park,
end
fn which(v)
  match v
    when Kind.park -> \"kind\"
    when Use.park -> \"use\"
  end
end
print(which(Use.park), which(Kind.park))
";
    assert_eq!(run(src), ["use kind"]);
}

#[test]
fn a_variant_pattern_resolves_through_a_module() {
    let out = run_with(
        &[("shapes", SHAPES)],
        "import shapes as s\n\
         import shapes: Shape\n\
         fn f(v)\n\
         \x20 match v\n\
         \x20   when s.Shape.dot -> \"dot\"\n\
         \x20   when s.Shape.circle(r) -> \"circle {r}\"\n\
         \x20   when Shape.square(_) -> \"square\"\n\
         \x20 end\n\
         end\n\
         print(f(Shape.dot), f(Shape.circle(2)), f(s.Shape.square(1)))",
    );
    assert_eq!(out, ["dot circle 2 square"]);
}

#[test]
fn a_bare_identifier_in_a_pattern_is_a_binding() {
    // `red` binds whatever is matched; it is not the variant.
    let src = format!("{COLOR}print(match Color.green\n  when red -> red\nend)");
    assert_eq!(run(&src), ["Color.green"]);
}

#[test]
fn a_pattern_that_names_no_variant_is_a_compile_error() {
    // A call-shaped pattern with no enum in front.
    let e = compile_err(&format!("{COLOR}match Color.red\n  when custom(r, g, b) -> 1\nend"));
    assert!(e.contains("`custom(…)` is not a pattern"), "{e}");
    assert!(e.contains("did you mean Color.custom?"), "{e}");
    // An unknown variant, an unknown enum.
    let e = compile_err(&format!("{COLOR}match Color.red\n  when Color.rde -> 1\nend"));
    assert!(e.contains("has no variant `rde` — did you mean Color.red?"), "{e}");
    let e = compile_err(&format!("{COLOR}match Color.red\n  when Colour.red -> 1\nend"));
    assert!(e.contains("`Colour` is not an enum in scope"), "{e}");
    // A payload pattern lists every payload field.
    let e = compile_err(&format!("{COLOR}match Color.red\n  when Color.custom(r) -> 1\nend"));
    assert!(
        e.contains("the pattern `Color.custom` has 1 field, but Color.custom has 3 payload fields (r, g, b)"),
        "{e}"
    );
    assert!(e.contains("`Color.custom(_, _, _)`"), "{e}");
    let e = compile_err(&format!("{COLOR}match Color.red\n  when Color.red(x) -> 1\nend"));
    assert!(e.contains("Color.red has no payload"), "{e}");
}

// ── Across runs ──────────────────────────────────────────────────

#[test]
fn variants_survive_collection_and_runs_and_the_ir_round_trip() {
    // A variant held in `state`, with garbage churned around it, still reads
    // its payload and its data on a later run.
    let src = format!(
        "{OP}\
         state kept = Op.fight(\"bridge\", true)\n\
         for i in range(0, 3000) do\n  let tmp = Op.gift(i)\nend\n\
         print(kept, kept.place, kept.tone, Op.gift(1).tone)"
    );
    let mut env = env_with(&[]);
    let pid = env.load_program(&src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for _ in 0..3 {
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(
            env.take_output(),
            ["Op.fight(\"bridge\", true) bridge bad good"]
        );
    }
    // The declared shapes are part of the program, serialized with it.
    let program = env.get_program(pid).unwrap();
    let fight = program.enum_variant("Op.fight").expect("declared");
    assert_eq!(fight.fields, ["place", "won"]);
    assert_eq!(fight.data, ["tone"]);
    assert!(program.enum_variant("Op.nope").is_none());
    let json = serde_json::to_string(program).unwrap();
    let mut other = env_with(&[]);
    let pid = other.load_program_ir(&json).unwrap();
    let sid = other.create_stack(pid).unwrap();
    other.run(sid).unwrap();
    assert_eq!(
        other.take_output(),
        ["Op.fight(\"bridge\", true) bridge bad good"]
    );
}

#[test]
fn a_variant_kept_in_state_reads_the_data_its_enum_declares_now() {
    let old = "\
enum Kind
  park(shape: \"area\"),
  road(shape: \"route\"),
end
state kept = Kind.park
print(kept, kept.shape)
";
    let mut env = env_with(&[]);
    let pid = env.load_program(old).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["Kind.park area"]);
    // Edit the data: the value in `state` is the same variant, and its data
    // is whatever the declaration says after the reload.
    let new = old.replace("\"area\"", "\"green\"");
    env.reload_program(sid, &new, None).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["Kind.park green"]);
    // Add a key: the kept variant answers it too.
    let new = "\
enum Kind
  park(shape: \"green\", cost: 3),
  road(shape: \"route\", cost: 9),
end
state kept = Kind.park
print(kept, kept.shape, kept.cost)
";
    env.reload_program(sid, new, None).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["Kind.park green 3"]);
}
