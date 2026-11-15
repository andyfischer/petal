//! `@doc(…)` and `@module_doc(…)` (docs/proposals/doc-annotation.md, step 1):
//! where they parse, what lands in the AST, every diagnostic, and the
//! invariant that a compile-time annotation never reaches the IR.

use std::path::{Path, PathBuf};

use petal::ast::{Doc, DocValue, Stmt, StmtKind};
use petal::source_map::FileId;

fn parse(src: &str) -> (Vec<Stmt>, Option<Doc>) {
    let (_, stmts, module) = petal::cst::parse_source_with_module_doc(src, FileId(0))
        .unwrap_or_else(|e| panic!("{src:?}: {e}"));
    (stmts, module)
}

fn parse_error(src: &str) -> String {
    match petal::cst::parse_source(src, FileId(0)) {
        Ok(_) => panic!("{src:?} parsed"),
        Err(e) => e,
    }
}

fn doc_of(s: &Stmt) -> &Doc {
    s.doc.as_deref().unwrap_or_else(|| panic!("no doc on {:?}", s.kind))
}

fn body(s: &Stmt) -> &str {
    doc_of(s).body.as_deref().unwrap_or("")
}

#[test]
fn doc_goes_on_every_declaration() {
    let src = r#"@doc("A function.")
fn f(x)
  x
end
@doc("Public.") pub fn g()
  1
end
class P
  x: int
end
@doc("A method.")
fn P.m(self)
  self.x
end
@doc("A native.")
native fn host_thing(a: int) -> int = "host_thing"
@doc("A class.")
pub class Q
  y: int
end
@doc("An enum.")
enum E
  a,
  b
end
@doc("A let.") let L = 1
@doc("A var.") var V = 2
@doc("A state.") state S = 3
@doc("A memo.") memo(1) M = 4
@doc("A pub let.") pub let PL = 5
"#;
    let (stmts, module) = parse(src);
    assert!(module.is_none());
    let docs: Vec<(&str, &str)> = stmts
        .iter()
        .filter(|s| s.doc.is_some())
        .map(|s| {
            let kind = match &s.kind {
                StmtKind::FnDecl { .. } => "fn",
                StmtKind::NativeFn(_) => "native",
                StmtKind::ClassDecl { .. } => "class",
                StmtKind::EnumDecl { .. } => "enum",
                StmtKind::Let { .. } => "let",
                StmtKind::State { .. } => "state",
                StmtKind::Memo { .. } => "memo",
                _ => "other",
            };
            (kind, body(s))
        })
        .collect();
    assert_eq!(
        docs,
        [
            ("fn", "A function."),
            ("fn", "Public."),
            ("fn", "A method."),
            ("native", "A native."),
            ("class", "A class."),
            ("enum", "An enum."),
            ("let", "A let."),
            ("let", "A var."),
            ("state", "A state."),
            ("memo", "A memo."),
            ("let", "A pub let."),
        ]
    );
    // `pub` stays a modifier of the declaration.
    assert!(stmts[1].exported);
    // A nested `fn` takes one too.
    let (stmts, _) = parse("fn outer()\n  @doc(\"inner\") fn inner()\n    1\n  end\n  inner()\nend\n");
    let StmtKind::FnDecl { body: inner, .. } = &stmts[0].kind else { panic!() };
    assert_eq!(self::body(&inner[0]), "inner");
}

