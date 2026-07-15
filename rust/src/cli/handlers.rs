//! Per-subcommand handlers extracted from the `execute()` dispatch, plus the
//! shared front-end helpers (env construction, source compilation, term
//! resolution, and graph/term rendering) they build on.

use std::fs;
use std::path::PathBuf;
use std::process;

use crate::backend::OptFlags;
use crate::dot_graph::program_to_dot;
use crate::env::Env;
use crate::ir_display::display_program_with;
use crate::lexer::Lexer;
use crate::program::{Program, ProgramId, Term, TermId};
use crate::source_map::ENTRY_FILE;

use super::{SourceInput, die, die_plain};

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_run(
    json: bool,
    trace: bool,
    record_trace: Option<String>,
    ir: bool,
    dup_stats: bool,
    no_opt: bool,
    trace_pending: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    if trace || std::env::var("PETAL_DEBUG").is_ok() {
        unsafe {
            std::env::set_var("PETAL_TRACE", "1");
        }
    }
    // `--trace-pending` (or PETAL_TRACE_PENDING=1) turns on the absorption log
    // and prints the frame pending report after the run.
    let trace_pending = trace_pending || std::env::var("PETAL_TRACE_PENDING").is_ok();
    let mut env = make_env(include_dirs);
    if no_opt {
        env.set_opt_flags(OptFlags::none());
    }
    if record_trace.is_some() {
        env.trace_mut().enable();
    }
    let load_result = if ir {
        env.load_program_ir(source)
    } else {
        load_into(&mut env, source, source_input)
    };
    let pid = match load_result {
        Ok(pid) => pid,
        Err(e) => die(json, &e, classify_load_error(&e)),
    };
    // Surface type-checker warnings on stderr before running. Warnings go to
    // stderr even in --json mode, so JSON consumers of stdout are unaffected.
    if let Some(program) = env.get_program(pid) {
        eprint_warnings(program);
    }
    let sid = match env.create_stack(pid) {
        Ok(sid) => sid,
        Err(e) => die(json, &e, "compile"),
    };
    if trace_pending {
        env.enable_pending_trace(sid);
    }
    let run_result = env.run(sid);

    if let Some(path) = &record_trace {
        write_trace_to_file(&env, pid, path);
    }

    if dup_stats {
        eprintln!("{}", env.dup_stats());
        eprintln!("{}", env.alloc_stats());
    }

    if trace_pending {
        let report = env.pending_report(pid, sid);
        eprintln!(
            "pending report: {}",
            serde_json::to_string_pretty(&report).unwrap()
        );
    }

    if let Err(e) = run_result {
        die(json, &e, "runtime");
    }
}

/// Run the program and print the frame pending report — the JSON array of every
/// live pending resource (`{ id, key, state, age_frames, origin,
/// absorbed_count }`). This is what the MCP `PendingReport` tool shells out to
/// and what an agent debugging "why is this region blank" reads. `--json` emits
/// the raw report array; otherwise a short human-readable listing is printed.
pub(super) fn handle_pending_report(
    json: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let mut env = make_env(include_dirs);
    let pid = match load_into(&mut env, source, source_input) {
        Ok(pid) => pid,
        Err(e) => die(json, &e, classify_load_error(&e)),
    };
    let sid = match env.create_stack(pid) {
        Ok(sid) => sid,
        Err(e) => die(json, &e, "compile"),
    };
    // Record absorptions too, so a caller inspecting the report sees per-frame
    // absorption counts populated.
    env.enable_pending_trace(sid);
    let run_result = env.run(sid);

    let report = env.pending_report(pid, sid);
    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        print_pending_report_text(&report);
    }

    if let Err(e) = run_result {
        die(json, &e, "runtime");
    }
}

