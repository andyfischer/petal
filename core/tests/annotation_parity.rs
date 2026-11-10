//! Parity between the call form of an annotation and its `@` form
//! (docs/proposals/annotation-syntax.md, D1):
//!
//! ```text
//! let X = const(e)             ≡   @const let X = e
//! let S = tunable(4.0, 0, 10)  ≡   @tunable(0, 10) let S = 4.0
//! let G = rigid(9.8)           ≡   @rigid let G = 9.8
//! ```
//!
//! Every `let`/`var`/`state`/`memo` (top level and in function bodies) whose
//! whole initializer is a `const(…)`, `tunable(…)` or `rigid(…)` call, in
//! `examples/`, the core's `.ptl` fixtures, the Petal sources held by the
//! core's Rust tests and the `petal` snippets of the language guide and the
//! settings and static-values docs, is rewritten into the `@` form, and the
//! two programs must be the same to everything that reads them:
//!
//! - the IR, by [`petal::ir_equiv::ir_equivalent`] (with the terms'
//!   `is_const`/`is_config` flags);
//! - `show-static --json`, with columns left out (the rewrite moves them);
//! - the compile's diagnostics and a failed load's errors, by severity, line
//!   and message;
//! - the hot-reload construct kinds of an edit of each initializer;
//! - `static_bindings` (what hosts and the C bridge list as settings) and a
//!   setting's value set by name (`literal_edit::set_path`, which is
//!   `set_config_value` and `propose-edit`'s literal rewrite);
//! - the direct-manipulation knobs `dm::read_hints` finds, by literal text.
//!
//! The rewrite is done twice: with each annotation on the declaration's line
//! (lines unchanged, so everything is compared as is), and with each on a line
//! of its own above the declaration (lines move, so line numbers are left out).

use std::path::{Path, PathBuf};

use petal::ast::{Expr, ExprKind, Stmt, StmtKind};
use petal::env::Env;
use petal::lexer::{Lexer, Token};
use petal::source_map::FileId;
use petal::static_value::{StaticBinding, StaticValue, static_bindings};

/// One rewritten declaration: its name, and its value's char range in the
/// call form and in the `@` form.
struct Rewritten {
    name: String,
    old_value: (usize, usize),
    new_value: (usize, usize),
}

/// The value of a binding statement and its name.
fn binding(s: &Stmt) -> Option<(&str, &Expr)> {
    match &s.kind {
        StmtKind::Let { name, value, .. } => Some((name, value)),
        StmtKind::State { name, init, .. } | StmtKind::Memo { name, init, .. } => {
            Some((name, init))
        }
        _ => None,
    }
}

/// Every binding statement of `stmts`, at the top level and in `fn` bodies.
fn bindings<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Stmt>) {
    for s in stmts {
        match &s.kind {
            StmtKind::FnDecl { body, .. } => bindings(body, out),
            _ if binding(s).is_some() => out.push(s),
            _ => {}
        }
    }
}

