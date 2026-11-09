//! Parity between `config let X = v` and `let X = tunable(v)`
//! (docs/tasks/lifecycle-annotation-functions-refactor.md).
//!
//! Every single-line `config let` in `examples/` and in the core's `.ptl`
//! fixtures is rewritten into the `tunable` form, and the two files must look
//! the same to everything that reads settings: `static_bindings` (value,
//! text, range, the setting flag), path edits by name (`literal_edit`, so
//! `set_config_value` and the C bridge), and the compiler's config marks
//! (which direct manipulation ranks and selects by).

use std::path::{Path, PathBuf};

use petal::env::Env;
use petal::program::{ProgramId, TermOp};
use petal::static_value::{StaticBinding, StaticValue, static_bindings};

/// Where a line comment starts, outside a string literal.
fn comment_start(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i + 1 < b.len() {
        match b[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && b[i + 1] == b'/' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// `[pub ]config let NAME[: T] = v  // c` → `[pub ]let NAME[: T] = tunable(v)  // c`,
/// and `config let X = tunable(…)` → `let X = tunable(…)`. A multi-line
/// initializer is left alone (the bracket does not close on the line).
fn rewrite_line(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let (prefix, rest) = match rest.strip_prefix("pub ") {
        Some(r) => ("pub ", r),
        None => ("", rest),
    };
    let rest = rest.strip_prefix("config let ")?;
    let eq = rest.find('=')?;
    let (decl, after) = (&rest[..eq], &rest[eq + 1..]);
    let (init, comment) = match comment_start(after) {
        Some(c) => (&after[..c], &after[c..]),
        None => (after, ""),
    };
    let value = init.trim();
    let opens = value.matches(['(', '[', '{']).count();
    let closes = value.matches([')', ']', '}']).count();
    if value.is_empty() || opens != closes {
        return None;
    }
    let wrapped = if value.starts_with("tunable(") {
        value.to_string()
    } else {
        format!("tunable({value})")
    };
    let pad = if comment.is_empty() { "" } else { "  " };
    Some(format!(
        "{}{prefix}let {}= {wrapped}{pad}{comment}",
        &line[..indent],
        decl
    ))
}

fn rewrite(src: &str) -> (String, Vec<String>) {
    let mut names = Vec::new();
    let mut out = String::new();
    for line in src.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        match rewrite_line(body) {
            Some(new) => {
                let name = new
                    .trim_start()
                    .trim_start_matches("pub ")
                    .trim_start_matches("let ")
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .unwrap_or("")
                    .to_string();
                names.push(name);
                out.push_str(&new);
            }
            None => out.push_str(body),
        }
        out.push_str(nl);
    }
    (out, names)
}

fn ptl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !matches!(p.file_name().and_then(|n| n.to_str()), Some("target" | "node_modules")) {
                ptl_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "ptl") {
            out.push(p);
        }
    }
}

/// What a host sees of one binding, with the line left out (the rewrite
/// keeps lines, but a host's view should not depend on it anyway).
fn host_view(b: &StaticBinding) -> String {
    format!(
        "{} value={:?} text={:?} setting={} exported={} range={:?}",
        b.name, b.value, b.text, b.is_config, b.exported, b.range
    )
}

/// The constant terms the compiler marked `is_config`, by value.
fn config_constants(src: &str) -> Option<Vec<String>> {
    let mut env = Env::new();
    env.set_check_only(true);
    let program = env.compile_program(ProgramId(0), src).ok()?;
    let mut out: Vec<String> = program
        .terms
        .iter()
        .filter(|t| t.is_config && matches!(t.op, TermOp::Constant(_)))
        .map(|t| match t.op {
            TermOp::Constant(c) => format!("{:?}", program.constants.get(c)),
            _ => unreachable!(),
        })
        .collect();
    out.sort();
    Some(out)
}

#[test]
fn tunable_settings_match_config_let_across_the_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    ptl_files(&root.join("examples"), &mut files);
    ptl_files(&root.join("core/tests"), &mut files);
    files.sort();
    let mut checked = 0;
    let mut settings = 0;
    let mut compiled = 0;
    for path in files {
        let src = std::fs::read_to_string(&path).unwrap();
        if !src.contains("config let") {
            continue;
        }
        let (new, names) = rewrite(&src);
        if names.is_empty() {
            continue;
        }
        let shown = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
        let (Ok(old_b), Ok(new_b)) = (static_bindings(&src), static_bindings(&new)) else {
            panic!("{shown}: one of the two forms does not parse");
        };
        let old_v: Vec<String> = old_b.iter().map(host_view).collect();
        let new_v: Vec<String> = new_b.iter().map(host_view).collect();
        assert_eq!(old_v, new_v, "{shown}: static_bindings differ");
        // Setting each rewritten binding by name edits the same literal.
        for name in &names {
            let Some(b) = old_b.iter().find(|b| &b.name == name) else { continue };
            let value = match &b.value {
                Ok(StaticValue::Int(n)) => StaticValue::int(n + 1),
                Ok(StaticValue::Float(f)) => StaticValue::float(f + 0.5),
                Ok(StaticValue::Bool(v)) => StaticValue::Bool(!v),
                Ok(StaticValue::Str(_)) => StaticValue::Str("parity".into()),
                _ => continue,
            };
            let a = petal::literal_edit::set_path(&src, name, &value)
                .unwrap_or_else(|e| panic!("{shown}: set {name} on config let: {e:?}"));
            let b2 = petal::literal_edit::set_path(&new, name, &value)
                .unwrap_or_else(|e| panic!("{shown}: set {name} on tunable: {e:?}"));
            let (ab, bb) = (static_bindings(&a).unwrap(), static_bindings(&b2).unwrap());
            let pick = |bs: &[StaticBinding]| {
                bs.iter().find(|x| &x.name == name).map(host_view)
            };
            assert_eq!(pick(&ab), pick(&bb), "{shown}: after setting {name}");
            settings += 1;
        }
        // The compiler marks the same literals as config.
        if let Some(old_c) = config_constants(&src) {
            let new_c = config_constants(&new)
                .unwrap_or_else(|| panic!("{shown}: the tunable form does not compile"));
            assert_eq!(old_c, new_c, "{shown}: config-marked literals differ");
            compiled += 1;
        }
        checked += 1;
    }
    eprintln!("parity: {checked} files, {settings} settings set by name, {compiled} compiled");
    assert!(checked >= 15, "only {checked} files had a single-line `config let`");
    assert!(settings >= 100, "only {settings} settings were set by name");
    assert!(compiled >= 10, "only {compiled} files compiled to compare marks");
}

#[test]
fn the_rewriter_keeps_comments_and_skips_multi_line_values() {
    assert_eq!(
        rewrite_line("pub config let SPEED = 4.0   // m/s").as_deref(),
        Some("pub let SPEED = tunable(4.0)  // m/s")
    );
    assert_eq!(
        rewrite_line("config let URL = \"a//b\"").as_deref(),
        Some("let URL = tunable(\"a//b\")")
    );
    assert_eq!(
        rewrite_line("config let R = tunable(2, 0, 5)").as_deref(),
        Some("let R = tunable(2, 0, 5)")
    );
    assert_eq!(rewrite_line("config let POST = {"), None);
    assert_eq!(rewrite_line("let x = 1"), None);
}
