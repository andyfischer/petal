//! Parity between `const X = e` and `let X = const(e)`
//! (docs/tasks/lifecycle-annotation-functions-refactor.md, Phase 1).
//!
//! Every top-level `const` declaration in `examples/`, in the core's `.ptl`
//! fixtures, in the Petal sources held by the core's Rust tests and in the
//! `petal` snippets of `docs/language-guide.md` and
//! `docs/features/static-values.md` is rewritten into the annotation form, and
//! the two programs must be the same to everything that reads constants:
//!
//! - the IR, compared by [`petal::ir_equiv::ir_equivalent`] (which includes
//!   the terms' `is_const` flags), so the static cache and the golden IR see
//!   one program;
//! - `show-static --json` (kind, class, `const_check`, reasons, readers),
//!   with columns left out because the rewrite moves them;
//! - the compile's diagnostics, and a failed load's errors, by severity, line
//!   and message (both forms are reported as "declared `const`");
//! - the hot-reload construct kinds of an edit of each initializer.
//!
//! It is the gate for migrating Cheesecake (Phase 4).

use std::path::{Path, PathBuf};

use petal::ast::StmtKind;
use petal::env::Env;
use petal::source_map::FileId;

/// One rewritten declaration: its name and where the new form's initializer
/// starts and ends (char offsets into the rewritten text).
struct Rewritten {
    name: String,
    /// The initializer `e`, as char offsets into the old text.
    old_value: (usize, usize),
    /// `e` inside `const(e)`, as char offsets into the new text.
    new_value: (usize, usize),
}

/// Rewrite every top-level `const X = e` / `pub const X = e` in `src` into
/// `let X = const(e)` / `pub let X = const(e)`. `None` when `src` does not
/// parse or declares no `const`.
fn rewrite(src: &str) -> Option<(String, Vec<Rewritten>)> {
    let (_, stmts) = petal::cst::parse_source(src, FileId(0)).ok()?;
    let chars: Vec<char> = src.chars().collect();
    // (offset of the `const` keyword, value start, value end, name)
    let mut sites = Vec::new();
    for s in &stmts {
        let StmtKind::Let {
            name,
            is_const: true,
            ..
        } = &s.kind
        else {
            continue;
        };
        let start = s.span.start.offset as usize;
        let mut kw = start;
        if chars[kw..].starts_with(&['p', 'u', 'b']) {
            kw += 3;
            while chars[kw].is_whitespace() {
                kw += 1;
            }
        }
        assert!(
            chars[kw..].starts_with(&['c', 'o', 'n', 's', 't']),
            "a `const` declaration that does not start with the keyword: {name}"
        );
        // The initializer as written: from after the `=` to the end of the
        // statement. The value's own span leaves out grouping parens
        // (`(a) ++ b` starts at `a`), so it is not used for the edges.
        let eq = kw + chars[kw..].iter().position(|c| *c == '=').expect("an `=`");
        let mut vs = eq + 1;
        while chars[vs].is_whitespace() {
            vs += 1;
        }
        let ve = s.span.end.offset as usize;
        sites.push((kw, vs, ve, name.clone()));
    }
    if sites.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut done = Vec::new();
    let mut at = 0;
    let mut len = 0; // chars written so far
    let push = |out: &mut String, len: &mut usize, cs: &[char]| {
        out.extend(cs);
        *len += cs.len();
    };
    for (kw, vs, ve, name) in sites {
        push(&mut out, &mut len, &chars[at..kw]);
        push(&mut out, &mut len, &['l', 'e', 't']);
        push(&mut out, &mut len, &chars[kw + 5..vs]);
        push(&mut out, &mut len, &['c', 'o', 'n', 's', 't', '(']);
        let new_start = len;
        push(&mut out, &mut len, &chars[vs..ve]);
        let new_end = len;
        push(&mut out, &mut len, &[')']);
        at = ve;
        done.push(Rewritten {
            name,
            old_value: (vs, ve),
            new_value: (new_start, new_end),
        });
    }
    out.extend(&chars[at..]);
    Some((out, done))
}

