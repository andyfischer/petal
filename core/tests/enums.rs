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

fn warnings_with(modules: &[(&str, &str)], src: &str) -> Vec<String> {
    let mut env = env_with(modules);
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
fn constant_data_cannot_call_a_function_that_is_not_hoisted() {
    // `scaled` reads `base`, which the file computes as it runs, so it is
    // not hoisted and is not there when the data is evaluated. That is said
    // by name (`petal check` reports it, and a run stops on it) rather than
    // as `Cannot call nil`.
    let e = run_err(
        "let base = len(\"abc\") + 1\nfn scaled(n)\n  n * base\nend\n\
         enum Kind\n  park(cost: scaled(3)),\n  road(cost: 9),\nend\nprint(Kind.park.cost)",
    );
    assert!(e.contains("`scaled` cannot be used in an enum's constant data"), "{e}");
    assert!(e.contains("line 6"), "{e}");
    // A hoisted function is fine, whichever side of the enum it is on.
    let out = run(
        "enum Kind\n  park(cost: twice(3)),\n  road(cost: twice(4)),\nend\n\
         fn twice(n)\n  n * 2\nend\nprint(Kind.park.cost, Kind.road.cost)",
    );
    assert_eq!(out, ["6 8"]);
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
    // In a method's signature too, with the enum declared below the method.
    let src = "class Pen\n  w,\nend\nfn Pen.ink(p, c: Shade) -> Shade\n  c\nend\n\
               enum Shade\n  dark,\n  light,\nend\nlet p = Pen(1)\n";
    let ok = format!("{src}print(p.ink(Shade.dark))");
    assert!(warnings(&ok).is_empty(), "{:?}", warnings(&ok));
    assert_eq!(run(&ok), ["Shade.dark"]);
    let w = warnings(&format!("{src}p.ink(\"dark\")"));
    assert!(
        w.iter()
            .any(|w| w.contains("argument 1 to `ink`: expected `Shade`, found `string`")),
        "{w:?}"
    );
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
        e.contains("the pattern `Color.custom(…)` has 1 field, but Color.custom has 3 payload fields (r, g, b)"),
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

// ── Static checks: an enum's name is the type of its variants ───────────

const TOWN: &str = "\
enum Use
  road, green
end
enum Kind
  parkway(shape: \"route\", use: Use.road),
  park(shape: \"area\", use: Use.green),
end
enum Op
  fight(place, won, tone: \"bad\"),
  gift(amount, tone: \"good\"),
end
";

#[test]
fn an_enum_typed_slot_holds_a_variant_of_that_enum_and_nothing_else() {
    // As a parameter, a return type, a `let` and a class field: no warning,
    // and in particular no `unknown type name`.
    let ok = format!(
        "{TOWN}class Spot\n  kind: Kind,\nend\n\
         fn pick(k: Kind, n: int) -> Kind\n  if n > 0 then k else Kind.park end\nend\n\
         let k: Kind = Kind.parkway\n\
         let found: Kind = Kind.from(\"park\")\n\
         print(pick(k, 1), pick(Kind.park, 0), pick(found, 1), Spot(k).kind)"
    );
    assert!(warnings(&ok).is_empty(), "{:?}", warnings(&ok));
    assert_eq!(run(&ok), ["Kind.parkway Kind.park Kind.park Kind.parkway"]);

    // Another enum's variant, or a value that is no variant, does not fit.
    let w = warnings(&format!("{TOWN}let k: Kind = Use.road"));
    assert_eq!(w, ["type mismatch: `k` declared `Kind` but assigned `Use`"]);
    let w = warnings(&format!(
        "{TOWN}fn pick(k: Kind) -> Kind\n  k\nend\nprint(pick(Use.road), pick(3))"
    ));
    assert_eq!(
        w,
        [
            "argument 1 to `pick`: expected `Kind`, found `Use`",
            "argument 1 to `pick`: expected `Kind`, found `int`"
        ]
    );
    let w = warnings(&format!("{TOWN}fn pick() -> Kind\n  3\nend\nprint(pick())"));
    assert_eq!(w, ["return type mismatch: `pick` declares `Kind` but returns `int`"]);
    let w = warnings(&format!("{TOWN}fn n(k: Kind) -> int\n  k\nend\nprint(n(Kind.park))"));
    assert_eq!(w, ["return type mismatch: `n` declares `int` but returns `Kind`"]);
    // A payload variant's construction is a value of the enum; uncalled, it
    // is a function.
    let w = warnings(&format!("{TOWN}let a: Op = Op.gift(1)\nlet b: Op = Op.gift"));
    assert_eq!(w, ["type mismatch: `b` declared `Op` but assigned `function`"]);
    // The general `enum` type takes any enum's variant, and gives one back.
    let any = format!("{TOWN}fn show(e: enum) -> enum\n  e\nend\nlet k: Kind = show(Kind.park)");
    assert!(warnings(&any).is_empty(), "{:?}", warnings(&any));
}

#[test]
fn an_enum_type_reaches_an_importer_bare_and_through_its_module() {
    let town = TOWN.replace("enum ", "pub enum ");
    let modules = [("town", town.as_str())];
    let ok = "import town\nimport town as t\n\
              fn pick(k: Kind) -> Kind\n  k\nend\n\
              print(pick(Kind.park), pick(t.Kind.parkway).shape)";
    assert!(warnings_with(&modules, ok).is_empty(), "{:?}", warnings_with(&modules, ok));
    assert_eq!(run_with(&modules, ok), ["Kind.park route"]);
    let w = warnings_with(
        &modules,
        "import town as t\nimport town: Kind\n\
         fn pick(k: Kind) -> Kind\n  k\nend\n\
         print(pick(t.Use.road), t.Kind.park.shap, pick(t.Kind.park).colour)",
    );
    assert_eq!(
        w,
        [
            "argument 1 to `pick`: expected `Kind`, found `Use`",
            "`Kind.park` has no field `shap` (it has: shape, use, name)",
            "no variant of enum `Kind` has a field `colour` (fields of `Kind`: shape, use, name)"
        ]
    );
}

#[test]
fn a_field_no_variant_answers_is_a_check_warning() {
    // A value of the enum: the data keys, `name`, and any variant's payload
    // field are fine; anything else can only fail.
    let w = warnings(&format!(
        "{TOWN}fn f(k: Kind, op: Op)\n  print(k.shape, k.use, k.name, op.place, op.amount, op.tone)\n  \
         print(k.shap, op.plce)\nend\nf(Kind.park, Op.gift(1))"
    ));
    assert_eq!(
        w,
        [
            "no variant of enum `Kind` has a field `shap` (fields of `Kind`: shape, use, name)",
            "no variant of enum `Op` has a field `plce` (fields of `Op`: tone, place, won, amount, name)"
        ]
    );
    // A variant written in place is held to its own fields: `Op.gift` has no
    // `place`, though `Op.fight` does.
    let w = warnings(&format!(
        "{TOWN}print(Kind.park.colour, Op.gift(1).place, Op.gift(1).amount, Kind.park.name)"
    ));
    assert_eq!(
        w,
        [
            "`Kind.park` has no field `colour` (it has: shape, use, name)",
            "`Op.gift` has no field `place` (it has: amount, tone, name)"
        ]
    );
    // An un-annotated binding is typed by what it was given, and a `match`
    // arm or an `if` that yields variants of one enum is a value of it.
    let w = warnings(&format!(
        "{TOWN}let a = Kind.park\nlet b = if a == Kind.park then Kind.parkway else a end\n\
         print(a.shap, b.usee)"
    ));
    assert_eq!(w.len(), 2, "{w:?}");
    // `??` and `?.` ask for absence to be tolerated, as they do on a class.
    let quiet = format!("{TOWN}let a = Kind.park\nprint(a.colour ?? \"none\", a?.colour)");
    assert!(warnings(&quiet).is_empty(), "{:?}", warnings(&quiet));
    // What the checker warns about is what fails at run time.
    let e = run_err(&format!("{TOWN}let a = Kind.park\nprint(a.shap)"));
    assert!(e.contains("shap"), "{e}");
}

#[test]
fn a_rebound_or_shadowed_enum_name_is_not_typed_as_the_enum() {
    // A local named like the enum is an ordinary value: no member typing.
    let src = format!("{TOWN}fn f(Kind)\n  Kind.anything\nend\nprint(f({{anything: 1}}))");
    assert!(warnings(&src).is_empty(), "{:?}", warnings(&src));
    assert_eq!(run(&src), ["1"]);
    // The enum read as a value is a record of its members.
    let src = format!("{TOWN}let E = Kind\nlet r: record = Kind\nprint(E.park.shape)");
    assert!(warnings(&src).is_empty(), "{:?}", warnings(&src));
}

// ── Review fixes ─────────────────────────────────────────────────────────

/// Run `src` for `frames` runs of one stack under `policy`; what each printed.
fn frames(src: &str, policy: petal::policy::RunPolicy, frames: usize) -> Vec<Vec<String>> {
    let mut env = env_with(&[]);
    env.set_policy(policy);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let sid = env.create_stack(pid).unwrap();
    (0..frames)
        .map(|_| {
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap_or_else(|e| panic!("{src}\n-- failed to run: {e}"));
            env.take_output()
        })
        .collect()
}

#[test]
fn a_memoized_function_sees_constant_data_that_changed_between_runs() {
    // Constant data is evaluated by every run, and a native may answer
    // differently each time. A function skipped by the memo must not replay
    // what it read from the previous run's data.
    let src = "\
enum K
  a(v: random_int(0, 1000000)),
end
fn label(k)
  var acc = 0
  for i in range(0, 40) do
    set acc += i
  end
  \"v={k.v}\"
end
print(label(K.a) == \"v={K.a.v}\")
";
    use petal::policy::RunPolicy;
    let fast = frames(src, RunPolicy::FAST, 4);
    assert_eq!(fast, frames(src, RunPolicy::BASELINE, 4));
    assert!(fast.iter().all(|f| f == &["true"]), "{fast:?}");
}

#[test]
fn a_function_held_by_a_variant_is_called_through_the_field() {
    // As on a record or a class instance: `v.field(args)` calls a function
    // the field holds, whether it is constant data or a payload field.
    let src = "\
enum A
  x(k: fn(v) -> v + 1),
  y(k: fn(v) -> v),
end
enum B
  with(cb),
  none,
end
let b = B.with(fn(v) -> v * 2)
print(A.x.k(1), (A.x.k)(1), A.y.k(1), b.cb(4))
";
    assert_eq!(run(src), ["2 2 1 8"]);
    // A method the variant does not have names the enum, not `type enum`.
    let e = run_err("enum A\n  x,\nend\nA.x.nope()");
    assert!(e.contains("No method 'nope' on enum A"), "{e}");
}

#[test]
fn constant_data_that_reads_other_data_is_evaluated_after_it() {
    // Declaration order does not matter: data that reads another variant's
    // data is evaluated after it, across enums and within one.
    let src = "\
enum M
  m(v: L.x.v + K.a.v),
end
enum L
  x(v: K.a.v + 1),
end
enum K
  a(v: 1),
end
print(M.m.v, L.x.v)
";
    assert_eq!(run(src), ["3 2"]);
    let src = "enum K\n  a(o: K.b.o + 1),\n  b(o: 5),\nend\nprint(K.a.o, K.b.o)";
    assert_eq!(run(src), ["6 5"]);
    // Naming a variant is not reading its data, so two variants may still
    // name each other; reading each other's data is a cycle.
    let e = compile_err("enum K\n  a(o: K.b.o + 1),\n  b(o: K.a.o),\nend");
    assert!(
        e.contains(
            "the constant data of `K.a` and `K.b` read each other, so neither can be evaluated first"
        ),
        "{e}"
    );
    assert!(e.contains("line 2"), "{e}");
    // A read the compiler cannot see (through a function) says what happened
    // rather than claiming the field does not exist.
    let e = run_err(
        "fn peek()\n  K.b.o\nend\nenum K\n  a(o: peek()),\n  b(o: 5),\nend\nprint(K.a.o)",
    );
    assert!(
        e.contains("the constant data of K.b is not evaluated yet"),
        "{e}"
    );
}

#[test]
fn a_let_bound_to_a_variant_is_a_constant_let() {
    let src = "\
let dflt = K.a
let both = [K.a, K.b]
enum O
  o(k: dflt, ks: both),
end
enum K
  a, b
end
print(O.o.k, O.o.ks)
";
    assert_eq!(run(src), ["K.a [K.a, K.b]"]);
}

#[test]
fn a_local_named_like_a_late_function_is_usable_in_constant_data() {
    // `late` is not hoisted (it reads `zz`), but the data never refers to
    // it: the pattern binds its own `late`.
    let src = "\
let zz = len([1])
fn late()
  zz
end
enum J
  x(u: match 3
    when late -> late + 1
  end),
end
print(J.x.u)
";
    assert_eq!(run(src), ["4"]);
}

const PARKS: &str = "\
pub enum Kind
  park, road
end
enum Hidden
  h1, h2
end
class Secret
  n: int
end
pub fn mk()
  Kind.park
end
pub fn hid()
  Hidden.h1
end
";

#[test]
fn an_enum_a_file_cannot_see_is_not_a_type_name_there() {
    // As for a class: private to its module, or public but not imported.
    let src = "\
import shapes: mk, hid
fn f(k: Hidden) -> Hidden
  k
end
fn g(k: Kind) -> Kind
  k
end
fn s(c: Secret)
  c
end
let w: Hidden = hid()
";
    let w = warnings_with(&[("shapes", PARKS)], src);
    for name in ["Hidden", "Kind", "Secret"] {
        assert!(
            w.iter().any(|m| m.contains(&format!("unknown type name `{name}`"))),
            "{name}: {w:?}"
        );
    }
    // Imported by name, or with the module, it is a type.
    for import in ["import shapes: Kind, mk", "import shapes"] {
        let src = format!("{import}\nfn g(k: Kind) -> Kind\n  k\nend\n");
        let w = warnings_with(&[("shapes", PARKS)], &src);
        assert!(w.is_empty(), "{import}: {w:?}");
    }
}

#[test]
fn a_class_may_not_take_the_name_of_an_enum_in_either_order() {
    // In one file the enum is the one reported, wherever it stands.
    let e = compile_err("enum E\n  a,\nend\nclass E\n  x,\nend");
    assert!(e.contains("enum `E` has the same name as class `E`"), "{e}");
    // Across files, whichever is declared second.
    let e = compile_err_with(
        &[("lib", "pub enum Foo\n  a, b\nend\npub fn mk()\n  Foo.a\nend")],
        "import lib\nclass Foo\n  x: int\nend\nlet f = Foo(1)",
    );
    assert!(e.contains("class `Foo` has the same name as enum `Foo`"), "{e}");
}

#[test]
fn an_unresolved_pattern_is_reported_once() {
    // `Hidden` is private to `town`: the pattern does not resolve, and the
    // match is not then held to the variants of an enum this file cannot see.
    let town = "enum Hidden\n  x,\nend\npub fn hidden()\n  Hidden.x\nend";
    let e = compile_err_with(
        &[("town", town)],
        "import town\nmatch town.hidden()\n  when Hidden.y -> print(\"y\")\nend",
    );
    assert!(e.contains("`Hidden` is not an enum in scope"), "{e}");
    assert!(!e.contains("does not cover every variant"), "{e}");
}

#[test]
fn a_lambda_in_constant_data_is_desugared_and_checked_like_any_other() {
    let src = "\
fn bump(n)
  n + 1
end
enum K
  a(f: fn(x)
    let c = x
    bump(@c)
    c
  end),
end
print(K.a.f(1))
";
    assert_eq!(run(src), ["2"]);
    // A discarded pure expression inside it is reported, as in a `let`.
    let w = warnings("enum K\n  a(f: fn(x)\n    x + 1\n    x\n  end),\nend\nprint(K.a.f(1))");
    let l = warnings("let f = fn(x)\n  x + 1\n  x\nend\nprint(f(1))");
    assert!(!l.is_empty(), "the `let` form is expected to warn");
    assert_eq!(w.len(), l.len(), "{w:?} vs {l:?}");
}

#[test]
fn error_texts_name_what_was_written() {
    // A pattern's arity error shows the pattern as call-shaped when it was.
    let e = compile_err(
        "enum Shape\n  rect(w, h),\n  unit,\nend\nmatch Shape.unit\n  when Shape.rect(w) -> 1\n  when _ -> 2\nend",
    );
    assert!(e.contains("the pattern `Shape.rect(…)` has 1 field"), "{e}");
    // A call-shaped bare pattern is pointed at the variant that has a payload.
    let e = compile_err(
        "enum Kind\n  road(lanes),\nend\nenum Use\n  road,\nend\n\
         match Kind.road(2)\n  when road(n) -> n\nend",
    );
    assert!(e.contains("did you mean Kind.road? (also Use.road)"), "{e}");
    // A variant in scope beats the habit hint for `None`.
    let e = run_err("enum A\n  None,\nend\nprint(None)");
    assert!(e.contains("did you mean A.None?"), "{e}");
    // An enum takes no methods, and the message does not suggest a class.
    let e = compile_err("enum Kind\n  park,\nend\nfn Kind.describe(k)\n  1\nend");
    assert!(e.contains("`Kind` is an enum, and methods are declared on classes only"), "{e}");
    assert!(!e.contains("class Kind"), "{e}");
}

// ── The read path: slots, the shared `all` list, cheap declaration ──────
//
// What docs/tasks/static-value-optimization.md (A1) changed underneath the
// rules above: a declared variant is found by a slot in the stack's enum
// table rather than by its tag's text, `Enum.all` is one list per stack, and
// a run that publishes the data the run before published does no comparing.
// None of it is visible to a program; these hold it to that.

use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::policy::RunPolicy;
use petal::value::Value;

/// `host_variant(tag, ...payload)`: a variant value built by the host, with
/// whatever tag it is given, the way `PetalCxt::emit` and the C bridge's
/// `pb_builder_begin_enum` build one.
fn native_host_variant(cx: &mut PetalCxt) -> NativeResult {
    let Value::String(tag) = cx.get_value(1)? else {
        return Err("host_variant: tag".into());
    };
    let tag = cx.heap().get_string(tag).to_string();
    let payload: Vec<Value> = (2..=cx.arg_count()).map(|i| cx.get_value(i)).collect::<Result<_, _>>()?;
    let heap = cx.heap_mut();
    let tag = heap.alloc_string(tag);
    let data = heap.alloc_list(payload);
    cx.push_value(Value::EnumVariant { tag, data });
    Ok(1)
}

fn host_env() -> Env {
    let mut env = env_with(&[]);
    env.register_native("host_variant", native_host_variant, NativeEffects::PURE);
    env
}

/// Run `src` in an env that has `host_variant`, under `policy`.
fn run_host(src: &str, policy: RunPolicy) -> Result<Vec<String>, String> {
    let mut env = host_env();
    env.set_policy(policy);
    let src = format!("native fn host_variant(...args)\n{src}");
    let pid = env.load_program(&src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid)?;
    Ok(env.take_output())
}

#[test]
fn a_variant_a_host_built_with_a_free_form_tag_still_works() {
    // No enum declares `blob` or `draw.rect`: the value has a name, a
    // payload a pattern can take apart, and no fields.
    let src = "\
enum Kind
  park(shape: \"area\"),
  road(shape: \"route\"),
end
let b = host_variant(\"blob\", 1, 2)
let d = host_variant(\"draw.rect\", 3)
print(b, b.name, type(b), d, d.name)
print(b == host_variant(\"blob\", 1, 2), b == host_variant(\"blob\", 1, 3), b == Kind.park)
print(b.shape ?? \"none\", d?.shape, b?.name)
";
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        assert_eq!(
            run_host(src, policy).unwrap(),
            ["blob(1, 2) blob enum draw.rect(3) rect", "true false false", "none nil blob"]
        );
        let e = run_host(&format!("{src}print(b.shape)"), policy).unwrap_err();
        assert!(e.contains("No field 'shape' on enum variant blob"), "{e}");
    }
}

#[test]
fn a_variant_a_host_built_with_a_declared_tag_is_that_variant() {
    // The tag is all a variant is: one the host spells like a declared
    // variant reads that variant's data and payload names, and equals it.
    let src = "\
enum Kind
  park(shape: \"area\"),
  road(shape: \"route\"),
end
enum Op
  move(dx, dy, tone: \"go\"),
end
let k = host_variant(\"Kind.road\")
let op = host_variant(\"Op.move\", 3, 4)
print(k, k.shape, k.name, k == Kind.road, k == Kind.park)
print(op.dx, op.dy, op.tone, op == Op.move(3, 4))
let short = host_variant(\"Op.move\", 3)
print(short.dx, short.dy ?? \"none\", short.tone)
";
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        assert_eq!(
            run_host(src, policy).unwrap(),
            ["Kind.road route road true false", "3 4 go true", "3 none go"]
        );
    }
}

#[test]
fn a_variant_too_long_to_intern_is_found_by_its_text() {
    // A tag past the heap's interning limit has a new id every time it is
    // built, so the table's index never holds it; its text still does.
    let long = "v".repeat(petal::heap::INTERN_MAX_LEN + 40);
    let src = format!(
        "enum K\n  {long}(code: 7),\n  w(code: 8),\nend\n\
         let k = K.{long}\nprint(k.code, K.w.code, k == K.{long}, len(K.all), K.all[0].code)\n\
         fn pick(k)\n  match k\n    when K.{long} -> \"long\"\n    when K.w -> \"w\"\n  end\nend\n\
         print(pick(k), pick(K.w), len(k.name))"
    );
    let expect = ["7 8 true 2 7".to_string(), format!("long w {}", long.len())];
    assert_eq!(run(&src), expect);
    assert_eq!(frames(&src, RunPolicy::BASELINE, 2)[1], expect);
}

#[test]
fn a_variant_kept_in_state_follows_every_edit_of_its_enum() {
    // docs/features/hot-reload.md, "Editing an enum", row by row. The kept value is a
    // tag and a payload; the slot it is found at is the running program's.
    let v1 = "\
enum Kind
  park(shape: \"area\"),
  road(shape: \"route\"),
end
enum Op
  fight(place, won, tone: \"bad\"),
end
state kept = Kind.road
state op = Op.fight(\"bridge\", true)
print(kept, kept.shape, kept.name, op.place, op.tone)
print(Kind.all)
";
    let mut env = env_with(&[]);
    let pid = env.load_program(v1).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let step = |env: &mut Env, src: Option<&str>| -> Result<Vec<String>, String> {
        if let Some(src) = src {
            env.reload_program(sid, src, None).unwrap();
        }
        env.reset_stack(sid).unwrap();
        env.run(sid)?;
        Ok(env.take_output())
    };
    assert_eq!(
        step(&mut env, None).unwrap(),
        ["Kind.road route road bridge bad", "[Kind.park, Kind.road]"]
    );
    // A literal in the data: patched, and the kept variant answers anew.
    let v2 = v1.replace("\"route\"", "\"street\"");
    assert_eq!(
        step(&mut env, Some(&v2)).unwrap(),
        ["Kind.road street road bridge bad", "[Kind.park, Kind.road]"]
    );
    // A variant added in front and the rest reordered: every slot moves, the
    // kept variant is still `road`, and `Kind.all` follows the declaration.
    let v3 = "\
enum Kind
  lake(shape: \"water\", cost: 1),
  road(shape: \"street\", cost: 2),
  park(shape: \"area\", cost: 3),
end
enum Op
  fight(place, won, tone: \"bad\"),
end
state kept = Kind.park
state op = Op.fight(\"x\", false)
print(kept, kept.shape, kept.cost, kept == Kind.road, op.place, op.tone)
print(Kind.all)
";
    assert_eq!(
        step(&mut env, Some(v3)).unwrap(),
        ["Kind.road street 2 true bridge bad", "[Kind.lake, Kind.road, Kind.park]"]
    );
    // Its payload fields renamed and one added: the payload is positional.
    let v4 = v3
        .replace("fight(place, won, tone: \"bad\")", "fight(where, won, how, tone: \"worse\")")
        .replace("Op.fight(\"x\", false)", "Op.fight(\"x\", false, 1)")
        .replace("op.place, op.tone", "op.where, op.how ?? \"absent\", op.tone");
    assert_eq!(
        step(&mut env, Some(&v4)).unwrap(),
        ["Kind.road street 2 true bridge absent worse", "[Kind.lake, Kind.road, Kind.park]"]
    );
    // Its variant removed: it still prints and answers `name`, equals no
    // variant the enum has now, and a data read is the documented error.
    let v5 = "\
enum Kind
  lake(shape: \"water\", cost: 1),
  park(shape: \"area\", cost: 3),
end
state kept = Kind.park
print(kept, kept.name, type(kept), kept == Kind.lake, kept == Kind.park, kept.shape ?? \"gone\")
print(Kind.all)
";
    assert_eq!(
        step(&mut env, Some(v5)).unwrap(),
        ["Kind.road road Kind false false gone", "[Kind.lake, Kind.park]"]
    );
    let v6 = v5.replace("kept.shape ?? \"gone\"", "kept.shape");
    let e = step(&mut env, Some(&v6)).unwrap_err();
    assert!(e.contains("No field 'shape' on enum variant Kind.road"), "{e}");
    // Its enum removed: a variant of an enum that no longer exists.
    let v7 = "state kept = 0\nprint(kept, kept.name, kept?.shape)\n";
    assert_eq!(step(&mut env, Some(v7)).unwrap(), ["Kind.road road nil"]);
    // Putting the variant back makes the same value whole again. (`op` went
    // with its declaration two edits ago, and starts over.)
    assert_eq!(
        step(&mut env, Some(v3)).unwrap(),
        ["Kind.road street 2 true x bad", "[Kind.lake, Kind.road, Kind.park]"]
    );
}

#[test]
fn constant_data_leaves_the_revision_alone_and_dynamic_data_moves_it() {
    let revisions = |src: &str, policy: RunPolicy| -> Vec<u64> {
        let mut env = env_with(&[]);
        env.set_policy(policy);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        (0..4)
            .map(|_| {
                env.reset_stack(sid).unwrap();
                env.run(sid).unwrap();
                env.take_output();
                env.enum_data_revision(sid).unwrap()
            })
            .collect()
    };
    // Constant data, including a list built afresh by every run (equal, not
    // identical) and another variant: published once, then left alone.
    let constant = "\
enum Use
  road, green
end
enum K
  a(v: 1, tags: [\"x\", \"y\"], use: Use.road, at: {x: 1.5}),
  b(v: 2, tags: [], use: Use.green, at: {x: 2.5}),
end
print(K.a.v)
";
    let r = revisions(constant, RunPolicy::FAST);
    assert_eq!(r, [2, 2, 2, 2], "one bump per variant on the first run only");
    // Data a native answers differently each run moves it every run.
    let dynamic = "\
enum K
  a(v: random_int(0, 1000000000)),
  b(v: 2),
end
print(K.a.v)
";
    let r = revisions(dynamic, RunPolicy::FAST);
    assert!(r.windows(2).all(|w| w[1] == w[0] + 1), "{r:?}");
}

#[test]
fn a_memoized_function_that_reads_data_in_a_loop_sees_it_change() {
    // The reads after the first are retired without a dependency note of
    // their own; the one entry must still invalidate the record.
    let src = "\
enum K
  a(v: random_int(0, 1000000), w: 1),
  b(v: random_int(0, 1000000), w: 2),
end
fn total(ks)
  var acc = 0
  for i in range(0, 30) do
    for k in ks do
      set acc += k.v * k.w
    end
  end
  acc
end
fn steady(ks)
  var acc = 0
  for i in range(0, 30) do
    for k in ks do
      set acc += k.w
    end
  end
  acc
end
print(total(K.all) == 30 * (K.a.v + 2 * K.b.v), steady(K.all))
";
    let fast = frames(src, RunPolicy::FAST, 5);
    assert_eq!(fast, frames(src, RunPolicy::BASELINE, 5));
    assert!(fast.iter().all(|f| f == &["true 90"]), "{fast:?}");
}

#[test]
fn a_memoized_function_replays_while_constant_data_stays_put() {
    let src = "\
enum K
  a(v: 3),
  b(v: 4),
end
fn total(ks)
  var acc = 0
  for i in range(0, 40) do
    for k in ks do
      set acc += k.v
    end
  end
  acc
end
print(total(K.all))
";
    let mut env = env_with(&[]);
    // Without the static cache, which would keep `total(K.all)` (a static
    // value) and never make the call the memo is meant to replay.
    env.set_policy(RunPolicy::FAST.with_static_cache(false));
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for _ in 0..4 {
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), ["280"]);
    }
    let stats = env.memo_stats(sid).unwrap();
    assert_eq!((stats.records, stats.hits, stats.misses), (1, 3, 0), "{stats:?}");
}

#[test]
fn all_is_one_list_that_nothing_can_write_through() {
    // Every read of `K.all` hands out the same list, so every way of
    // "changing" one must leave it as declared: value semantics, with the
    // optimizer's in-place rewrites on and off.
    let src = "\
enum K
  a(v: 1),
  b(v: 2),
  c(v: 3),
end
fn grown()
  let xs = K.all
  append(xs, K.a)
end
fn accumulated()
  let xs = K.all
  for i in range(0, 3) do
    xs = append(xs, i)
  end
  xs
end
fn through_a_var()
  var xs = K.all
  set xs[0] = K.c
  set xs = append(xs, 9)
  xs
end
fn sorted_and_reversed()
  [sort_by(K.all, fn(k) -> 0 - k.v), reverse(K.all)]
end
state kept = K.all
kept = append(kept, len(kept))
print(grown())
print(accumulated())
print(through_a_var())
print(sorted_and_reversed())
print(append(K.all, 1), concat(K.all, K.all))
print(len(kept) > 3, kept[0], kept[2])
let E = K
print(E.all, K.all, K.all == [K.a, K.b, K.c], len(K.all))
";
    let expect = [
        "[K.a, K.b, K.c, K.a]",
        "[K.a, K.b, K.c, 0, 1, 2]",
        "[K.c, K.b, K.c, 9]",
        "[[K.c, K.b, K.a], [K.c, K.b, K.a]]",
        "[K.a, K.b, K.c, 1] [K.a, K.b, K.c, K.a, K.b, K.c]",
        "true K.a K.c",
        "[K.a, K.b, K.c] [K.a, K.b, K.c] true 3",
    ];
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        for frame in frames(src, policy, 4) {
            assert_eq!(frame, expect, "{policy:?}");
        }
    }
}

#[test]
fn all_is_built_once_and_survives_collection() {
    let src = "\
enum K
  a, b, c, d,
  wide(n),
end
var n = 0
for i in range(0, 4000) do
  let all = K.all
  let junk = [i, i + 1, i + 2]
  set n = n + len(all)
end
print(n, K.all)
";
    let mut env = env_with(&[]);
    env.set_heap_stats(true);
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let lists = |env: &Env| env.alloc_stats_for(sid).unwrap().get(petal::stats::AllocKind::List);
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["16000 [K.a, K.b, K.c, K.d]"]);
    let first = lists(&env);
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["16000 [K.a, K.b, K.c, K.d]"]);
    // 4,000 `junk` lists a run, and not one more for the 4,000 `K.all`s (the
    // first run also built the list itself and the heap's empty payload).
    assert_eq!(lists(&env) - first, 4000);
    assert!(first <= 4003, "{first}");
}

#[test]
fn two_stacks_and_a_fork_each_read_their_own_table() {
    let src = "\
enum K
  a(v: random_int(0, 1000000)),
  b(v: 2),
end
state n = 0
n = n + 1
print(K.a.v == K.all[0].v, K.b.v, n, K.all)
";
    let mut env = env_with(&[]);
    let pid = env.load_program(src).unwrap();
    let one = env.create_stack(pid).unwrap();
    let two = env.create_stack(pid).unwrap();
    // Two stacks of one context share its heap, and so the interned tags.
    env.run(one).unwrap();
    assert_eq!(env.take_output_for(one), ["true 2 1 [K.a, K.b]"]);
    env.run(two).unwrap();
    assert_eq!(env.take_output_for(two), ["true 2 1 [K.a, K.b]"]);
    // A fork has a heap of its own, copied id for id.
    env.run_speculative(one).unwrap();
    let fork = env.fork_execution(one).unwrap();
    env.reset_stack(fork).unwrap();
    env.run(fork).unwrap();
    assert_eq!(env.take_output_for(fork), ["true 2 2 [K.a, K.b]"]);
    env.reset_stack(one).unwrap();
    env.run(one).unwrap();
    assert_eq!(env.take_output_for(one), ["true 2 2 [K.a, K.b]"]);
    env.reset_stack(two).unwrap();
    env.run(two).unwrap();
    assert_eq!(env.take_output_for(two), ["true 2 2 [K.a, K.b]"]);
}

#[test]
fn switching_the_optimizer_between_runs_keeps_the_table() {
    // The two lowerings of one program number its variants alike, so a stack
    // run under one and then the other keeps its data and its `all` list.
    let src = "\
enum K
  a(v: 1),
  b(v: 2),
end
fn sum()
  var acc = 0
  for k in K.all do
    set acc += k.v
  end
  acc
end
print(sum(), K.b.v, K.all)
";
    let mut env = env_with(&[]);
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE, RunPolicy::FAST] {
        env.set_policy(policy);
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), ["3 2 [K.a, K.b]"]);
        assert_eq!(env.call_function(sid, "sum", &[]).unwrap(), Value::Int(3));
    }
}

// ── A1 review ────────────────────────────────────────────────────────────

#[test]
fn every_way_of_rewriting_a_list_that_began_as_all_leaves_all_alone() {
    // The shared list reaches a rewrite through a wrapper function, an index
    // write on a `let`, a `var`, a record field, a parameter, a nested list
    // and `state` — each of which the optimizer might take for a container
    // the code owns. None may write through it, on any frame.
    let src = "\
enum K
  a(v: 1),
  b(v: 2),
  c(v: 3),
end
fn all()
  K.all
end
fn wrap_append()
  append(all(), 7)
end
fn idx_let()
  let xs = K.all
  xs[0] = K.c
  xs
end
fn idx_loop()
  var xs = all()
  for i in range(0, 3) do
    set xs[i] = i
  end
  xs
end
fn rec()
  let r = { xs: K.all }
  r.xs = append(r.xs, 1)
  r.xs[1] = 5
  r
end
fn param(xs)
  xs[0] = 99
  xs = append(xs, 100)
  xs
end
fn collect()
  var out = K.all
  for k in K.all do
    set out = append(out, k.v)
  end
  out
end
fn nested()
  var ys = [K.all, K.all]
  set ys[0][0] = 1
  ys
end
state kept = K.all
kept[0] = K.b
state grown = all()
grown = append(grown, 1)
grown[1] = 0
print(wrap_append())
print(idx_let())
print(idx_loop())
print(rec())
print(param(K.all), param(all()))
print(collect())
print(nested())
print(kept, grown[0], grown[1], len(grown) > 3)
print(map(K.all, fn(k) -> k.v), filter(K.all, fn(k) -> k.v > 1))
print(K.all, all())
";
    let expect = [
        "[K.a, K.b, K.c, 7]",
        "[K.c, K.b, K.c]",
        "[0, 1, 2]",
        "{ xs: [K.a, 5, K.c, 1] }",
        "[99, K.b, K.c, 100] [99, K.b, K.c, 100]",
        "[K.a, K.b, K.c, 1, 2, 3]",
        "[[1, K.b, K.c], [K.a, K.b, K.c]]",
        "[K.b, K.b, K.c] K.a 0 true",
        "[1, 2, 3] [K.b, K.c]",
        "[K.a, K.b, K.c] [K.a, K.b, K.c]",
    ];
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        for frame in frames(src, policy, 4) {
            assert_eq!(frame, expect, "{policy:?}");
        }
    }
}

/// A rebinding variable that starts as `Enum.all` and is rewritten in a loop
/// (the Fisher-Yates bag of examples/games/tetris, an `append` accumulator)
/// gets a list of its own at the read, so the loop rewrites it in place: one
/// allocation for the whole loop and no copy per iteration, which is what it
/// cost when `Enum.all` allocated on every read. The shared list is not the
/// one rewritten.
#[test]
fn an_accumulator_that_starts_as_all_is_copied_once_and_then_grown_in_place() {
    let src = "\
enum Piece
  i, o, t, s, z, j, l,
end
fn shuffled(seed)
  let bag = Piece.all
  let s = seed
  let i = 6
  while i > 0 do
    s = (s * 1103515245 + 12345) % 2147483648
    let j = s % (i + 1)
    let tmp = bag[i]
    bag[i] = bag[j]
    bag[j] = tmp
    i = i - 1
  end
  bag
end
fn grown()
  let xs = Piece.all
  for i in range(0, 100) do
    xs = append(xs, i)
  end
  xs
end
fn branched(flip)
  let xs = Piece.all
  if flip then
    xs = append(xs, 1)
  end
  xs
end
var n = 0
for round in range(0, 50) do
  set n = n + len(shuffled(round)) + len(grown())
end
print(n, shuffled(3), len(grown()), grown()[7])
print(branched(true), branched(false))
print(Piece.all)
";
    let expect = [
        "5700 [Piece.i, Piece.z, Piece.o, Piece.j, Piece.l, Piece.s, Piece.t] 107 0",
        "[Piece.i, Piece.o, Piece.t, Piece.s, Piece.z, Piece.j, Piece.l, 1] \
         [Piece.i, Piece.o, Piece.t, Piece.s, Piece.z, Piece.j, Piece.l]",
        "[Piece.i, Piece.o, Piece.t, Piece.s, Piece.z, Piece.j, Piece.l]",
    ];
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        for frame in frames(src, policy, 3) {
            assert_eq!(frame, expect, "{policy:?}");
        }
    }

    // With the optimizer on, a run allocates one list per accumulator (its
    // private copy) rather than one per rewrite: 52 `shuffled` + 53 `grown`
    // + 2 `branched`, and the loop's `range` lists if it has any — far under
    // the 600 + 5,300 a copy per rewrite makes.
    let mut env = env_with(&[]);
    env.set_policy(RunPolicy::FAST);
    env.set_heap_stats(true);
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let lists = |env: &Env| env.alloc_stats_for(sid).unwrap().get(petal::stats::AllocKind::List);
    env.run(sid).unwrap();
    let first = lists(&env);
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output().len(), 6);
    let per_run = lists(&env) - first;
    assert!(per_run <= 300, "{per_run} lists allocated in a run");
}