/// Rewrite every whole-initializer annotation call of `src` into the `@`
/// form. `None` when `src` does not parse or has none.
fn rewrite(src: &str, own_line: bool) -> Option<(String, Vec<Rewritten>)> {
    let (_, stmts) = petal::cst::parse_source(src, FileId(0)).ok()?;
    let mut lexer = Lexer::new(src);
    lexer.tokenize().ok()?;
    let (tokens, spans) = (&lexer.tokens, &lexer.token_spans);
    let tok_at = |offset: usize| spans.iter().position(|s| s.start.offset as usize == offset);
    let chars: Vec<char> = src.chars().collect();

    let mut all = Vec::new();
    bindings(&stmts, &mut all);
    // (stmt start, annotation text, call start, value start, value end,
    //  call end, name, line indent)
    let mut sites = Vec::new();
    for s in all {
        let (name, value) = binding(s).expect("a binding");
        let ExprKind::Call { function, .. } = &value.kind else { continue };
        let ExprKind::Ident(f) = &function.kind else { continue };
        if !matches!(f.as_str(), "const" | "tunable" | "rigid") || petal::ast::is_annotation_call(value) {
            continue;
        }
        // The call must be the statement's whole tail, unparenthesized.
        let call_start = function.span.start.offset as usize;
        let call_end = value.span.end.offset as usize;
        if call_end != s.span.end.offset as usize {
            continue;
        }
        let Some(k) = tok_at(call_start) else { continue };
        if k == 0 || tokens[k - 1] != Token::Assign || tokens.get(k + 1) != Some(&Token::LParen) {
            continue;
        }
        // The first argument runs to the first top-level comma.
        let mut depth = 0i32;
        let mut comma = None;
        let mut close = None;
        for (j, t) in tokens.iter().enumerate().skip(k + 1) {
            match t {
                Token::LParen | Token::LBracket | Token::LBrace => depth += 1,
                Token::RParen | Token::RBracket | Token::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                Token::Comma if depth == 1 && comma.is_none() => comma = Some(j),
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        if spans[close].end.offset as usize != call_end {
            continue;
        }
        let open_end = spans[k + 1].end.offset as usize;
        let first_end = spans[comma.unwrap_or(close)].start.offset as usize;
        let text = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
        let first = text(open_end, first_end);
        let (vs, ve) = (
            open_end + (first.len() - first.trim_start().len()),
            first_end - (first.len() - first.trim_end().len()),
        );
        // A first argument named `value:` stays a call.
        if text(vs, ve).starts_with("value:") || vs >= ve {
            continue;
        }
        let rest = match comma {
            Some(c) => {
                let r = text(spans[c].end.offset as usize, spans[close].start.offset as usize);
                let r = r.trim().trim_end_matches(',').trim().to_string();
                r
            }
            None => String::new(),
        };
        if rest.contains('\n') && !own_line {
            // A multi-line argument list would move lines.
            continue;
        }
        let annotation = if rest.is_empty() { format!("@{f}") } else { format!("@{f}({rest})") };
        let stmt_start = s.span.start.offset as usize;
        let line_start = chars[..stmt_start].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
        let indent: String = chars[line_start..stmt_start].iter().collect();
        if !indent.chars().all(char::is_whitespace) {
            continue;
        }
        sites.push((stmt_start, annotation, call_start, vs, ve, call_end, name.to_string(), indent));
    }
    if sites.is_empty() {
        return None;
    }
    sites.sort_by_key(|s| s.0);
    let mut out: Vec<char> = Vec::new();
    let mut done = Vec::new();
    let mut at = 0;
    for (stmt_start, annotation, call_start, vs, ve, call_end, name, indent) in sites {
        if stmt_start < at {
            continue; // nested in an earlier site (cannot happen for bindings)
        }
        out.extend(&chars[at..stmt_start]);
        out.extend(annotation.chars());
        if own_line {
            out.push('\n');
            out.extend(indent.chars());
        } else {
            out.push(' ');
        }
        out.extend(&chars[stmt_start..call_start]);
        let new_start = out.len();
        out.extend(&chars[vs..ve]);
        let new_end = out.len();
        at = call_end;
        done.push(Rewritten {
            name,
            old_value: (vs, ve),
            new_value: (new_start, new_end),
        });
    }
    out.extend(&chars[at..]);
    Some((out.into_iter().collect(), done))
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

/// Whether a line holds an annotation call (a cheap pre-filter).
fn has_annotation_call(src: &str) -> bool {
    src.contains("const(") || src.contains("tunable(") || src.contains("rigid(")
}

/// The string literals of a Rust test file that hold an annotation call: raw
/// strings as written, ordinary strings with their simple escapes undone.
/// Anything it misreads fails to parse and is skipped.
fn rust_fixtures(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == '\'' && b.get(i + 2) == Some(&'\'') {
            i += 3;
            continue;
        }
        if b[i] == '\'' && b.get(i + 1) == Some(&'\\') && b.get(i + 3) == Some(&'\'') {
            i += 4;
            continue;
        }
        if b[i] == 'r'
            && (b.get(i + 1) == Some(&'"') || b.get(i + 1) == Some(&'#'))
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
    out.retain(|s| has_annotation_call(s));
    out
}

/// The ```petal blocks of a Markdown file that hold an annotation call.
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
                    if has_annotation_call(block) {
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

/// Drop every `"column"` key at any depth, and every `"line"` key when
/// `lines` is false.
fn without_positions(v: &mut serde_json::Value, lines: bool) {
    match v {
        serde_json::Value::Object(m) => {
            m.remove("column");
            if !lines {
                m.remove("line");
            }
            m.values_mut().for_each(|v| without_positions(v, lines));
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|v| without_positions(v, lines)),
        // "the function at line 317"
        serde_json::Value::String(t) if !lines => *t = mask_numbers(t),
        _ => {}
    }
}

/// `t` with every run of digits replaced by `_`: a message that names lines
/// ("the function at line 317"), compared where lines move.
fn mask_numbers(t: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in t.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('_');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

struct Compiled {
    env: Env,
    pid: Option<petal::program::ProgramId>,
    findings: Vec<String>,
    report: Option<serde_json::Value>,
    knobs: Vec<String>,
}

fn compile(src: &str, origin: Option<&Path>, lines: bool) -> Compiled {
    let line = |l: u32| if lines { l.to_string() } else { "_".to_string() };
    // A misplaced `const` is named as written (`@const` or `const(…)`);
    // otherwise the two forms are told the same thing.
    let msg = |m: &str| {
        let m = m
            .replace("`@const` checks", "`const(…)` checks")
            .replace("rop `@const`", "rop `const(…)`")
            .replace(
                "`@const` takes no arguments: the declared value is the one that must be constant",
                "`const(…)` takes one argument, the value that must be constant",
            );
        if lines { m } else { mask_numbers(&m) }
    };
    let mut env = Env::new();
    env.set_check_only(true);
    match env.load_program_diag(src, origin) {
        Ok(pid) => {
            let program = env.get_program(pid).expect("loaded");
            let findings = program
                .warnings
                .iter()
                .map(|d| format!("{:?} line {}: {}", d.severity, line(d.span.start.line), msg(&d.message)))
                .collect();
            let mut report = env.static_report(program).to_json(false);
            without_positions(&mut report, lines);
            // The knobs direct manipulation finds, by the literal's text.
            let chars: Vec<char> = src.chars().collect();
            let hints = petal::dm::read_hints(program, env.native_table());
            let mut knobs: Vec<String> = program
                .terms
                .iter()
                .filter_map(|t| {
                    let hint = hints.get(t.id)?;
                    let span = program.source_map.get(t.id)?;
                    let (a, b) = (span.start.offset as usize, span.end.offset as usize);
                    let text: String = chars.get(a..b)?.iter().collect();
                    Some(format!("{text} {hint:?}"))
                })
                .collect();
            knobs.sort();
            Compiled { env, pid: Some(pid), findings, report: Some(report), knobs }
        }
        Err(e) => {
            let findings = e
                .items
                .iter()
                .map(|i| {
                    format!(
                        "{:?} line {}: {}",
                        e.phase,
                        i.span.map_or("-".to_string(), |s| line(s.start.line)),
                        msg(&i.message)
                    )
                })
                .collect();
            Compiled { env, pid: None, findings, report: None, knobs: Vec::new() }
        }
    }
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

/// What a host sees of one binding. The `text` of a binding whose value is
/// the annotation call itself (`rigid(v)`, a malformed `const(1, 2)`) is the
/// source after the `=`, which is the call in one form and `v` in the other,
/// so it is left out there.
fn host_view(b: &StaticBinding, lines: bool) -> String {
    let is_call = matches!(&b.value, Ok(StaticValue::Call { .. }));
    let text = if is_call { None } else { b.text.clone() };
    format!(
        "{} value={:?} text={:?} setting={} exported={} range={:?} options={:?} comment={:?} line={}",
        b.name,
        b.value,
        text,
        b.is_config,
        b.exported,
        b.range,
        b.options,
        // An own-line annotation sits between a comment and its binding.
        if lines { b.comment.clone() } else { None },
        if lines { b.line } else { 0 },
    )
}

#[derive(Default)]
struct Tally {
    samples: usize,
    decls: usize,
    compiled: usize,
    failed_alike: usize,
    set_by_name: usize,
    knobs: usize,
}

fn check(shown: &str, src: &str, origin: Option<&Path>, own_line: bool, tally: &mut Tally) {
    let Some((new, done)) = rewrite(src, own_line) else { return };
    let lines = !own_line;
    petal::cst::parse_source(&new, FileId(0))
        .unwrap_or_else(|e| panic!("{shown}: the @ form does not parse: {e}\n{new}"));
    let a = compile(src, origin, lines);
    let b = compile(&new, origin, lines);
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
            if a.report != b.report {
                let decls = |r: &Option<serde_json::Value>| -> Vec<serde_json::Value> {
                    r.as_ref().and_then(|r| r["declarations"].as_array().cloned()).unwrap_or_default()
                };
                let (da, db) = (decls(&a.report), decls(&b.report));
                let first = da.iter().zip(&db).find(|(x, y)| x != y);
                panic!("{shown}: show-static differs: {first:#?}\n--- new:\n{new}");
            }
            assert_eq!(a.knobs, b.knobs, "{shown}: DM knobs differ\n--- new:\n{new}");
            tally.knobs += a.knobs.len();
            tally.compiled += 1;
        }
        (None, None) => tally.failed_alike += 1,
        _ => panic!("{shown}: one form loads and the other does not\n{:?}\n{:?}\n--- new:\n{new}", a.findings, b.findings),
    }
    // Hosts list the same bindings, and set them by name the same way.
    if let (Ok(old_b), Ok(new_b)) = (static_bindings(src), static_bindings(&new)) {
        let ov: Vec<String> = old_b.iter().map(|b| host_view(b, lines)).collect();
        let nv: Vec<String> = new_b.iter().map(|b| host_view(b, lines)).collect();
        assert_eq!(ov, nv, "{shown}: static_bindings differ\n--- new:\n{new}");
        for r in &done {
            let Some(b) = old_b.iter().find(|b| b.name == r.name) else { continue };
            let value = match &b.value {
                Ok(StaticValue::Int(n)) => StaticValue::int(n + 1),
                Ok(StaticValue::Float(f)) => StaticValue::float(f + 0.5),
                Ok(StaticValue::Bool(v)) => StaticValue::Bool(!v),
                Ok(StaticValue::Str(_)) => StaticValue::Str("parity".into()),
                _ => continue,
            };
            let (Ok(x), Ok(y)) = (
                petal::literal_edit::set_path(src, &r.name, &value),
                petal::literal_edit::set_path(&new, &r.name, &value),
            ) else {
                panic!("{shown}: setting {} by name works in one form only", r.name);
            };
            let pick = |s: &str| {
                static_bindings(s)
                    .unwrap()
                    .iter()
                    .find(|x| x.name == r.name)
                    .map(|b| host_view(b, lines))
            };
            assert_eq!(pick(&x), pick(&y), "{shown}: after setting {}", r.name);
            tally.set_by_name += 1;
        }
    }
    // An edit of each initializer reloads the same way.
    for r in &done {
        assert_eq!(
            reload_kinds(src, &wrap_in_list(src, r.old_value)),
            reload_kinds(&new, &wrap_in_list(&new, r.new_value)),
            "{shown}: a reload of an edit to {} differs",
            r.name
        );
    }
    // Respelling one form as the other recompiles (`full`, or `constructs`
    // when it also moves a function body), never `none`: the trees are equal
    // but every span after the edit moved.
    assert_ne!(
        petal::source_diff::diff_source(src, &new, FileId(0)).change.label(),
        "none",
        "{shown}: respelling the call as `@` must not be classified as layout"
    );
    tally.samples += 1;
    tally.decls += done.len();
}

fn run_corpus(own_line: bool) -> Tally {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut tally = Tally::default();
    let mut files = Vec::new();
    ptl_files(&root.join("examples"), &mut files);
    ptl_files(&root.join("core/tests"), &mut files);
    files.sort();
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        if !has_annotation_call(&src) {
            continue;
        }
        let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        check(&shown, &src, Some(path), own_line, &mut tally);
    }
    let mut rust = Vec::new();
    for e in std::fs::read_dir(root.join("core/tests")).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("annotation_parity.rs") {
            rust.push(p);
        }
    }
    rust.sort();
    for path in &rust {
        let src = std::fs::read_to_string(path).unwrap();
        for (i, fixture) in rust_fixtures(&src).iter().enumerate() {
            let shown = format!("{} fixture {i}", path.strip_prefix(&root).unwrap_or(path).display());
            check(&shown, fixture, None, own_line, &mut tally);
        }
    }
    for doc in [
        "docs/language-guide.md",
        "docs/syntax/settings.md",
        "docs/features/static-values.md",
        "docs/Builtins.md",
    ] {
        let src = std::fs::read_to_string(root.join(doc)).unwrap();
        for (i, snippet) in markdown_snippets(&src).iter().enumerate() {
            check(&format!("{doc} snippet {i}"), snippet, None, own_line, &mut tally);
        }
    }
    eprintln!(
        "annotation parity (own_line={own_line}): {} samples, {} declarations, {} compiled, \
         {} failed alike, {} set by name, {} DM knobs",
        tally.samples, tally.decls, tally.compiled, tally.failed_alike, tally.set_by_name, tally.knobs
    );
    tally
}

#[test]
fn same_line_annotations_match_the_call_form_across_the_corpus() {
    let t = run_corpus(false);
    assert!(t.samples >= 50, "only {} samples", t.samples);
    assert!(t.decls >= 150, "only {} declarations", t.decls);
    assert!(t.compiled >= 40, "only {} compiled", t.compiled);
    assert!(t.set_by_name >= 50, "only {} set by name", t.set_by_name);
    assert!(t.knobs >= 50, "only {} DM knobs", t.knobs);
}

#[test]
fn own_line_annotations_match_the_call_form_across_the_corpus() {
    let t = run_corpus(true);
    assert!(t.samples >= 50, "only {} samples", t.samples);
    assert!(t.compiled >= 40, "only {} compiled", t.compiled);
}

#[test]
fn the_rewriter_moves_the_call_onto_the_declaration() {
    let src = "pub let S = tunable(4.0, 0, 10)  // speed\nlet K = const([\n  1,\n])\nfn f()\n  let g = rigid(2)\n  g\nend\nlet n = tunable(1) * 2\n";
    let (new, done) = rewrite(src, false).unwrap();
    assert_eq!(
        new,
        "@tunable(0, 10) pub let S = 4.0  // speed\n@const let K = [\n  1,\n]\nfn f()\n  @rigid let g = 2\n  g\nend\nlet n = tunable(1) * 2\n"
    );
    assert_eq!(done.len(), 3);
    let (own, _) = rewrite(src, true).unwrap();
    assert!(own.starts_with("@tunable(0, 10)\npub let S = 4.0"), "{own}");
    assert!(own.contains("  @rigid\n  let g = 2"), "{own}");
}

/// The lowering itself: the same AST as the call, up to spans.
fn ast_shape(src: &str) -> String {
    let (_, stmts) = petal::cst::parse_source(src, FileId(0)).expect("parses");
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.remove("span");
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(&stmts).expect("serializes");
    strip(&mut v);
    v.to_string()
}

#[test]
fn annotations_lower_to_the_call() {
    let same = |a: &str, b: &str| assert_eq!(ast_shape(a), ast_shape(b), "{a:?} vs {b:?}");
    same("@tunable(0, 10) let S = 4.0\n", "let S = tunable(4.0, 0, 10)\n");
    same("@tunable let X = 4\n", "let X = tunable(4)\n");
    same("@tunable\nlet X = 4\n", "let X = tunable(4)\n");
    same(
        "@tunable(options: [\"title\", \"play\"]) let START = \"title\"\n",
        "let START = tunable(\"title\", options: [\"title\", \"play\"])\n",
    );
    same("@tunable(min: 3.0, max: 12.0) pub let RUN = 7.0\n", "pub let RUN = tunable(7.0, min: 3.0, max: 12.0)\n");
    same("@tunable(0, 10, step: 2) let S = 4\n", "let S = tunable(4, 0, 10, step: 2)\n");
    // Stacking applies bottom-up: the nearest annotation is innermost.
    same("@const @tunable let X = 4\n", "let X = const(tunable(4))\n");
    same("@const\n@tunable(0, 9)\nlet X = 4\n", "let X = const(tunable(4, 0, 9))\n");
    same("@rigid @tunable(0, 9) let X = 4\n", "let X = rigid(tunable(4, 0, 9))\n");
    // Comments and blank lines may sit between an annotation and its
    // declaration.
    same("@const\n// the table\n\nlet T = [1, 2]\n", "let T = const([1, 2])\n");
    // Inside a function, and on the other bindings.
    same("fn f()\n  @tunable let g = 3\n  g\nend\n", "fn f()\n  let g = tunable(3)\n  g\nend\n");
    same("@tunable var V = 1\n", "var V = tunable(1)\n");
    same("@tunable state s = 1\n", "state s = tunable(1)\n");
    same("@rigid memo(k) m = 1\n", "memo(k) m = rigid(1)\n");
    same("@tunable let x: float = 1.0\n", "let x: float = tunable(1.0)\n");
}

#[test]
fn the_call_built_from_an_annotation_is_recognizable() {
    let (_, stmts) = petal::cst::parse_source(
        "@tunable(0, 10) let A = 4\nlet B = tunable(4, 0, 10)\nlet C = 4 |> tunable(0, 10)\n",
        FileId(0),
    )
    .unwrap();
    let value = |i: usize| match &stmts[i].kind {
        StmtKind::Let { value, .. } => value.clone(),
        _ => unreachable!(),
    };
    assert!(petal::ast::is_annotation_call(&value(0)));
    assert!(!petal::ast::is_annotation_call(&value(1)));
    assert!(!petal::ast::is_annotation_call(&value(2)));
    // Its span is the value's; the statement's starts at the `@`.
    let a = value(0);
    assert_eq!((a.span.start.column, a.span.end.column), (25, 26));
    assert_eq!(stmts[0].span.start.column, 1);
}

/// What a source that does not parse reports.
fn parse_error(src: &str) -> String {
    match petal::cst::parse_source(src, FileId(0)) {
        Ok(_) => panic!("{src:?} parsed"),
        Err(e) => e,
    }
}

#[test]
fn misplaced_annotations_are_parse_errors() {
    let err = |src: &str, want: &str| {
        let got = parse_error(src);
        assert!(got.contains(want), "{src:?}: wanted {want:?} in {got:?}");
    };
    err("@foo let X = 1\n", "`@foo` is not an annotation. `@` takes `const`, `tunable` or `rigid`. To pass the value through a function, write `let X = foo(1)`");
    err("@foo pub let X = \"a\"\n", "write `pub let X = foo(\"a\")`");
    err("@tunable(0, 10) fn speed()\n  1\nend\n", "`@` on a function is reserved for compile-time annotations; there are none yet. `@tunable` annotates a `let`; a function is not a setting.");
    err("@logged\nfn fact(n)\n  n\nend\n", "`@` on a function is reserved for compile-time annotations; there are none yet.");
    err("@const pub fn f()\n  1\nend\n", "reserved for compile-time annotations");
    err("@rigid native fn f() = \"g\"\n", "reserved for compile-time annotations");
    err("@const\n", "an annotation must be followed by a declaration (`let`, `pub let`)");
    err("@tunable 4\n", "an annotation must be followed by a declaration");
    err("@tunable x = 4\n", "an annotation must be followed by a declaration");
    err("@tunable set x = 4\n", "an annotation must be followed by a declaration");
    err("let y = @rigid 2.0\n", "`@` annotates a declaration and goes before `let`");
    err("let y = @rigid 2.0\n", "write the call: `rigid(…)`");
    err("print(@const 1)\n", "`@` annotates a declaration");
    err("pub @tunable let X = 1\n", "annotations go before `pub`");
    err("@tunable pub @rigid let X = 1\n", "annotations go before `pub`");
    err("@tunable class P\n  x: int\nend\n", "a class declaration takes no annotations");
    err("@tunable enum E\n  a\nend\n", "an enum declaration takes no annotations");
    err("@ tunable let X = 1\n", "no space after the `@`");
    err("@(1) let X = 1\n", "`@` must be followed by an annotation name");
    err("@tunable(value: 2) let X = 1\n", "cannot also be named `value:`");
    err("f(@x)\n", "the `@` rebind operator was removed");
}

/// The lowered call meets the diagnostics the call form meets.
#[test]
fn annotations_keep_the_call_forms_diagnostics() {
    let findings = |src: &str| -> Vec<String> {
        let mut env = Env::new();
        env.set_check_only(true);
        match env.load_program(src) {
            Ok(pid) => env.get_program(pid).unwrap().warnings.iter().map(|d| d.message.clone()).collect(),
            Err(e) => vec![e],
        }
    };
    let has = |src: &str, want: &str| {
        let got = findings(src);
        assert!(got.iter().any(|m| m.contains(want)), "{src:?}: wanted {want:?}, got {got:?}");
    };
    has("@const var X = 1\nprint(X)\n", "a `var` is assigned again");
    has("@const @tunable let X = 4\nprint(X)\n", "drop `@const`");
    has("fn f()\n  @const let x = 2\n  return x\nend\nprint(f())\n", "inside a function");
    has("@const let X = random()\nprint(X)\n", "is declared `const` but is not constant");
    has("@const state s = 3\nprint(s)\n", "a `state` slot cannot be constant");
    // A written call as the declared value shares its span with the call the
    // annotation builds: `@const let X = const(4)` is `const(const(4))`, whose
    // inner `const(…)` is not the whole initializer.
    has("@const let X = const(4)\nprint(X)\n", "it must wrap the whole initializer");
    has("@const\nlet X = const([1, 2])\nprint(X)\n", "it must wrap the whole initializer");
}

/// A diagnostic about the call as a whole points at the `@name` written for
/// it, not at the declared value (whose span the built call has); one about
/// an argument points at the argument, as in the call form.
#[test]
fn a_call_level_diagnostic_points_at_the_annotation() {
    let carets = |src: &str| -> Vec<(String, u32, u32, u32)> {
        let mut env = Env::new();
        env.set_check_only(true);
        let pid = env.load_program(src).expect("compiles in check mode");
        env.get_program(pid)
            .unwrap()
            .warnings
            .iter()
            .map(|d| (d.message.clone(), d.span.start.line, d.span.start.column, d.span.end.column))
            .collect()
    };
    let at = |src: &str, want: &str| -> (u32, u32, u32) {
        let got = carets(src);
        let (_, l, c, e) = got
            .iter()
            .find(|(m, ..)| m.contains(want))
            .unwrap_or_else(|| panic!("{src:?}: wanted {want:?}, got {got:?}"));
        (*l, *c, *e)
    };
    assert_eq!(at("@tunable(0, 10, 1, 2, 3) let S = 4.0\nprint(S)\n", "expects 1-5 arguments"), (1, 1, 9));
    assert_eq!(at("@rigid(0) let S = 4.0\nprint(S)\n", "expects 1 argument"), (1, 1, 7));
    assert_eq!(at("@const @tunable let S = 4.0\nprint(S)\n", "drop `@const`"), (1, 1, 7));
    assert_eq!(at("fn f()\n  @const\n  let x = 2\n  x\nend\nprint(f())\n", "inside a function"), (2, 3, 9));
    // The message names the annotation that was written, not the call.
    assert_eq!(at("fn f()\n  @const\n  let x = 2\n  x\nend\nprint(f())\n", "`@const` checks a top-level `let`"), (2, 3, 9));
    assert_eq!(at("@const var v = 2\nprint(v)\n", "Drop `@const`, or use `let`"), (1, 1, 7));
    assert_eq!(at("@const(1) let K = 4\nprint(K)\n", "`@const` takes no arguments"), (1, 1, 7));
    assert_eq!(at("let K = const(4, 1)\nprint(K)\n", "`const(…)` takes one argument"), (1, 9, 20));
    // A named argument no form of the builtin takes, or one given twice, is
    // reported from the IR's call term: that term is blamed at the `@name`
    // too.
    assert_eq!(at("@tunable(bogus: 1) let S = 4.0\nprint(S)\n", "no parameter named 'bogus'"), (1, 1, 9));
    assert_eq!(at("@tunable(min: 0, min: 1) let S = 4.0\nprint(S)\n", "named 'min', 'min'"), (1, 1, 9));
    assert_eq!(at("@tunable(min: 0)\nlet S = 4.0\n@tunable(oops: 2) let T = 1\nprint(S + T)\n", "named 'oops'"), (3, 1, 9));
    // An argument's diagnostic stays on the argument.
    assert_eq!(at("@tunable(\"a\", \"b\") let S = 4.0\nprint(S)\n", "argument 2 to `tunable`"), (1, 10, 13));
    // The call form is unchanged.
    assert_eq!(at("let S = rigid(4.0, 0)\nprint(S)\n", "expects 1 argument"), (1, 9, 22));
}

#[test]
fn a_one_line_setting_with_named_bounds_runs_and_is_listed() {
    let src = "@tunable(min: 3.0, max: 12.0, step: 0.5) pub let RUN = 7.0  // tiles/s\n\
               @tunable let KICK = 6.0\n\
               @tunable(options: [\"title\", \"play\"])\n\
               let START = \"title\"\n\
               @const\n\
               let KINDS = {sedan: {mass: 1200.0}}\n\
               print(RUN, KICK, START, KINDS.sedan.mass)\n";
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).expect("loads");
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).expect("runs");
    assert_eq!(env.take_output().join("\n").trim(), "7.0 6.0 title 1200.0");
    let b = static_bindings(src).unwrap();
    let run = b.iter().find(|b| b.name == "RUN").unwrap();
    assert!(run.is_config && run.exported);
    let range = run.range.expect("a range");
    assert_eq!((range.min, range.max, range.step), (Some(3.0), Some(12.0), Some(0.5)));
    assert_eq!(run.text.as_deref(), Some("7.0"));
    let start = b.iter().find(|b| b.name == "START").unwrap();
    assert!(start.is_config);
    assert_eq!(start.options.as_ref().map(Vec::len), Some(2));
    let kinds = b.iter().find(|b| b.name == "KINDS").unwrap();
    assert!(!kinds.is_config);
    // A value set by name rewrites the literal after the `=`.
    let set = petal::literal_edit::set_path(src, "RUN", &StaticValue::float(8.5)).unwrap();
    assert!(set.starts_with("@tunable(min: 3.0, max: 12.0, step: 0.5) pub let RUN = 8.5  // tiles/s\n"), "{set}");
    let set = petal::literal_edit::set_path(src, "START", &StaticValue::Str("play".into())).unwrap();
    assert!(set.contains("let START = \"play\"\n"), "{set}");
}

#[test]
fn a_value_edit_in_the_at_form_is_a_literal_patch() {
    let old = "@tunable(0, 10) let S = 4.0\nprint(S)\n";
    let new = "@tunable(0, 10) let S = 5.5\nprint(S)\n";
    assert_eq!(petal::source_diff::diff_source(old, new, FileId(0)).change.label(), "values");
    // Adding or removing an annotation recompiles, as adding the call does.
    let plain = "let S = 4.0\nprint(S)\n";
    assert_eq!(
        reload_kinds(plain, old),
        reload_kinds(plain, "let S = tunable(4.0, 0, 10)\nprint(S)\n")
    );
}