/// `text` with the chars `range` replaced by `[` + themselves + `]`: an edit
/// of one initializer that a reload has to recompile.
fn wrap_in_list(text: &str, range: (usize, usize)) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars[..range.0].iter().collect();
    out.push('[');
    out.extend(&chars[range.0..range.1]);
    out.push(']');
    out.extend(&chars[range.1..]);
    out
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

/// The string literals of a Rust test file that hold a `const` declaration:
/// raw strings (`r"…"`, `r#"…"#`) as written, ordinary strings with their
/// simple escapes undone. Good enough for test fixtures; anything it misreads
/// fails to parse and is skipped.
fn rust_fixtures(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // A line comment: skip to its end.
        if b[i] == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // A char literal such as '"' must not open a string.
        if b[i] == '\'' && b.get(i + 2) == Some(&'\'') {
            i += 3;
            continue;
        }
        if b[i] == '\'' && b.get(i + 1) == Some(&'\\') && b.get(i + 3) == Some(&'\'') {
            i += 4;
            continue;
        }
        if b[i] == 'r' && (b.get(i + 1) == Some(&'"') || b.get(i + 1) == Some(&'#'))
            && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '_'))
        {
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) == Some(&'"') {
                let body_start = j + 1;
                let mut k = body_start;
                loop {
                    if k >= b.len() {
                        return out;
                    }
                    if b[k] == '"' && (0..hashes).all(|h| b.get(k + 1 + h) == Some(&'#')) {
                        break;
                    }
                    k += 1;
                }
                out.push(b[body_start..k].iter().collect());
                i = k + 1 + hashes;
                continue;
            }
        }
        if b[i] == '"' {
            let mut s = String::new();
            let mut k = i + 1;
            while k < b.len() && b[k] != '"' {
                if b[k] == '\\' && k + 1 < b.len() {
                    k += 1;
                    match b[k] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        '\n' => {
                            // A line continuation: skip the next line's indent.
                            while k + 1 < b.len() && b[k + 1].is_whitespace() {
                                k += 1;
                            }
                        }
                        c => s.push(c),
                    }
                } else {
                    s.push(b[k]);
                }
                k += 1;
            }
            out.push(s);
            i = k + 1;
            continue;
        }
        i += 1;
    }
    out.retain(|s| s.lines().any(|l| is_const_line(l)));
    out
}

fn is_const_line(line: &str) -> bool {
    let t = line.trim_start();
    let t = t.strip_prefix("pub ").unwrap_or(t);
    t.strip_prefix("const ")
        .is_some_and(|r| r.starts_with(|c: char| c.is_alphabetic() || c == '_'))
}

/// The ```petal blocks of a Markdown file that declare a `const`.
fn markdown_snippets(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in src.lines() {
        match &mut cur {
            None => {
                let t = line.trim_start();
                if t.starts_with("```petal") && !t.contains("ignore") {
                    cur = Some(String::new());
                }
            }
            Some(block) => {
                if line.trim_start().starts_with("```") {
                    if block.lines().any(is_const_line) {
                        out.push(std::mem::take(block));
                    }
                    cur = None;
                } else {
                    block.push_str(line);
                    block.push('\n');
                }
            }
        }
    }
    out
}

/// Drop every `"column"` key, at any depth: the rewrite moves columns.
fn without_columns(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(m) => {
            m.remove("column");
            m.values_mut().for_each(without_columns);
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(without_columns),
        _ => {}
    }
}

/// What a compile of `src` reports, by line and message, and its
/// `show-static --json`; or the load's errors, by line and message.
struct Compiled {
    env: Env,
    pid: Option<petal::program::ProgramId>,
    findings: Vec<String>,
    report: Option<serde_json::Value>,
}