#[test]
fn every_field_is_read() {
    let src = r#"@doc("""
  Constrain a value. A {x, y} record is fine in a raw string.
""",
  params: {value: "The number.", lo: "Lower."},
  returns: "The value, clamped.",
  options: {bg: "Fill."},
  example: ["clamp(1, 0, 2)", """
    clamp(9, 0, 5)   # 5
  """],
  see: ["lerp", "ui/drag"],
  deprecated: "Use `clamp`.",
  hidden: true,
  extra: {since: "0.4", n: -3, x: 1.5, ok: false, none: nil, tags: ["a"], nested: {k: 1}})
fn clamp2(value, lo, hi)
  value
end
"#;
    let (stmts, _) = parse(src);
    let d = doc_of(&stmts[0]);
    assert_eq!(d.cleaned_body().unwrap(), "Constrain a value. A {x, y} record is fine in a raw string.");
    assert_eq!(d.params, [("value".into(), "The number.".into()), ("lo".into(), "Lower.".into())]);
    assert_eq!(d.returns.as_deref(), Some("The value, clamped."));
    assert_eq!(d.options, [("bg".into(), "Fill.".into())]);
    assert_eq!(d.cleaned_examples(), ["clamp(1, 0, 2)", "clamp(9, 0, 5)   # 5"]);
    assert_eq!(d.see, ["lerp", "ui/drag"]);
    assert_eq!(d.deprecated.as_deref(), Some("Use `clamp`."));
    assert!(d.hidden);
    assert_eq!(
        d.extra,
        [
            ("since".into(), DocValue::String("0.4".into())),
            ("n".into(), DocValue::Int(-3)),
            ("x".into(), DocValue::Float(1.5)),
            ("ok".into(), DocValue::Bool(false)),
            ("none".into(), DocValue::Nil),
            ("tags".into(), DocValue::List(vec![DocValue::String("a".into())])),
            ("nested".into(), DocValue::Record(vec![("k".into(), DocValue::Int(1))])),
        ]
    );
    // The doc's span runs from its `@` to its `)`; the statement's starts at
    // the `@` too.
    assert_eq!((d.span.start.line, d.span.start.column), (1, 1));
    assert_eq!(d.span.end.line, 13);
    assert_eq!(stmts[0].span.start.offset, 0);
    // A single `example:` string and fields with no body.
    let (stmts, _) = parse("@doc(example: \"f()\", hidden: false)\nfn f()\n  1\nend\n");
    let d = doc_of(&stmts[0]);
    assert_eq!((d.body.as_deref(), d.example.as_slice(), d.hidden), (None, &["f()".to_string()][..], false));
    // JSON leaves out what is absent, and keeps maps in source order.
    let json = serde_json::to_value(&stmts[0]).unwrap();
    assert_eq!(json["doc"], serde_json::json!({"example": ["f()"]}));
}

#[test]
fn doc_is_not_reserved() {
    // `doc` stays an ordinary name: a binding, a function, a parameter, a
    // record field.
    let src = "let doc = 1\nfn doc2(doc)\n  doc + 1\nend\nfn docf(x)\n  x * 2\nend\nlet r = {doc: doc2(doc)}\nprint(r.doc, docf(doc))\n";
    let (stmts, _) = parse(src);
    assert!(stmts.iter().all(|s| s.doc.is_none()));
    let mut env = petal::env::Env::new();
    env.load_program(src).expect("compiles");
    let (stmts, _) = parse("fn doc(x)\n  x\nend\nprint(doc(3))\n");
    assert!(stmts[0].doc.is_none());
}

/// The AST of `src` as JSON, without spans or docs.
fn ast_shape(src: &str) -> String {
    let (stmts, _) = parse(src);
    let mut v = serde_json::to_value(&stmts).unwrap();
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.remove("span");
                m.remove("doc");
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut v);
    v.to_string()
}

#[test]
fn doc_stacks_with_value_annotations_in_any_order() {
    let want = ast_shape("let SPEED = tunable(4.0, 0, 10)\n");
    for src in [
        "@doc(\"Speed.\") @tunable(0, 10) let SPEED = 4.0\n",
        "@tunable(0, 10) @doc(\"Speed.\") let SPEED = 4.0\n",
        "@tunable(0, 10)\n@doc(\"Speed.\")\nlet SPEED = 4.0\n",
    ] {
        assert_eq!(ast_shape(src), want, "{src:?}");
        assert_eq!(body(&parse(src).0[0]), "Speed.");
    }
    // Stacked value annotations still apply bottom-up around a doc.
    assert_eq!(
        ast_shape("@rigid @doc(\"x\") @tunable let X = 1\n"),
        ast_shape("let X = rigid(tunable(1))\n")
    );
}

#[test]
fn module_doc_documents_the_file() {
    let src = "# Licence header.\n\n@module_doc(\"\"\"\n  Arithmetic.\n\"\"\", see: [\"core\"], hidden: true, extra: {k: 1})\n\nimport ./x\nfn f()\n  1\nend\n";
    let (stmts, module) = parse(src);
    let module = module.expect("a module doc");
    assert_eq!(module.cleaned_body().as_deref(), Some("Arithmetic."));
    assert_eq!(module.see, ["core"]);
    assert!(module.hidden);
    // It is not one of the file's statements.
    assert!(matches!(stmts[0].kind, StmtKind::Import(_)));
    assert_eq!(stmts.len(), 2);
    // Directly followed by a declaration, it still documents the file.
    let (stmts, module) = parse("@module_doc(\"M.\")\nfn f()\n  1\nend\n");
    assert_eq!(module.unwrap().body.as_deref(), Some("M."));
    assert!(stmts[0].doc.is_none());
    // A file may be only its doc.
    assert!(parse("@module_doc(\"Nothing else.\")\n").1.is_some());
}

#[test]
fn misused_docs_are_parse_errors() {
    let err = |src: &str, want: &str| {
        let got = parse_error(src);
        assert!(got.contains(want), "{src:?}: wanted {want:?} in {got:?}");
    };
    // Placement and count.
    err("@doc(\"a\") @doc(\"b\") fn f()\n  1\nend\n", "a declaration takes one `@doc`, and this one already has one");
    err("@module_doc(\"a\")\n@module_doc(\"b\")\n", "a file takes one `@module_doc`");
    err("import ./x\n@module_doc(\"a\")\n", "`@module_doc` goes at the top of the file, before imports");
    err("let x = 1\n@module_doc(\"a\")\n", "`@module_doc` goes at the top of the file, before imports [line 2, column 1]");
    err("fn f()\n  @module_doc(\"a\")\n  1\nend\n", "`@module_doc` goes at the top of the file");
    err("@doc(\"a\") @module_doc(\"b\")\nlet x = 1\n", "`@module_doc` annotates the file, not a declaration");
    err("@module_doc(\"a\") let x = 1\n", "a statement of its own");
    err("@doc(\"a\")\n", "an annotation must be followed by a declaration");
    err("@doc(\"a\") print(1)\n", "an annotation must be followed by a declaration");
    err("@doc(\"a\") test \"t\" do\n  1\nend\n", "an annotation must be followed by a declaration");
    err("pub @doc(\"a\") fn f()\n  1\nend\n", "annotations go before `pub`");
    err("let y = @doc(\"a\") 2\n", "`@doc` is not an expression");
    // Value annotations on a function, class or enum.
    err("@doc(\"a\") @tunable fn f()\n  1\nend\n", "`@tunable` annotates a `let`; a function takes only `@doc`.");
    err("@const native fn f() = \"g\"\n", "`@const` annotates a `let`; a function takes only `@doc`.");
    err("@memoize fn f()\n  1\nend\n", "`@memoize` is not an annotation; a function takes only `@doc`.");
    err("@doc(\"a\") @rigid class P\n  x: int\nend\n", "`@rigid` annotates a `let`; a class declaration takes only `@doc`.");
    // Literal-only arguments.
    err(
        "@doc(\"Draws {bg, fg}.\") fn f()\n  1\nend\n",
        "`@doc` takes text written out; `{bg, fg}` in a \"…\" string is an interpolation: write `\\{` for a literal brace, or use a raw string: \"\"\"…\"\"\" [line 1, column 6]",
    );
    err("@doc(returns: \"the {x}\") fn f(x)\n  x\nend\n", "`{x}` in a \"…\" string is an interpolation");
    err("@module_doc(\"a {b}\")\n", "`@module_doc` takes text written out; `{b}`");
    err("let NAME = \"x\"\n@doc(NAME) fn f()\n  1\nend\n", "`@doc` takes only literals (a string, number, bool or nil, or a list or record of them), since tools read it without running the program; `NAME` is a name [line 2, column 6]");
    err("@doc(\"a\" ++ \"b\") fn f()\n  1\nend\n", "this is computed");
    err("@doc(see: [lower(\"X\")]) fn f()\n  1\nend\n", "this is a call");
    err("@doc(extra: {...base}) fn f()\n  1\nend\n", "no `...` spread");
    err("@doc(1) fn f()\n  1\nend\n", "`@doc`: its body is a string: write it in quotes, or as a raw string");
    err("@doc(\"a\", \"b\") fn f()\n  1\nend\n", "`@doc` takes one positional argument, the body");
    // Fields.
    err("@doc(\"a\", param: {x: \"X\"}) fn f(x)\n  x\nend\n", "`@doc` has no field `param:`; did you mean `params:`?");
    err("@doc(\"a\", exmaple: \"f()\") fn f()\n  1\nend\n", "did you mean `example:`?");
    err("@doc(\"a\", category: \"Physics\") fn f()\n  1\nend\n", "`@doc` has no field `category:`; put tool-specific fields in `extra: {category: …}`");
    err("@doc(\"a\", returns: \"x\", returns: \"y\") fn f()\n  1\nend\n", "`@doc` gives `returns:` twice");
    err("@doc(params: {x: \"a\", x: \"b\"}) fn f(x)\n  x\nend\n", "`params:` names `x` twice");
    err("@module_doc(\"a\", params: {x: \"X\"})\n", "`@module_doc` takes no `params:`: it documents a file");
    err("@module_doc(\"a\", example: \"x\")\n", "`@module_doc` takes no `example:`");
    err("@module_doc(\"a\", category: \"x\")\n", "put tool-specific fields in `extra: {category: …}`");
    // Field shapes.
    err("@doc(params: [\"x\"]) fn f(x)\n  x\nend\n", "`params:` is a record of strings");
    err("@doc(params: {x: 1}) fn f(x)\n  x\nend\n", "`x` is a string");
    err("@doc(see: \"lerp\") fn f()\n  1\nend\n", "`see:` is a list of names");
    err("@doc(hidden: \"yes\") fn f()\n  1\nend\n", "`hidden:` is `true` or `false`");
    err("@doc(extra: [1]) fn f()\n  1\nend\n", "`extra:` is a record of fields");
    err("@doc(example: 3) fn f()\n  1\nend\n", "`example:` is a string, or a list of strings");
}

#[test]
fn interpolation_escapes_and_raw_strings_are_text() {
    let (stmts, _) = parse("@doc(\"A \\{x, y} record.\") fn f()\n  1\nend\n");
    assert_eq!(body(&stmts[0]), "A {x, y} record.");
    let (stmts, _) = parse("@doc(\"\"\"Quotes \" and \\ stay; {x} too.\"\"\") fn f()\n  1\nend\n");
    assert_eq!(body(&stmts[0]), "Quotes \" and \\ stay; {x} too.");
}

/// A doc says nothing about where the function is: `fn_info` reports the
/// `fn` keyword's line, past its `@doc`.
#[test]
fn fn_info_reports_the_fn_keyword_past_the_doc() {
    let src = "@doc(\"\"\"\nTwice.\n\"\"\")\nfn twice(x)\n  x * 2\nend\nprint(fn_info(twice).line, fn_info(twice).column)\n";
    let mut env = petal::env::Env::new();
    env.set_echo(false);
    env.run_source(src).expect("runs");
    assert_eq!(env.take_output(), ["4 1"]);
}

// ── The IR invariant ─────────────────────────────────────────────────────

fn ptl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !p.ends_with("node_modules") && !p.ends_with("target") {
                ptl_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "ptl") {
            out.push(p);
        }
    }
}

/// `src` with a `@doc` written before every top-level declaration, on its
/// line, and a `@module_doc` on a line of its own above everything. `None`
/// when `src` does not parse or has a `@module_doc` already.
fn documented(src: &str) -> Option<String> {
    let (_, stmts, module) = petal::cst::parse_source_with_module_doc(src, FileId(0)).ok()?;
    if module.is_some() {
        return None;
    }
    let chars: Vec<char> = src.chars().collect();
    let mut at: Vec<usize> = stmts
        .iter()
        .filter(|s| {
            s.test.is_none()
                && s.doc.is_none()
                && matches!(
                    s.kind,
                    StmtKind::FnDecl { .. }
                        | StmtKind::NativeFn(_)
                        | StmtKind::ClassDecl { .. }
                        | StmtKind::EnumDecl { .. }
                        | StmtKind::Let { .. }
                        | StmtKind::State { .. }
                        | StmtKind::Memo { .. }
                )
        })
        .map(|s| s.span.start.offset as usize)
        .collect();
    at.sort();
    at.dedup();
    let mut out = String::from("@module_doc(\"\"\"\nA file, with {braces} and \"quotes\".\n\"\"\", see: [\"x\"])\n");
    let mut from = 0;
    for a in at {
        out.extend(&chars[from..a]);
        out.push_str("@doc(\"A \\{x} doc.\", params: {a: \"\"\"A.\"\"\"}, extra: {n: 1}) ");
        from = a;
    }
    out.extend(&chars[from..]);
    Some(out)
}

#[test]
fn docs_never_reach_the_ir() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    ptl_files(&root.join("examples"), &mut files);
    ptl_files(&root.join("core/tests"), &mut files);
    files.sort();
    let (mut compared, mut decls) = (0, 0);
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let Some(with_docs) = documented(&src) else { continue };
        let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        let a = petal::ir_equiv::compile_for_compare(&src, &[], Some(path));
        let b = petal::ir_equiv::compile_for_compare(&with_docs, &[], Some(path));
        match (a, b) {
            (Ok((env_a, pa)), Ok((env_b, pb))) => {
                let (pa, pb) = (env_a.get_program(pa).unwrap(), env_b.get_program(pb).unwrap());
                if let Err(diff) = petal::ir_equiv::ir_equivalent(pa, pb) {
                    panic!("{shown}: adding docs changed the IR: {}", diff.to_json());
                }
                compared += 1;
                decls += with_docs.matches("@doc(").count();
            }
            (Err(_), Err(_)) => {}
            (Ok(_), Err(e)) => panic!("{shown}: compiles without docs but not with them: {e}"),
            (Err(e), Ok(_)) => panic!("{shown}: compiles only with docs ({e})"),
        }
    }
    eprintln!("doc IR invariant: {compared} programs, {decls} docs");
    assert!(compared >= 100, "only {compared} programs compared");
}

// ── fmt ──────────────────────────────────────────────────────────────────

#[test]
fn fmt_round_trips_docs() {
    let src = "@module_doc(\"\"\"\n  The file.\n     Kept   as written.\n\"\"\")\n\n@doc(\"\"\"\nRaw   text,  {x}  kept.\n\"\"\",\n  params: {a: \"A.\"},\n  example: \"\"\"\n    f(1)   # 1\n  \"\"\")\npub fn f(a)\n  a\nend\n\n@doc(\"Speed.\") @tunable(0, 10) let S = 4.0\n";
    let out = petal::fmt::format_source(src).expect("formats");
    assert_eq!(out, src, "a formatted file is left as it is");
    // Messy spacing is fixed outside the strings, never inside them.
    let messy = "@doc( \"a  b\" ,params:{x:\"\"\"  y  \"\"\"} )\nfn f(x)\n  x\nend\n";
    let out = petal::fmt::format_source(messy).expect("formats");
    assert_eq!(out, "@doc(\"a  b\", params: {x: \"\"\"  y  \"\"\"})\nfn f(x)\n  x\nend\n");
    // A `@module_doc` directly followed by a statement gets a blank line.
    let out = petal::fmt::format_source("@module_doc(\"M.\")\nfn f()\n  1\nend\n").expect("formats");
    assert_eq!(out, "@module_doc(\"M.\")\n\nfn f()\n  1\nend\n");
    let out = petal::fmt::format_source("# header\n@module_doc(\"M.\")\nimport ./x\n").expect("formats");
    assert_eq!(out, "# header\n@module_doc(\"M.\")\n\nimport ./x\n");
}
