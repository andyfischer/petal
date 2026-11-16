//! `petal doc` (docs/proposals/doc-annotation.md, step 2): the JSON report
//! of the fixture package against its snapshot and against
//! docs/schemas/petal-doc.schema.json, `--brief`, `--private`, the ways a
//! target is named, and every bundled package.
//!
//! The snapshot is `tests/fixtures/doc_pkg.json`. After an intended change,
//! rewrite it with `PETAL_UPDATE_SNAPSHOTS=1 cargo test --test doc_cli`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Run `petal <args>` in `dir`: (stdout, stderr, exit code).
fn petal_in(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(PETAL)
        .args(args)
        .current_dir(dir)
        .env_remove("PETAL_PATH")
        .env("PETAL_OFFLINE", "1")
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn doc_json(dir: &Path, args: &[&str]) -> Value {
    let mut all = vec!["doc", "--json"];
    all.extend_from_slice(args);
    let (out, err, code) = petal_in(dir, &all);
    assert_eq!(code, 0, "petal {all:?} failed: {err}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"))
}

fn schema() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/schemas/petal-doc.schema.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_fixture_package_matches_its_snapshot() {
    let report = doc_json(&fixtures(), &["doc_pkg"]);
    let text = serde_json::to_string_pretty(&report).unwrap() + "\n";
    let snapshot = fixtures().join("doc_pkg.json");
    if std::env::var_os("PETAL_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&snapshot, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&snapshot).unwrap_or_default();
    assert!(
        text == expected,
        "`petal doc --json doc_pkg` differs from tests/fixtures/doc_pkg.json; if the change \
         is intended, rerun with PETAL_UPDATE_SNAPSHOTS=1.\n--- got:\n{text}"
    );
    validate(&report, &schema(), &schema(), "$");
}

#[test]
fn the_report_carries_docs_signatures_and_overloads() {
    let report = doc_json(&fixtures(), &["doc_pkg"]);
    let package = &report["package"];
    assert_eq!(package["description"], "Shapes and sums, documented.");
    assert_eq!(package["doc"]["summary"], "Shapes and sums, documented.");
    assert_eq!(package["doc"]["extra"]["meta"]["ratio"], 0.5);
    let root = &report["modules"][0];
    assert_eq!(root["reexports"][0]["canonical"], "doc_pkg/shapes");
    let item = |module: &Value, name: &str| -> Value {
        module["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    };
    // A comment is not a doc.
    assert_eq!(item(root, "total")["doc"], Value::Null);
    assert_eq!(item(root, "total")["line"], 14);
    // Private items are left out without --private; hidden ones are flagged.
    assert_eq!(item(root, "private_helper"), Value::Null);
    assert_eq!(item(root, "_plumbing")["hidden"], true);
    let scale = item(root, "scale");
    assert_eq!(scale["forms"][0]["params"][1]["default"], "2");
    assert_eq!(scale["forms"][0]["params"][1]["optional"], true);
    // `@tunable` lowers the value; the report shows it as written.
    assert_eq!(item(root, "PHI")["value"], "1.618");

    let shapes = &report["modules"][1];
    // One @doc describes the whole overload set.
    let outline = item(shapes, "outline");
    assert_eq!(outline["forms"].as_array().unwrap().len(), 2);
    assert_eq!(outline["doc"]["summary"], "Draw a box.");
    assert_eq!(outline["forms"][1]["doc"], Value::Null);
    assert_eq!(
        outline["doc"]["example"][1],
        "outline(Box(0, 0, 4, 4), {fill: :f00})"
    );
    // Several: each form keeps its own, and the first is the set's.
    let grow = item(shapes, "grow");
    assert_eq!(grow["doc"]["summary"], "Grow by one amount.");
    assert_eq!(grow["forms"][1]["doc"]["summary"], "Grow by two amounts.");
    // The raw string's indentation is cleaned.
    assert_eq!(
        item(shapes, "Box")["doc"]["body"],
        "An axis-aligned box.\nFields are in constructor order."
    );
    assert_eq!(item(shapes, "Box.area")["receiver"], "Box");
    let measure = item(shapes, "measure");
    assert_eq!(measure["kind"], "native_fn");
    assert_eq!(measure["forms"][0]["host"], "host_measure");
    assert_eq!(measure["forms"][0]["params"][2]["rest"], true);
    assert_eq!(
        item(shapes, "Shape")["doc"]["extra"]["editor_category"],
        "Shapes"
    );
}

#[test]
fn brief_is_one_line_per_item_without_hidden_ones() {
    let (out, err, code) = petal_in(&fixtures(), &["doc", "--brief", "doc_pkg"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("package doc_pkg 0.3.0 — Shapes and sums, documented.\n"),
        "{out}"
    );
    assert!(
        out.contains("\n  fn scale(xs, k = 2) — Scale every element by `k`.\n"),
        "{out}"
    );
    assert!(
        out.contains(
            "\n  fn outline(b: Box) -> nil | (b: Box, style: record) -> nil — Draw a box.\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("\n  fn sum_all(xs) — An old name. [deprecated]\n"),
        "{out}"
    );
    assert!(
        out.contains("\n  native fn measure(text: string, size?: num, ...rest) -> float — Measure"),
        "{out}"
    );
    assert!(!out.contains("_plumbing"), "{out}");
    assert!(!out.contains("private_helper"), "{out}");
    let (private, _, _) = petal_in(&fixtures(), &["doc", "--brief", "--private", "doc_pkg"]);
    assert!(private.contains("\n  fn private_helper(a)\n"), "{private}");
}

#[test]
fn a_target_is_a_file_a_directory_a_package_or_a_module() {
    let dir = fixtures();
    // A file of a package is that module, with the package's facts.
    let file = doc_json(&dir, &["doc_pkg/src/shapes.ptl"]);
    assert_eq!(file["package"]["name"], "doc_pkg");
    assert_eq!(file["modules"].as_array().unwrap().len(), 1);
    assert_eq!(file["modules"][0]["name"], "doc_pkg/shapes");
    assert_eq!(file["modules"][0]["file"], "src/shapes.ptl");
    // A package by name (the project's own, found from inside it), and one
    // module of it.
    let inside = dir.join("doc_pkg");
    let by_name = doc_json(&inside, &["doc_pkg"]);
    assert_eq!(by_name["modules"].as_array().unwrap().len(), 2);
    let module = doc_json(&inside, &["doc_pkg/shapes"]);
    assert_eq!(module["modules"][0]["name"], "doc_pkg/shapes");
    // A loose file.
    let tmp = std::env::temp_dir().join(format!("petal-doc-loose-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("util.ptl"),
        "@doc(\"Twice.\")\npub fn twice(x)\n  x * 2\n",
    )
    .unwrap();
    let loose = doc_json(&tmp, &["util.ptl"]);
    assert_eq!(loose["package"], Value::Null);
    assert_eq!(loose["modules"][0]["items"][0]["doc"]["summary"], "Twice.");
    std::fs::remove_dir_all(&tmp).unwrap();
    // Nothing by that name.
    let (_, err, code) = petal_in(&dir, &["doc", "no_such_package"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("no file, package or module named 'no_such_package'"),
        "{err}"
    );
    let (_, err, code) = petal_in(&inside, &["doc", "doc_pkg/nope"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("package 'doc_pkg' has no module 'nope'"),
        "{err}"
    );
}

#[test]
fn every_bundled_package_is_documented() {
    let dir = fixtures();
    for target in [
        "core",
        "std",
        "color",
        "vec2",
        "io",
        "json",
        "noise",
        "autodiff",
        "ui",
        "ui/input",
        "ui/drag",
        "bloom",
        "text_layout",
        "glsl",
        "core/math",
    ] {
        let report = doc_json(&dir, &[target]);
        validate(&report, &schema(), &schema(), target);
        let modules = report["modules"].as_array().unwrap();
        assert!(!modules.is_empty(), "{target}");
        for module in modules {
            assert!(module.get("error").is_none(), "{target}: {module}");
        }
        let items: usize = modules
            .iter()
            .map(|m| m["items"].as_array().unwrap().len())
            .sum();
        assert!(items > 0, "{target} has no items");
        if target != "std" {
            assert!(report["package"]["description"].is_string(), "{target}");
        }
        let (brief, err, code) = petal_in(&dir, &["doc", "--brief", target]);
        assert_eq!(code, 0, "{target}: {err}");
        assert!(brief.lines().count() > items / 2, "{target}");
    }
    // The prelude's functions are Petal; `core`'s are natives with call forms.
    let core = doc_json(&dir, &["core/math"]);
    let clamp = core["modules"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "clamp")
        .cloned()
        .expect("core/math declares clamp");
    assert_eq!(clamp["kind"], "native_fn");
    assert_eq!(clamp["forms"][0]["params"][1]["name"], "lo");
}

// ── a validator for the subset of JSON Schema the schema file uses ──────

fn validate(value: &Value, schema: &Value, root: &Value, at: &str) {
    if let Err(e) = check(value, schema, root, at) {
        panic!("does not match docs/schemas/petal-doc.schema.json: {e}");
    }
}

fn check(value: &Value, schema: &Value, root: &Value, at: &str) -> Result<(), String> {
    let fail = |what: String| Err(format!("{at}: {what}"));
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.strip_prefix("#/$defs/").expect("local $ref");
        return check(value, &root["$defs"][name], root, at);
    }
    if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
        let matching = options
            .iter()
            .filter(|s| check(value, s, root, at).is_ok())
            .count();
        if matching != 1 {
            return fail(format!("{value} matches {matching} of oneOf {options:?}"));
        }
        return Ok(());
    }
    if let Some(c) = schema.get("const")
        && value != c
    {
        return fail(format!("{value} is not {c}"));
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array)
        && !e.contains(value)
    {
        return fail(format!("{value} is not one of {e:?}"));
    }
    if let Some(t) = schema.get("type").and_then(Value::as_str) {
        let ok = match t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "null" => value.is_null(),
            other => panic!("schema type {other}"),
        };
        if !ok {
            return fail(format!("{value} is not a {t}"));
        }
    }
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|v| v < min)
    {
        return fail(format!("{value} < {min}"));
    }
    if let (Some(items), Some(list)) = (schema.get("items"), value.as_array()) {
        for (i, v) in list.iter().enumerate() {
            check(v, items, root, &format!("{at}[{i}]"))?;
        }
    }
    if let Some(obj) = value.as_object()
        && schema.get("type").and_then(Value::as_str) == Some("object")
    {
        for key in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = key.as_str().unwrap();
            if !obj.contains_key(key) {
                return fail(format!("missing `{key}`"));
            }
        }
        let props = schema.get("properties").and_then(Value::as_object);
        for (k, v) in obj {
            let at = format!("{at}.{k}");
            match props.and_then(|p| p.get(k)) {
                Some(s) => check(v, s, root, &at)?,
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => return Err(format!("{at}: not in the schema")),
                    Some(Value::Bool(true)) | None => {}
                    Some(s) => check(v, s, root, &at)?,
                },
            }
        }
    }
    Ok(())
}