fn compile(src: &str, origin: Option<&Path>) -> Compiled {
    let mut env = Env::new();
    env.set_check_only(true);
    match env.load_program_diag(src, origin) {
        Ok(pid) => {
            let program = env.get_program(pid).expect("loaded");
            let findings = program
                .warnings
                .iter()
                .map(|d| format!("{:?} line {}: {}", d.severity, d.span.start.line, d.message))
                .collect();
            let mut report = env.static_report(program).to_json(false);
            without_columns(&mut report);
            Compiled {
                env,
                pid: Some(pid),
                findings,
                report: Some(report),
            }
        }
        Err(e) => {
            let findings = e
                .items
                .iter()
                .map(|i| {
                    format!(
                        "{:?} line {:?}: {}",
                        e.phase,
                        i.span.map(|s| s.start.line),
                        i.message
                    )
                })
                .collect();
            Compiled {
                env,
                pid: None,
                findings,
                report: None,
            }
        }
    }
}

/// The construct changes a reload of `old` → `new` reports, positions left
/// out.
fn reload_kinds(old: &str, new: &str) -> String {
    let diff = petal::source_diff::diff_source(old, new, FileId(0));
    let constructs = match &diff.change {
        petal::source_diff::SourceChange::Constructs(cs) => cs
            .iter()
            .map(|c| format!("{:?} {:?} {:?}", c.kind, c.name, c.edit))
            .collect::<Vec<_>>()
            .join("; "),
        _ => String::new(),
    };
    format!("{} [{constructs}]", diff.change.label())
}

#[derive(Default)]
struct Tally {
    samples: usize,
    decls: usize,
    compiled: usize,
    failed_alike: usize,
}

fn check(shown: &str, src: &str, origin: Option<&Path>, tally: &mut Tally) {
    let Some((new, done)) = rewrite(src) else { return };
    // The annotation form parses wherever the keyword form does.
    petal::cst::parse_source(&new, FileId(0))
        .unwrap_or_else(|e| panic!("{shown}: the const(…) form does not parse: {e}\n{new}"));
    let a = compile(src, origin);
    let b = compile(&new, origin);
    assert_eq!(a.findings, b.findings, "{shown}: diagnostics differ\n--- new:\n{new}");
    match (a.pid, b.pid) {
        (Some(pa), Some(pb)) => {
            let (pa, pb) = (
                a.env.get_program(pa).expect("loaded"),
                b.env.get_program(pb).expect("loaded"),
            );
            if let Err(diff) = petal::ir_equiv::ir_equivalent(pa, pb) {
                panic!("{shown}: IR differs: {diff}\n--- new:\n{new}");
            }
            assert_eq!(a.report, b.report, "{shown}: show-static differs");
            tally.compiled += 1;
        }
        (None, None) => tally.failed_alike += 1,
        _ => panic!("{shown}: one form loads and the other does not"),
    }
    // An edit of each initializer reloads the same way.
    for r in &done {
        let old_edit = wrap_in_list(src, r.old_value);
        let new_edit = wrap_in_list(&new, r.new_value);
        assert_eq!(
            reload_kinds(src, &old_edit),
            reload_kinds(&new, &new_edit),
            "{shown}: a reload of an edit to {} differs",
            r.name
        );
    }
    tally.samples += 1;
    tally.decls += done.len();
}