/// Render the pending report as a short human-readable listing (the non-`--json`
/// output of `pending-report`): one line per live resource with its state, age,
/// absorption count, and origin call site.
fn print_pending_report_text(report: &serde_json::Value) {
    let entries = report.as_array().map(Vec::as_slice).unwrap_or(&[]);
    if entries.is_empty() {
        println!("No pending resources.");
        return;
    }
    println!("Pending resources ({}):", entries.len());
    for entry in entries {
        let state = entry.get("state").and_then(|s| s.as_str()).unwrap_or("?");
        let age = entry
            .get("age_frames")
            .and_then(|a| a.as_u64())
            .unwrap_or(0);
        let absorbed = entry
            .get("absorbed_count")
            .and_then(|a| a.as_u64())
            .unwrap_or(0);
        let origin = entry
            .get("origin")
            .and_then(|o| o.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("<unknown origin>");
        println!("  {state} {age}f  absorbed {absorbed}x  {origin}");
    }
}

pub(super) fn handle_explain(
    json: bool,
    term_query: String,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let mut env = make_env(include_dirs);
    env.trace_mut().enable();
    let pid = match load_into(&mut env, source, source_input) {
        Ok(pid) => pid,
        Err(e) => die_plain(&e),
    };
    let sid = env.create_stack(pid).unwrap_or_else(|e| die_plain(&e));
    // Run to completion (ignore errors — we still want the partial trace)
    let _ = env.run(sid);

    let program = env.get_program(pid).expect("program");
    let target_id = match program.find_term(&term_query) {
        Some(id) => id,
        None => term_not_found(program, &term_query),
    };

    let entries = env.trace().explain(program, env.heap(), target_id, 16);

    // Pretty header — use the resolved term name if available so an
    // `--term 72` query still shows `(total)` instead of `(72)`.
    let header_name = program.get_term(target_id).name.clone().unwrap_or_else(|| {
        if term_query.parse::<u32>().is_ok() || term_query.starts_with('t') {
            "unnamed".to_string()
        } else {
            term_query.clone()
        }
    });

    if json {
        let entries_json: Vec<_> = entries.iter().map(|e| e.to_json()).collect();
        let out = serde_json::json!({
            "term_id": target_id.0,
            "name": header_name,
            "chain": entries_json,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("Explain t{} ({}):", target_id.0, header_name);
        println!("  Provenance chain:");
        for (i, e) in entries.iter().enumerate() {
            let loc = match (e.line, e.column) {
                (Some(l), Some(c)) => format!("[line {}, column {}]", l, c),
                _ => "[no location]".to_string(),
            };
            let name = e.name.as_deref().unwrap_or("-");
            let value = e.value.as_deref().unwrap_or("<not executed>");
            let arrow = if i == 0 { "=>" } else { " ." };
            println!(
                "    {} t{} {} {} = {}",
                arrow, e.term_id.0, name, loc, value
            );
        }
    }
}

/// Format the `[line N, column M]` (or `[file line N, column M]`) position tag
/// for a warning's span — mirrors `backend::errors::format_position`.
fn warning_position(program: &Program, span: &crate::source_map::SourceSpan) -> String {
    match program.source_map.file_name_for_span(span) {
        Some(file) => format!(
            "[{} line {}, column {}]",
            file, span.start.line, span.start.column
        ),
        None => format!("[line {}, column {}]", span.start.line, span.start.column),
    }
}

/// Render a program's type-checker warnings as human-readable text (for
/// stderr). Each diagnostic becomes a `warning:` line, a ` --> <position>`
/// line, and (when a real span + source exist) a caret snippet.
fn render_warnings_text(program: &Program) -> String {
    let mut out = String::new();
    for d in &program.warnings {
        out.push_str(&format!("warning: {}\n", d.message));
        out.push_str(&format!(" --> {}\n", warning_position(program, &d.span)));
        let src = program
            .source_map
            .source_for_span(&d.span)
            .unwrap_or(&program.source);
        if let Some(snippet) =
            crate::backend::errors::format_source_snippet(src, &d.span)
        {
            out.push_str(&snippet);
            out.push('\n');
        }
    }
    out
}

/// Print a program's type-checker warnings to stderr (nothing when there are
/// none). Used before running and by `check`; stderr keeps them off the stdout
/// JSON channel.
fn eprint_warnings(program: &Program) {
    let text = render_warnings_text(program);
    if !text.is_empty() {
        eprint!("{}", text);
    }
}

/// Build the JSON array of a program's warnings: one object per diagnostic with
/// `message`, `line`, `column`, and `file` (null for the entry file).
fn warnings_json(program: &Program) -> serde_json::Value {
    let items: Vec<serde_json::Value> = program
        .warnings
        .iter()
        .map(|d| {
            let file = program.source_map.file_name_for_span(&d.span);
            serde_json::json!({
                "message": d.message,
                "line": d.span.start.line,
                "column": d.span.start.column,
                "file": file,
            })
        })
        .collect();
    serde_json::Value::Array(items)
}

pub(super) fn handle_check(
    json: bool,
    strict: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let mut env = make_env(include_dirs);
    let is_empty = source.trim().is_empty();
    match load_into(&mut env, source, source_input) {
        Ok(pid) => {
            let program = env.get_program(pid);
            let warning_count = program.map_or(0, |p| p.warnings.len());
            if json {
                let warnings = program
                    .map(warnings_json)
                    .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
                let mut obj = serde_json::json!({ "ok": true, "warnings": warnings });
                if is_empty {
                    obj["warning"] = serde_json::json!("empty program");
                }
                println!("{}", obj);
            } else {
                if let Some(program) = program {
                    eprint_warnings(program);
                }
                if is_empty {
                    eprintln!("warning: empty program");
                }
                // Otherwise silent on success, like most linters
            }
            // `--strict` turns warnings into a non-zero exit (for CI); plain
            // `check` always succeeds. Output above is unchanged either way.
            if strict && warning_count > 0 {
                process::exit(1);
            }
        }
        Err(e) => die(json, &e, classify_load_error(&e)),
    }
}

pub(super) fn handle_lint(
    fix: bool,
    check: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let opts = crate::lint::LintOptions {
        include_dirs: include_dirs.to_vec(),
        origin: source_origin(source_input),
    };
    let outcome = match crate::lint::lint_source(source, &opts) {
        Ok(o) => o,
        Err(e) => die_plain(&e),
    };
    for note in &outcome.notes {
        eprintln!("lint: {}", note);
    }
    let changed = outcome.changed(source);

    // Inline code always prints the normalized result to stdout.
    if let SourceInput::Inline(_) = source_input {
        print!("{}", outcome.output);
        return;
    }
    let SourceInput::File(path) = source_input else {
        unreachable!()
    };
    let summary = format!(
        "{}: {} line(s) reformatted, {} rebind rewrite(s)",
        path, outcome.reindented_lines, outcome.rebinds
    );
    if check {
        // CI mode: no output on success, one stderr line on failure.
        if changed {
            eprintln!("would fix {}", summary);
            process::exit(1);
        }
    } else if fix {
        if changed {
            if let Err(e) = fs::write(path, &outcome.output) {
                eprintln!("Error writing '{}': {}", path, e);
                process::exit(1);
            }
            println!("fixed {}", summary);
        }
    } else if changed {
        println!("would fix {} (run with --fix to apply)", summary);
        process::exit(1);
    }
}

pub(super) fn handle_show_tokens(json: bool, source: &str) {
    let mut lexer = Lexer::new(source);
    match lexer.tokenize() {
        Ok(_) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&lexer.tokens).unwrap());
            } else {
                for (i, token) in lexer.tokens.iter().enumerate() {
                    println!("{}: {:?}", i, token);
                }
            }
        }
        Err(e) => die_plain(&e),
    }
}

