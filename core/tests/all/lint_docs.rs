//! `petal lint`'s documentation rules (core/src/lint/docs.rs,
//! docs/proposals/doc-annotation.md): `doc-params`, `undocumented-pub`,
//! `deprecated` and `package-description`. All four report only.

use std::path::{Path, PathBuf};

use petal::lint::{
    DEPRECATED, DOC_PARAMS, Finding, LintOptions, PACKAGE_DESCRIPTION, RuleFilter,
    UNDOCUMENTED_PUB, lint_source,
};

fn only(rule: &str) -> LintOptions {
    LintOptions {
        rules: RuleFilter {
            include: Some(vec![rule.to_string()]),
            exclude: Vec::new(),
        },
        ..LintOptions::default()
    }
}

#[track_caller]
fn findings(src: &str, opts: &LintOptions) -> Vec<Finding> {
    let out = lint_source(src, opts).expect("lint_source should succeed");
    assert_eq!(out.output, src, "a doc rule never rewrites");
    out.findings
}

fn messages(src: &str, rule: &str) -> Vec<String> {
    findings(src, &only(rule)).into_iter().map(|f| f.message).collect()
}

/// A directory of files for a test that needs imports or a `petal.toml`.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-lint-docs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn lint_file(file: &Path, rule: &str) -> Vec<Finding> {
    let src = std::fs::read_to_string(file).unwrap();
    let opts = LintOptions {
        origin: Some(file.to_path_buf()),
        ..only(rule)
    };
    findings(&src, &opts)
}

// ── doc-params ─────────────────────────────────────────────────────────────

#[test]
fn a_params_key_no_parameter_has_is_reported_with_a_suggestion() {
    let src = "\
@doc(\"Constrain.\", params: {value: \"v\", low: \"l\", hi: \"h\"})
fn clamp_to(value, lo, hi)
  value
";
    let found = findings(src, &only(DOC_PARAMS));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].message,
        "`@doc` describes `low`, which `clamp_to` does not take; did you mean `lo`?"
    );
    // At the key, not at the `@`.
    assert_eq!((found[0].line, found[0].column), (1, 41));
}

#[test]
fn a_params_key_may_name_a_parameter_of_any_form_of_the_set() {
    let src = "\
@doc(\"A switch.\", params: {r: \"where\", on: \"state\", opts: \"style\"})
fn switch_at(r, on)
  switch_at(r, on, {})

fn switch_at(r, on, opts)
  on
";
    assert!(messages(src, DOC_PARAMS).is_empty());
}