#[test]
fn const_annotation_matches_const_declaration_across_the_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut tally = Tally::default();

    let mut files = Vec::new();
    ptl_files(&root.join("examples"), &mut files);
    ptl_files(&root.join("core/tests"), &mut files);
    files.sort();
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        if !src.lines().any(is_const_line) {
            continue;
        }
        let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        check(&shown, &src, Some(path), &mut tally);
    }
    let ptl_samples = tally.samples;

    let mut rust = Vec::new();
    for e in std::fs::read_dir(root.join("core/tests")).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "rs") {
            rust.push(p);
        }
    }
    rust.sort();
    for path in &rust {
        let src = std::fs::read_to_string(path).unwrap();
        for (i, fixture) in rust_fixtures(&src).iter().enumerate() {
            let shown = format!(
                "{} fixture {i}",
                path.strip_prefix(&root).unwrap_or(path).display()
            );
            check(&shown, fixture, None, &mut tally);
        }
    }
    let rust_samples = tally.samples - ptl_samples;

    for doc in ["docs/language-guide.md", "docs/features/static-values.md"] {
        let src = std::fs::read_to_string(root.join(doc)).unwrap();
        for (i, snippet) in markdown_snippets(&src).iter().enumerate() {
            check(&format!("{doc} snippet {i}"), snippet, None, &mut tally);
        }
    }
    let doc_samples = tally.samples - ptl_samples - rust_samples;

    eprintln!(
        "const parity: {} samples ({ptl_samples} .ptl files, {rust_samples} Rust fixtures, \
         {doc_samples} doc snippets), {} declarations, {} compiled and compared, {} failed \
         to load alike",
        tally.samples, tally.decls, tally.compiled, tally.failed_alike
    );
    assert!(ptl_samples >= 5, "only {ptl_samples} .ptl files declare a `const`");
    assert!(rust_samples >= 20, "only {rust_samples} Rust fixtures declare a `const`");
    assert!(doc_samples >= 2, "only {doc_samples} doc snippets declare a `const`");
    assert!(tally.compiled >= 30, "only {} samples compiled to compare", tally.compiled);
}

#[test]
fn the_rewriter_wraps_the_whole_initializer() {
    let (new, done) = rewrite(
        "pub const A = 1 + 2  // three\nconst B = [\n  1,\n  2,\n]\nlet c = A\n",
    )
    .unwrap();
    assert_eq!(
        new,
        "pub let A = const(1 + 2)  // three\nlet B = const([\n  1,\n  2,\n])\nlet c = A\n"
    );
    assert_eq!(done.len(), 2);
    assert!(rewrite("let x = 1\n").is_none());
}

/// `const(…)` outside a top-level `let`'s whole initializer is an error,
/// and so is one around a setting.
#[test]
fn misplaced_const_annotations_are_errors() {
    let errors = |src: &str| -> Vec<String> {
        let mut env = Env::new();
        env.set_check_only(true);
        match env.load_program(src) {
            Ok(pid) => env
                .get_program(pid)
                .unwrap()
                .warnings
                .iter()
                .filter(|d| d.severity == petal::diagnostic::Severity::Error)
                .map(|d| d.message.clone())
                .collect(),
            Err(e) => vec![e],
        }
    };
    let one = |src: &str, want: &str| {
        let got = errors(src);
        assert!(
            got.iter().any(|m| m.contains(want)),
            "{src:?}: wanted an error containing {want:?}, got {got:?}"
        );
    };
    one("let T = {a: const(1)}\nprint(T)\n", "not a part of it");
    one("var V = const(3)\nprint(V)\n", "a `var` is assigned again");
    one("state s = const(3)\nprint(s)\n", "a `state` slot cannot be constant");
    one("fn f()\n  let x = const(2)\n  return x\nend\nprint(f())\n", "inside a function");
    one("let S = const(tunable(4, 0, 10))\nprint(S)\n", "drop `const(…)`");
    one("config let S = const(4)\nprint(S)\n", "drop `const(…)`");
    one("let X = const(1, 2)\nprint(X)\n", "takes one argument");
    one("let X = const(random())\nprint(X)\n", "is declared `const` but is not constant");
    one("let F = const(fn() -> 1)\nprint(F)\n", "is a function");
    // The whole initializer of a top-level `let`: no error.
    assert_eq!(errors("let X = const([1, 2])\npub let Y = const(len(X))\nprint(Y)\n"), Vec::<String>::new());
    // `tunable(const(e))` is legal.
    assert_eq!(errors("let S = tunable(const(4))\nprint(S)\n"), Vec::<String>::new());
}

/// The annotation is erased: the program runs as its argument.
#[test]
fn const_annotation_runs_as_its_argument() {
    let mut env = Env::new();
    env.set_echo(false);
    let src = "let X = const([1, 2, 3])\nlet N = const(len(X) * 2)\nprint(N, X)\n";
    let pid = env.load_program(src).expect("loads");
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).expect("runs");
    assert_eq!(env.take_output().join("\n").trim(), "6 [1, 2, 3]");
}