pub(super) fn handle_show_ast(json: bool, source: &str) {
    match crate::cst::parse_source(source, ENTRY_FILE) {
        Ok((_tree, stmts)) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&stmts).unwrap());
            } else {
                for stmt in &stmts {
                    println!("{:#?}", stmt);
                }
            }
        }
        Err(e) => die_plain(&e),
    }
}

pub(super) fn handle_show_ir(
    json: bool,
    all: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let program = compile_source(source, source_input, include_dirs);
    if json {
        println!("{}", serde_json::to_string_pretty(&program).unwrap());
    } else {
        print!("{}", display_program_with(&program, !all));
    }
}

pub(super) fn handle_show_bytecode(
    json: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    use crate::backend::bytecode::{
        InPlaceSet, analyze_escapes, apply_last_use, disasm, lower_program_opt,
    };
    let program = compile_source(source, source_input, include_dirs);
    // Mirror the runtime defaults: the disassembly shows the in-place
    // opcodes a run would actually execute, for both M4 routes.
    // `PETAL_OPT=off`/`none` shows the clone-and-alloc lowering;
    // `PETAL_OPT=all` enables every opt.
    let flags = crate::env::Env::opt_flags_from_env();
    let in_place = if flags.in_place_mutation {
        analyze_escapes(&program)
    } else {
        InPlaceSet::default()
    };
    match lower_program_opt(&program, &in_place) {
        Ok(mut bc) => {
            if flags.in_place_straight_line {
                apply_last_use(&mut bc, &program);
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&disasm::render_json(&bc, &program)).unwrap()
                );
            } else {
                print!("{}", disasm::render_text(&bc, &program));
            }
        }
        Err(e) => {
            eprintln!("Error lowering to bytecode: {}", e);
            process::exit(1);
        }
    }
}