#[test]
fn natives_classes_and_lambdas_are_checked_against_their_own_parameters() {
    let src = "\
@doc(\"Print.\", params: {values: \"what\", sep: \"no such\"})
native fn print_all(...values)

@doc(\"A point.\", params: {x: \"across\", z: \"no such\"})
class Pt
  x,
  y

@doc(\"Twice.\", params: {n: \"the number\", m: \"no such\"})
let twice = fn(n) n * 2
";
    let found = messages(src, DOC_PARAMS);
    assert_eq!(
        found,
        vec![
            "`@doc` describes `sep`, which `print_all` does not take",
            "`@doc` describes `z`, which `Pt` does not take",
            "`@doc` describes `m`, which `twice` does not take",
        ]
    );
}

#[test]
fn see_entries_must_reach_something() {
    let src = "\
@module_doc(\"Helpers.\", see: [\"ui\", \"nowhere/at_all\"])

class Box
  w,
  h

@doc(\"Area.\", see: [\"lerp\", \"sum\", \"Box\", \"Box.w\", \"ui/drag\", \"ui.button\", \"perimeter\",
                    \"ghost\", \"ui.no_such_widget\", \"Box.depth\"])
fn area(b)
  b.w * b.h

fn perimeter(b)
  2 * (b.w + b.h)
";
    let found = messages(src, DOC_PARAMS);
    let unreachable: Vec<&str> = found
        .iter()
        .map(|m| m.split('`').nth(3).unwrap_or(""))
        .collect();
    assert_eq!(
        unreachable,
        vec!["nowhere/at_all", "ghost", "ui.no_such_widget", "Box.depth"],
        "{found:?}"
    );
}

#[test]
fn see_reaches_what_the_imports_bring_in() {
    let dir = project(
        "see",
        &[
            (
                "main.ptl",
                "import ./lib/h\n\n@doc(\"Main.\", see: [\"helper\", \"h.helper\", \"h\", \"h.missing\"])\n\
                 fn main_fn()\n  helper()\n",
            ),
            ("lib/h.ptl", "pub fn helper()\n  1\n"),
        ],
    );
    let main = dir.join("main.ptl");
    let found: Vec<String> = lint_file(&main, DOC_PARAMS).into_iter().map(|f| f.message).collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`h.missing`"), "{found:?}");
}

#[test]
fn doc_params_can_be_silenced() {
    let src = "\
# petal-lint-ignore doc-params -- kept for the old name
@doc(\"Constrain.\", params: {low: \"l\"})
fn clamp_to(value, lo, hi)
  value
";
    assert!(messages(src, DOC_PARAMS).is_empty());
}

// ── undocumented-pub ───────────────────────────────────────────────────────

#[test]
fn undocumented_pub_is_opt_in() {
    let src = "pub fn bare(x)\n  x\n";
    let out = lint_source(src, &LintOptions::default()).unwrap();
    assert_eq!(out.count(UNDOCUMENTED_PUB), 0);
    let enabled = LintOptions {
        rules: RuleFilter::default().with_enabled(&[UNDOCUMENTED_PUB.to_string()]),
        ..LintOptions::default()
    };
    let out = lint_source(src, &enabled).unwrap();
    assert_eq!(out.count(UNDOCUMENTED_PUB), 1);
}

#[test]
fn each_undocumented_pub_set_is_reported_once_and_private_items_never() {
    let src = "\
@doc(\"Both forms.\")
pub fn documented(a)
  a

pub fn documented(a, b)
  a

pub fn twice(a)
  a

pub fn twice(a, b)
  a

fn private_one()
  1

pub let LIMIT = 10
pub native fn host_thing(x)
";
    let found = findings(src, &only(UNDOCUMENTED_PUB));
    let rows: Vec<(usize, &str)> = found.iter().map(|f| (f.line, f.message.as_str())).collect();
    assert_eq!(
        rows,
        vec![
            (8, "`pub fn twice` has no `@doc`"),
            (17, "`pub let LIMIT` has no `@doc`"),
            (18, "`pub native fn host_thing` has no `@doc`"),
        ]
    );
}

#[test]
fn a_package_root_module_needs_a_module_doc() {
    let dir = project(
        "root",
        &[
            ("petal.toml", "[package]\nname = \"kit\"\n"),
            ("src/kit.ptl", "@doc(\"Fresh.\")\npub fn fresh(x)\n  x\n"),
            ("src/other.ptl", "@doc(\"Other.\")\npub fn other(x)\n  x\n"),
        ],
    );
    let root = lint_file(&dir.join("src/kit.ptl"), UNDOCUMENTED_PUB);
    assert_eq!(root.len(), 1, "{root:?}");
    assert_eq!((root[0].line, root[0].column), (1, 1));
    assert!(root[0].message.contains("root module of package `kit`"), "{root:?}");
    // A submodule's own doc is optional.
    assert!(lint_file(&dir.join("src/other.ptl"), UNDOCUMENTED_PUB).is_empty());
    std::fs::write(
        dir.join("src/kit.ptl"),
        "@module_doc(\"The kit.\")\n\n@doc(\"Fresh.\")\npub fn fresh(x)\n  x\n",
    )
    .unwrap();
    assert!(lint_file(&dir.join("src/kit.ptl"), UNDOCUMENTED_PUB).is_empty());
}

// ── deprecated ─────────────────────────────────────────────────────────────

#[test]
fn a_use_of_a_deprecated_item_in_the_same_file_warns_with_the_replacement() {
    let src = "\
@doc(\"Old.\", deprecated: \"use `clamp_to` instead\")
fn old_clamp(v)
  inner(v)

fn inner(v)
  v

print(old_clamp(3))
let f = old_clamp
fn shadowing(old_clamp)
  old_clamp(1)
";
    let found = findings(src, &only(DEPRECATED));
    let rows: Vec<(usize, usize, &str)> =
        found.iter().map(|f| (f.line, f.column, f.message.as_str())).collect();
    assert_eq!(
        rows,
        vec![
            (8, 7, "`old_clamp` is deprecated: use `clamp_to` instead"),
            (9, 9, "`old_clamp` is deprecated: use `clamp_to` instead"),
        ]
    );
}

#[test]
fn deprecated_code_calling_deprecated_code_is_not_reported() {
    let src = "\
@doc(\"Old.\", deprecated: \"use `b`\")
fn a()
  c()

@doc(\"Older.\", deprecated: \"use `b`\")
fn c()
  1

fn b()
  2
";
    assert!(messages(src, DEPRECATED).is_empty());
}

#[test]
fn deprecation_follows_imports_qualifiers_and_re_exports() {
    let dir = project(
        "deprecated",
        &[
            ("petal.toml", "[package]\nname = \"kit\"\n"),
            ("src/kit.ptl", "@module_doc(\"The kit.\")\n\npub import ./old\n\n@doc(\"New.\")\npub fn fresh(x)\n  x\n"),
            (
                "src/old.ptl",
                "@doc(\"Old.\", deprecated: \"use `kit.fresh`\")\npub fn stale(x)\n  x\n\n\
                 @doc(\"One.\", deprecated: \"pass two arguments\")\npub fn pair(a)\n  a\n\n\
                 @doc(\"Two.\")\npub fn pair(a, b)\n  a\n",
            ),
            (
                "main.ptl",
                "import kit\nimport kit/old as o\n\nprint(stale(1))\nprint(kit.stale(2))\n\
                 print(o.pair(1))\nprint(o.pair(1, 2))\nprint(fresh(3))\n",
            ),
        ],
    );
    let found = lint_file(&dir.join("main.ptl"), DEPRECATED);
    let rows: Vec<(usize, &str)> = found.iter().map(|f| (f.line, f.message.as_str())).collect();
    assert_eq!(
        rows,
        vec![
            (4, "`stale` is deprecated: use `kit.fresh`"),
            (5, "`kit.stale` is deprecated: use `kit.fresh`"),
            (6, "`o.pair` is deprecated: pass two arguments"),
        ]
    );
}

#[test]
fn a_local_declaration_hides_an_imported_deprecated_one() {
    let dir = project(
        "hide",
        &[
            ("lib.ptl", "@doc(\"Old.\", deprecated: \"gone\")\npub fn stale()\n  1\n"),
            ("main.ptl", "import ./lib\n\nfn stale()\n  2\n\nprint(stale())\n"),
        ],
    );
    assert!(lint_file(&dir.join("main.ptl"), DEPRECATED).is_empty());
}

// ── package-description ────────────────────────────────────────────────────

#[test]
fn a_description_that_disagrees_with_the_module_doc_is_reported() {
    let dir = project(
        "description",
        &[
            ("petal.toml", "[package]\nname = \"kit\"\ndescription = \"A small kit of helpers.\"\n"),
            ("src/kit.ptl", "@module_doc(\"\"\"\nA kit of small helpers.\n\nMore here.\n\"\"\")\n\npub fn f()\n  1\n"),
        ],
    );
    let found = lint_file(&dir.join("src/kit.ptl"), PACKAGE_DESCRIPTION);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].message,
        "petal.toml's description \"A small kit of helpers.\" disagrees with the first \
         sentence of the `@module_doc`, \"A kit of small helpers.\": make them say the same"
    );

    // Case, spacing, code marks and the full stop do not count.
    std::fs::write(
        dir.join("petal.toml"),
        "[package]\nname = \"kit\"\ndescription = \"a KIT of small `helpers`\"\n",
    )
    .unwrap();
    assert!(lint_file(&dir.join("src/kit.ptl"), PACKAGE_DESCRIPTION).is_empty());
}

#[test]
fn the_manifest_reads_description_and_lint_enable() {
    let m = petal::package::Manifest::parse(
        "[package]\nname = \"kit\"\ndescription = \"Helpers.\"\n\n[lint]\nenable = [\"undocumented-pub\"]\n",
        Path::new("petal.toml"),
    )
    .unwrap();
    assert_eq!(m.description.as_deref(), Some("Helpers."));
    assert_eq!(m.lint_enable, vec!["undocumented-pub".to_string()]);
}