pub(super) fn handle_show_provenance(
    json: bool,
    term_query: String,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let program = compile_source(source, source_input, include_dirs);

    let root_id = resolve_terms(&program, std::slice::from_ref(&term_query))[0];

    let root_term = program.get_term(root_id);
    let (ancestor_ids, edges) = program.trace_provenance(root_id);

    if json {
        let root_json = term_to_json(root_term);
        let ancestors_json: Vec<_> = ancestor_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        let edges_json = edges_to_json(&edges);
        let output = serde_json::json!({
            "root": root_json,
            "ancestors": ancestors_json,
            "edges": edges_json,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        println!(
            "Provenance of t{} ({}):",
            root_id.0,
            root_term.name.as_deref().unwrap_or("unnamed")
        );
        println!("  op: {:?}", root_term.op);
        println!(
            "  inputs: {:?}",
            root_term.inputs.iter().map(|i| i.0).collect::<Vec<_>>()
        );
        println!();
        println!("Ancestors ({}):", ancestor_ids.len());
        print_term_rows(&program, &ancestor_ids);
        println!();
        println!("Edges ({}):", edges.len());
        for (from, to) in &edges {
            println!("  t{} -> t{}", from.0, to.0);
        }
    }
}

pub(super) fn handle_show_dependents(
    json: bool,
    term_query: String,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let program = compile_source(source, source_input, include_dirs);

    let root_id = resolve_terms(&program, std::slice::from_ref(&term_query))[0];

    let root_term = program.get_term(root_id);
    let (dependent_ids, edges) = program.trace_dependents(root_id);

    if json {
        let root_json = term_to_json(root_term);
        let dependents_json: Vec<_> = dependent_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        let edges_json = edges_to_json(&edges);
        let output = serde_json::json!({
            "root": root_json,
            "dependents": dependents_json,
            "edges": edges_json,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        println!(
            "Dependents of t{} ({}):",
            root_id.0,
            root_term.name.as_deref().unwrap_or("unnamed")
        );
        println!("  op: {:?}", root_term.op);
        println!();
        println!("Downstream ({}):", dependent_ids.len());
        print_term_rows(&program, &dependent_ids);
        println!();
        println!("Edges ({}):", edges.len());
        for (from, to) in &edges {
            println!("  t{} -> t{}", from.0, to.0);
        }
    }
}

pub(super) fn handle_show_slice(
    json: bool,
    term_queries: Vec<String>,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let program = compile_source(source, source_input, include_dirs);

    let target_ids = resolve_terms(&program, &term_queries);

    let slice_ids = program.slice(&target_ids);

    if json {
        let terms_json: Vec<_> = slice_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        let output = serde_json::json!({
            "targets": target_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
            "slice": terms_json,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        println!(
            "Slice for targets: {}",
            target_ids
                .iter()
                .map(|id| format!("t{}", id.0))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!();
        println!("Terms ({}):", slice_ids.len());
        print_term_rows(&program, &slice_ids);
    }
}

pub(super) fn handle_show_graph(
    all: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let program = compile_source(source, source_input, include_dirs);
    println!("{}", program_to_dot(&program, !all));
}

// --- shared front-end helpers -------------------------------------------

/// The filesystem path a source input was read from, if any — the anchor for
/// resolving that file's imports relative to its own directory.
fn source_origin(input: &SourceInput) -> Option<PathBuf> {
    match input {
        SourceInput::File(path) if path != "-" => Some(PathBuf::from(path)),
        _ => None,
    }
}

/// Build an Env configured with the CLI's `-I` module search paths.
fn make_env(include_dirs: &[PathBuf]) -> Env {
    let mut env = Env::new();
    for dir in include_dirs {
        env.add_module_path(dir.clone());
    }
    env
}

/// Run the full front end (module resolution included). Returns the compiled
/// Program.
fn compile_source(
    source: &str,
    input: &SourceInput,
    include_dirs: &[PathBuf],
) -> crate::program::Program {
    let env = make_env(include_dirs);
    let result = match source_origin(input) {
        Some(path) => env.compile_program_at(ProgramId(0), source, &path),
        None => env.compile_program(ProgramId(0), source),
    };
    match result {
        Ok(program) => program,
        Err(e) => die_plain(&e),
    }
}

/// Load `source` into `env`, resolving imports relative to the input's path
/// when it has one.
fn load_into(env: &mut Env, source: &str, input: &SourceInput) -> Result<ProgramId, String> {
    match source_origin(input) {
        Some(path) => env.load_program_at(source, &path),
        None => env.load_program(source),
    }
}

/// Classify a load_program error into "lex" or "parse" based on the message
/// shape. The lexer's error messages mention specific characters; parser
/// errors mention tokens and grammar expectations.
fn classify_load_error(e: &str) -> &'static str {
    if e.contains("Unexpected character")
        || e.contains("Unterminated")
        || e.contains("braced expression")
    {
        "lex"
    } else {
        "parse"
    }
}

/// Print a "not found" error for a `--term` lookup with a did-you-mean hint
/// listing up to 10 available named terms, then exit.
fn term_not_found(program: &Program, query: &str) -> ! {
    eprintln!("Term '{}' not found", query);
    let names = program.named_terms();
    if !names.is_empty() {
        let shown: Vec<_> = names.iter().take(10).cloned().collect();
        let suffix = if names.len() > 10 {
            format!(", ... ({} more)", names.len() - 10)
        } else {
            String::new()
        };
        eprintln!("Available named terms: {}{}", shown.join(", "), suffix);
    }
    process::exit(1);
}

/// Resolve `--term` name/id queries to term ids, exiting with a
/// `term_not_found` hint on the first query that does not resolve.
fn resolve_terms(program: &Program, queries: &[String]) -> Vec<TermId> {
    let mut ids = Vec::new();
    for query in queries {
        match program.find_term(query) {
            Some(id) => ids.push(id),
            None => term_not_found(program, query),
        }
    }
    ids
}

/// Render dataflow graph edges to the `[{ "from", "to" }]` JSON shape shared by
/// the provenance and dependents outputs.
fn edges_to_json(edges: &[(TermId, TermId)]) -> Vec<serde_json::Value> {
    edges
        .iter()
        .map(|(from, to)| serde_json::json!({ "from": from.0, "to": to.0 }))
        .collect()
}

/// Print the `  t{id}: {op} {name}` rows shared by the provenance, dependents,
/// and slice text outputs.
fn print_term_rows(program: &Program, ids: &[TermId]) {
    for &id in ids {
        let t = program.get_term(id);
        println!(
            "  t{}: {:?} {}",
            t.id.0,
            t.op,
            t.name.as_deref().unwrap_or("")
        );
    }
}

/// Write the Env's trace buffer to `path` as pretty-printed JSON.
fn write_trace_to_file(env: &Env, pid: ProgramId, path: &str) {
    let Some(program) = env.get_program(pid) else {
        eprintln!("write_trace: program {} not found", pid.0);
        return;
    };
    let json = env.trace().to_json(program, env.heap());
    match serde_json::to_string_pretty(&json) {
        Ok(s) => {
            if let Err(e) = fs::write(path, s) {
                eprintln!("Failed to write trace to {}: {}", path, e);
            }
        }
        Err(e) => eprintln!("Failed to serialize trace: {}", e),
    }
}

fn term_to_json(term: &Term) -> serde_json::Value {
    // Simplified term representation for provenance output
    let op = serde_json::to_value(&term.op).unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "id": term.id.0,
        "op": op,
        "name": term.name,
        "inputs": term.inputs.iter().map(|i| i.0).collect::<Vec<_>>(),
    })
}
