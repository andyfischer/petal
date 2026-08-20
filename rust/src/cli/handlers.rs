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
use crate::program_analysis::EdgeKind;
use crate::source_map::ENTRY_FILE;

use super::{SourceInput, die, die_error, die_plain, die_with, error_json_value};

/// `petal lsp` — serve the language server on stdin/stdout until the client
/// disconnects. A broken pipe is how an editor normally shuts us down, so that
/// exits quietly; anything else is a real I/O failure worth reporting.
pub(super) fn handle_lsp() {
    if let Err(e) = crate::lsp::stdio::serve()
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        die_plain(&format!("lsp: {}", e));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_run(
    json: bool,
    trace: bool,
    record_trace: Option<String>,
    ir: bool,
    dup_stats: bool,
    profile: bool,
    no_opt: bool,
    trace_pending: bool,
    observe: bool,
    trace_emits: bool,
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
    // Enable before the run, not after: observation records writes as they
    // happen, so a buffer switched on afterwards has nothing in it.
    if observe {
        env.observations_mut().enable();
    }
    // Same rule for emit attribution — recording happens at the emit.
    if trace_emits {
        env.enable_emit_trace(true);
    }
    if profile {
        env.profile_mut().set_enabled(true);
    }
    let pid = if ir {
        // The IR loader is a deserializer, not the front end; it has no phase
        // of its own, and reported "parse" before the phase channel existed.
        match env.load_program_ir(source) {
            Ok(pid) => pid,
            Err(e) => die(json, &e, "parse"),
        }
    } else {
        match load_into(&mut env, source, source_input) {
            Ok(pid) => pid,
            Err(e) => die_error(json, &e, serde_json::Value::Null, source),
        }
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
    let run_started = std::time::Instant::now();
    let run_result = env.run(sid);
    let run_elapsed = run_started.elapsed();

    // Snapshot the observed values now. The map is a snapshot by contract, and
    // reading it here — before anything else touches the env — keeps the
    // reported values the ones the run finished (or died) with.
    let observed = observe.then(|| env.get_observations_json(pid, sid));

    if let Some(path) = &record_trace {
        write_trace_to_file(&env, pid, path);
    }

    if profile {
        // Names are resolved here rather than in `VmProfile` because the native
        // table is the `Env`'s, not the profile's.
        let report = env
            .profile()
            .report(Some(run_elapsed), |nid| env.native_fn_name(nid), 15);
        eprint!("{report}");
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

    if trace_emits {
        let report = emit_trace_report(&env, pid);
        if json {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        } else {
            print_emit_trace_text(&report);
        }
    }

    // The dump comes before the error report, in both modes and for the same
    // reason: the values are what the run *did*, the error is how it ended.
    // Reading them in that order is reading the program's story forward.
    match (run_result, observed) {
        (Err(e), Some(map)) if json => {
            // One JSON document on stdout, not two: the observed values ride on
            // the error object rather than being printed beside it.
            let mut obj = error_json_value(&e, "runtime");
            obj["observations"] = serde_json::Value::Object(map);
            println!("{}", serde_json::to_string_pretty(&obj).unwrap());
            process::exit(1);
        }
        (run_result, observed) => {
            if let Some(map) = observed {
                print_observations(json, &map);
            }
            if let Err(e) = run_result {
                die(json, &e, "runtime");
            }
        }
    }
}

/// Print the `--observe` dump: a JSON object under `--json`, otherwise a blank
/// line, a header, and one aligned `name = value` line per observed binding.
///
/// The blank line and header matter — the dump shares stdout with whatever the
/// program itself printed, and an unheralded list of assignments would read as
/// more program output. Names are sorted so two runs of the same program diff
/// cleanly; values are compact JSON, so a string is quoted and cannot be
/// mistaken for a bare name.
fn print_observations(json: bool, map: &serde_json::Map<String, serde_json::Value>) {
    if json {
        println!("{}", serde_json::to_string_pretty(map).unwrap());
        return;
    }
    println!();
    if map.is_empty() {
        println!("Observed values: none.");
        return;
    }
    let mut entries: Vec<(&String, String)> = map
        .iter()
        .map(|(k, v)| (k, serde_json::to_string(v).unwrap_or_default()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let width = entries.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    println!("Observed values ({}):", entries.len());
    for (name, value) in entries {
        println!("  {:<width$} = {}", name, value, width = width);
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
        Err(e) => die_error(json, &e, serde_json::Value::Null, source),
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

/// Build the `--trace-emits` report: for every channel the run emitted into,
/// each value with its resolved attribution — the frame `pick_frame` chose,
/// the callee name, the call span, and per-argument edit info. This is the
/// "observe" half of the direct-manipulation protocol
/// (docs/direct-manipulation.md); `propose-edit` is the "act" half, and the
/// `emit` indices in this report are what it addresses.
fn emit_trace_report(env: &Env, pid: ProgramId) -> serde_json::Value {
    use crate::provenance::{self, CallSite};

    let Some(program) = env.get_program(pid) else {
        return serde_json::json!({ "channels": {} });
    };
    let mut channels = serde_json::Map::new();
    for sym in env.output_channels() {
        let name = env.symbol_name(sym).unwrap_or("<unnamed>").to_string();
        let values = env.output_buffer(sym);
        let origins = env.output_origins(sym);
        let emits: Vec<serde_json::Value> = values
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let mut entry = serde_json::json!({
                    "index": i,
                    "value": crate::value::value_to_json(value, env.heap()),
                });
                // Origins are index-aligned with values; a run with tracing
                // off (or an emit with nothing to attribute) reports the value
                // alone, which is a legitimate answer.
                let site = origins
                    .get(i)
                    .and_then(|o| provenance::pick_frame(program, &o.chain, ENTRY_FILE))
                    .and_then(|term| CallSite::resolve(program, term));
                if let Some(site) = site {
                    entry["term"] = serde_json::json!(site.term.0);
                    entry["callee"] = serde_json::json!(site.callee);
                    entry["span"] = span_json(&site.span);
                    entry["args"] = serde_json::Value::Array(
                        site.args
                            .iter()
                            .map(|a| arg_site_json(program, a))
                            .collect(),
                    );
                }
                entry
            })
            .collect();
        channels.insert(name, serde_json::Value::Array(emits));
    }
    serde_json::json!({ "channels": channels })
}

/// One argument of a resolved call, as the report's JSON: where it is written,
/// how editable it is, and where an edit would land.
fn arg_site_json(program: &Program, arg: &crate::provenance::ArgSite) -> serde_json::Value {
    use crate::provenance::ArgKind;
    serde_json::json!({
        "index": arg.index,
        "kind": match arg.kind {
            ArgKind::Literal => "literal",
            ArgKind::Binding => "binding",
            ArgKind::Computed => "computed",
        },
        "value": arg.value.as_ref().map(static_value_json),
        "span": span_json(&arg.span),
        "editable_span": span_json(&arg.editable_span(program)),
    })
}

/// A `SourceSpan` as JSON (`null` for an unmapped one): 1-based line/column
/// plus char offsets, both ends.
fn span_json(span: &Option<crate::source_map::SourceSpan>) -> serde_json::Value {
    match span {
        Some(s) => serde_json::json!({
            "start": { "line": s.start.line, "column": s.start.column, "offset": s.start.offset },
            "end": { "line": s.end.line, "column": s.end.column, "offset": s.end.offset },
        }),
        None => serde_json::Value::Null,
    }
}

/// A scalar `StaticValue` as a JSON value; composites render as source text.
fn static_value_json(v: &crate::static_value::StaticValue) -> serde_json::Value {
    use crate::static_value::StaticValue;
    match v {
        StaticValue::Str(s) => serde_json::json!(s),
        StaticValue::Int(n) => serde_json::json!(n),
        StaticValue::Float(f) => serde_json::json!(f),
        StaticValue::Bool(b) => serde_json::json!(b),
        StaticValue::Nil => serde_json::Value::Null,
        other => serde_json::json!(other.to_source()),
    }
}

/// Human-readable `--trace-emits` output: a header per channel, one line per
/// emit with its callee and line, and an indented line per editable argument.
fn print_emit_trace_text(report: &serde_json::Value) {
    let empty = serde_json::Map::new();
    let channels = report
        .get("channels")
        .and_then(|c| c.as_object())
        .unwrap_or(&empty);
    println!();
    if channels.is_empty() {
        println!("Emitted values: none.");
        return;
    }
    for (name, emits) in channels {
        let emits = emits.as_array().map(Vec::as_slice).unwrap_or(&[]);
        println!(
            "Channel '{}' ({} emit{}):",
            name,
            emits.len(),
            if emits.len() == 1 { "" } else { "s" }
        );
        for e in emits {
            let idx = e.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let callee = e
                .get("callee")
                .and_then(|v| v.as_str())
                .unwrap_or("<unattributed>");
            let line = e
                .pointer("/span/start/line")
                .and_then(|v| v.as_u64())
                .map(|l| format!(" [line {}]", l))
                .unwrap_or_default();
            let value = e.get("value").map(|v| v.to_string()).unwrap_or_default();
            println!("  [{}] {}{} <- {}", idx, callee, line, value);
            for a in e
                .get("args")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
            {
                let ai = a.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                let av = a.get("value").filter(|v| !v.is_null());
                let at = a
                    .pointer("/editable_span/start/line")
                    .and_then(|v| v.as_u64())
                    .map(|l| format!(" (edit line {})", l))
                    .unwrap_or_default();
                match av {
                    Some(v) => println!("      arg {}: {} = {}{}", ai, kind, v, at),
                    None => println!("      arg {}: {}", ai, kind),
                }
            }
        }
    }
}

/// `petal propose-edit` — the goal half of direct manipulation: run the
/// program with emit tracing, pick the call that produced the addressed emit,
/// and propose source edits that make each addressed argument evaluate to its
/// requested value. Several `--arg`/`--to` pairs form a batch resolved
/// consistently. See docs/direct-manipulation.md for the protocol.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_propose_edit(
    json: bool,
    channel: &str,
    emit: usize,
    goals: &[(usize, String)],
    configurable: &[String],
    pinned: &[String],
    apply: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    use crate::direct_manipulation::{
        ManipulationGoal, VarPolicy, apply_edits, propose_edits_batch,
    };
    use crate::provenance;

    let mut env = make_env(include_dirs);
    env.enable_emit_trace(true);
    // The per-term trace supplies the values the arithmetic solver inverts
    // against; without it only statically-known siblings can be used.
    env.trace_mut().enable();
    let pid = match load_into(&mut env, source, source_input) {
        Ok(pid) => pid,
        Err(e) => die_error(json, &e, serde_json::Value::Null, source),
    };
    let sid = match env.create_stack(pid) {
        Ok(sid) => sid,
        Err(e) => die(json, &e, "compile"),
    };
    if let Err(e) = env.run(sid) {
        die(json, &e, "runtime");
    }

    let sym = env.intern_symbol(channel);
    let origins = env.output_origins(sym);
    if origins.is_empty() {
        let known: Vec<String> = env
            .output_channels()
            .iter()
            .filter_map(|&s| env.symbol_name(s).map(str::to_string))
            .collect();
        die(
            json,
            &format!(
                "channel '{}' recorded no emits; channels with emits: {}",
                channel,
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ),
            "goal",
        );
    }
    let Some(origin) = origins.get(emit) else {
        die(
            json,
            &format!(
                "channel '{}' has {} emit(s), no index {}",
                channel,
                origins.len(),
                emit
            ),
            "goal",
        );
    };

    let program = env.get_program(pid).expect("program");
    let Some(term) = provenance::pick_frame(program, &origin.chain, ENTRY_FILE) else {
        die(json, "the emit carries no attributable call chain", "goal");
    };

    let manipulation_goals: Vec<ManipulationGoal> = goals
        .iter()
        .map(|(arg_index, to)| ManipulationGoal {
            term,
            arg_index: *arg_index,
            new_value: parse_goal_value(to),
        })
        .collect();
    let mut policy = std::collections::HashMap::new();
    for name in pinned {
        policy.insert(name.clone(), VarPolicy::Static);
    }
    // Configurable wins on conflict: naming a variable both ways means the
    // caller most recently decided to tune it.
    for name in configurable {
        policy.insert(name.clone(), VarPolicy::Configurable);
    }

    let per_goal =
        match propose_edits_batch(program, &manipulation_goals, Some(env.trace()), &policy) {
            Ok(ps) => ps,
            Err(e) => die(json, &e.message, "goal"),
        };

    let applied = if apply {
        // Every goal has to be narrowed to exactly one proposal — ambiguity
        // is the caller's to resolve, and a refused goal has nothing to write.
        let mut chosen = Vec::new();
        for ((arg_index, _), ps) in goals.iter().zip(&per_goal) {
            match &ps[..] {
                [p] => chosen.push(p.edit.clone()),
                [] => die(
                    json,
                    &format!("--arg {} has no proposal to apply", arg_index),
                    "apply",
                ),
                _ => die(
                    json,
                    &format!(
                        "--arg {} still has {} proposals; narrow with --configurable / --static before --apply",
                        arg_index,
                        ps.len()
                    ),
                    "apply",
                ),
            }
        }
        let SourceInput::File(path) = source_input else {
            die(json, "--apply needs a file path, not inline code", "apply");
        };
        if path == "-" {
            die(json, "--apply needs a file path, not inline code", "apply");
        }
        let edited = match apply_edits(source, &chosen) {
            Ok(s) => s,
            Err(e) => die(json, &e.message, "apply"),
        };
        if let Err(e) = fs::write(path, &edited) {
            die(json, &format!("writing '{}': {}", path, e), "apply");
        }
        true
    } else {
        false
    };

    let proposals_json =
        |ps: &[crate::direct_manipulation::EditProposal]| -> Vec<serde_json::Value> {
            ps.iter()
                .map(|p| {
                    serde_json::json!({
                        "description": p.description,
                        "variable": p.variable,
                        "shared": p.shared,
                        "config": p.config,
                        "span": span_json(&Some(p.edit.span)),
                        "new_text": p.edit.new_text,
                    })
                })
                .collect()
        };

    if json {
        let goals_json: Vec<serde_json::Value> = goals
            .iter()
            .zip(&per_goal)
            .map(|((arg_index, to), ps)| {
                serde_json::json!({
                    "arg": arg_index,
                    "goal": to,
                    "proposals": proposals_json(ps),
                })
            })
            .collect();
        let mut out = serde_json::json!({
            "channel": channel,
            "emit": emit,
            "goals": goals_json,
            "applied": applied,
        });
        // A single goal also reports through the original flat keys, so
        // existing harnesses keep parsing.
        if let ([(arg_index, to)], [ps]) = (goals, &per_goal[..]) {
            out["arg"] = serde_json::json!(arg_index);
            out["goal"] = serde_json::json!(to);
            out["proposals"] = serde_json::Value::Array(proposals_json(ps));
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return;
    }

    let multi = goals.len() > 1;
    let mut any_ambiguous = false;
    for ((arg_index, to), ps) in goals.iter().zip(&per_goal) {
        if multi {
            println!("goal: arg {} -> {}", arg_index, to);
        }
        let indent = if multi { "  " } else { "" };
        if ps.is_empty() {
            println!(
                "{indent}No edit can satisfy this goal: the argument does not trace to editable text."
            );
            continue;
        }
        println!(
            "{indent}{} proposal{}:",
            ps.len(),
            if ps.len() == 1 { "" } else { "s" }
        );
        for (i, p) in ps.iter().enumerate() {
            let shared = if p.shared {
                "  [shared: other code reads this]"
            } else {
                ""
            };
            println!("{indent}  {}. {}{}", i + 1, p.description, shared);
        }
        any_ambiguous |= ps.len() > 1;
    }
    if applied {
        println!("Applied.");
    } else if any_ambiguous {
        println!("Narrow with --configurable <var> / --static <var>, or apply one by hand.");
    }
}

/// Parse the `--to` goal value the way a config file would read it: int, then
/// float, then `true`/`false`/`nil`, else a string.
fn parse_goal_value(to: &str) -> crate::static_value::StaticValue {
    use crate::static_value::StaticValue;
    if let Ok(n) = to.parse::<i64>() {
        return StaticValue::Int(n);
    }
    if let Ok(f) = to.parse::<f64>() {
        return StaticValue::Float(f);
    }
    match to {
        "true" => StaticValue::Bool(true),
        "false" => StaticValue::Bool(false),
        "nil" => StaticValue::Nil,
        s => StaticValue::Str(s.to_string()),
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
        Err(e) => die_plain(&e.to_string()),
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
        let entries_json: Vec<_> = entries.entries.iter().map(|e| e.to_json()).collect();
        let out = serde_json::json!({
            "term_id": target_id.0,
            "name": header_name,
            "chain": entries_json,
            "complete": entries.complete,
            "truncated": entries.truncated,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("Explain t{} ({}):", target_id.0, header_name);
        println!("  Provenance chain:");
        for (i, e) in entries.entries.iter().enumerate() {
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
            // The boundary is the whole point of stopping — an entry that
            // just ends reads as a chain that finished (§6e).
            if let Some(b) = &e.boundary {
                println!("       ^ {}", b.summary());
                if !b.writes.is_empty() {
                    println!("         writes to '{}':", b.var.as_deref().unwrap_or("?"));
                    for (n, w) in b.writes.iter().enumerate() {
                        let wloc = match (w.line, w.column) {
                            (Some(l), Some(c)) => format!("[line {}, column {}]", l, c),
                            _ => "[no location]".to_string(),
                        };
                        println!(
                            "           #{} t{} {} = {} (seq {})",
                            n + 1,
                            w.term_id.0,
                            wloc,
                            w.value,
                            w.seq
                        );
                    }
                }
            }
        }
        if entries.truncated {
            println!("  (chain truncated at depth {})", entries.entries.len());
        }
        if !entries.complete {
            println!("  Incomplete: the chain crosses a cell the trace could not resolve.");
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
        if let Some(snippet) = crate::backend::errors::format_source_snippet(src, &d.span) {
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
    ir: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let mut env = make_env(include_dirs);
    let is_empty = source.trim().is_empty();
    // `--ir` swaps the front end for the IR deserializer, so a third-party
    // emitter's IR can be CI-validated the same way source is. Everything below
    // is unchanged: the lowering gate is the point of `check` either way.
    // The IR loader is not a front-end phase and has no `LoadError`; it reports
    // exactly as `run --ir` does (a plain string tagged `"parse"`).
    let loaded = if ir {
        match env.load_program_ir(source) {
            Ok(pid) => Ok(pid),
            Err(e) => die(json, &e, "parse"),
        }
    } else {
        load_into(&mut env, source, source_input)
    };
    match loaded {
        Ok(pid) => {
            let program = env.get_program(pid);
            // `check` answers "will this run?", so it must lower to bytecode as
            // well as compile: a program can compile cleanly and still fail to
            // lower, and `check` is what CI and editors call. Use the same flags
            // a run would, so `check` and `run` agree on what lowers.
            if let Some(program) = program
                && let Err(e) = crate::backend::bytecode::lower_with_flags(
                    program,
                    crate::env::Env::opt_flags_from_env(),
                )
            {
                // Warnings are about the source, not the lowering, so report
                // them even though the program can't run — a sweep over a
                // corpus must not score a broken file as warning-free.
                if !json {
                    eprint_warnings(program);
                }
                die_with(json, &e, "lower", warnings_json(program));
            }
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
        Err(e) => die_error(json, &e, serde_json::Value::Null, source),
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
        "{}: {} line(s) reformatted, {} redundant cast(s) removed",
        path, outcome.reindented_lines, outcome.casts_removed
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
                print!("{}", crate::ast_display::display_stmts(&stmts));
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
    use crate::backend::bytecode::{disasm, lower_with_flags};
    let program = compile_source(source, source_input, include_dirs);
    // Lowered with the flags a run would use, so the disassembly shows the
    // in-place opcodes it would actually execute: `PETAL_OPT=off`/`none` shows
    // the clone-and-alloc lowering, `PETAL_OPT=all` enables every opt.
    let flags = crate::env::Env::opt_flags_from_env();
    match lower_with_flags(&program, flags) {
        Ok(bc) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&disasm::render_json(&bc, &program)).unwrap()
                );
            } else {
                print!("{}", disasm::render_text(&bc, &program));
            }
        }
        Err(e) => die_plain(&e),
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
    let prov = program.trace_provenance(root_id);
    let ancestor_ids = &prov.ancestors;
    let edges = &prov.edges;

    if json {
        let root_json = term_to_json(root_term);
        let ancestors_json: Vec<_> = ancestor_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        // Every edge a backward walk emits is a value edge by construction —
        // identity edges are exactly the ones it refuses to cross.
        let edges_json = edges_to_json(
            &edges
                .iter()
                .map(|&(a, b)| (a, b, EdgeKind::Dataflow))
                .collect::<Vec<_>>(),
        );
        let output = serde_json::json!({
            "root": root_json,
            "ancestors": ancestors_json,
            "edges": edges_json,
            "frontier": frontier_to_json(&program, &prov.frontier),
            "complete": prov.is_complete(),
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
        print_term_rows(&program, ancestor_ids);
        println!();
        println!("Edges ({}):", edges.len());
        for (from, to) in edges {
            println!("  t{} -> t{}", from.0, to.0);
        }
        if !prov.frontier.is_empty() {
            println!();
            print_frontier(&program, &prov.frontier);
        }
    }
}

/// Render the cell frontier a backward walk stopped at. This command never
/// runs the program, so the answer always degrades to the *static* one —
/// "not traced, and here is the complete set of possible writers" — never to
/// silence.
fn print_frontier(program: &Program, frontier: &[crate::program_analysis::CellFrontier]) {
    println!("Frontier ({}):", frontier.len());
    for f in frontier {
        println!("  t{}: {} (not traced)", f.read_term.0, f.describe());
        if f.writes.is_empty() {
            println!("    no write sites");
        }
        for &w in &f.writes {
            println!("    possible write: {}", term_site(program, w));
        }
        if f.host_writable {
            println!("    also writable by the host through set_state");
        }
    }
}

fn term_site(program: &Program, id: TermId) -> String {
    match program.source_map.get(id) {
        Some(s) if s.start.line > 0 => format!(
            "t{} [line {}, column {}]",
            id.0, s.start.line, s.start.column
        ),
        _ => format!("t{} [no location]", id.0),
    }
}

fn frontier_to_json(
    program: &Program,
    frontier: &[crate::program_analysis::CellFrontier],
) -> Vec<serde_json::Value> {
    frontier
        .iter()
        .map(|f| {
            serde_json::json!({
                "read_term": f.read_term.0,
                "var": f.var_name,
                "decl_term": f.cell_decl.map(|t| t.0),
                "captured": f.captured,
                "host_writable": f.host_writable,
                // Compile-time only: this path never runs the program, so the
                // dynamic writer is unavailable by construction, not missing.
                "resolution": "not_traced",
                "writes": f.writes.iter().map(|&w| {
                    let span = program.source_map.get(w).filter(|s| s.start.line > 0);
                    serde_json::json!({
                        "term_id": w.0,
                        "line": span.map(|s| s.start.line),
                        "column": span.map(|s| s.start.column),
                    })
                }).collect::<Vec<_>>(),
                "summary": f.describe(),
            })
        })
        .collect()
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
    let deps = program.trace_dependents(root_id);
    let dependent_ids = &deps.dependents;
    let edges = &deps.edges;
    let index = program.cell_index();

    if json {
        let root_json = term_to_json(root_term);
        let dependents_json: Vec<_> = dependent_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        let edges_json = edges_to_json(edges);
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
        print_term_rows(&program, dependent_ids);
        println!();
        println!("Edges ({}):", edges.len());
        for (from, to, kind) in edges {
            match kind {
                EdgeKind::Dataflow => println!("  t{} -> t{}", from.0, to.0),
                // A may-edge is a possibility, not a fact; printing it the
                // same way as a value edge would present one as the other.
                EdgeKind::CellMay => {
                    let var = cell_var_for_edge(&index, *from, *to)
                        .map(|v| format!(" (cell '{}', may)", v))
                        .unwrap_or_else(|| " (cell, may)".to_string());
                    println!("  t{} ~> t{}{}", from.0, to.0, var);
                }
                // Likewise for method dispatch: the call finds the function by
                // name at runtime, so this is a possibility, not an operand.
                EdgeKind::DispatchMay => {
                    println!("  t{} ~> t{} (dispatch, may)", from.0, to.0)
                }
            }
        }
    }
}

/// The var name behind a `CellMay` edge, for display.
fn cell_var_for_edge(
    index: &crate::program_analysis::CellIndex,
    from: TermId,
    to: TermId,
) -> Option<String> {
    for cand in [from, to] {
        if let Some(d) = index.decl_for_site(cand)
            && let Some(n) = index.var_name(d)
        {
            return Some(n.to_string());
        }
    }
    None
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

    // Conservative, not minimal: a slice that is too small silently computes a
    // *different value*, while one that is too big only loses precision. The
    // incompleteness is reported in-band rather than through the exit code —
    // the type-level gate is `SliceResult`, not the process status.
    let (slice_ids, frontier) = program.slice(&target_ids).conservative();

    if json {
        let terms_json: Vec<_> = slice_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        let output = serde_json::json!({
            "targets": target_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
            "slice": terms_json,
            "minimal": frontier.is_empty(),
            "complete": frontier.is_empty(),
            "frontier": frontier_to_json(&program, &frontier),
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
        if !frontier.is_empty() {
            println!();
            println!(
                "Not minimal — {} cell read{} crossed:",
                frontier.len(),
                if frontier.len() == 1 { "" } else { "s" }
            );
            print_frontier(&program, &frontier);
            println!(
                "  Every possible write is included, so the slice is sufficient in\n  \
                 terms — but not faithful in order: it does not carry the control\n  \
                 flow that selected among those writes."
            );
        }
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
fn load_into(
    env: &mut Env,
    source: &str,
    input: &SourceInput,
) -> Result<ProgramId, crate::error::LoadError> {
    env.load_program_diag(source, source_origin(input).as_deref())
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

/// Render dataflow graph edges to the `[{ "from", "to", "kind" }]` JSON shape
/// shared by the provenance and dependents outputs. Backward-walk edges are
/// uniformly `"dataflow"` — the walk refuses to cross anything else — so only
/// the forward walk ever emits `"may"`.
fn edges_to_json(edges: &[(TermId, TermId, EdgeKind)]) -> Vec<serde_json::Value> {
    edges
        .iter()
        .map(|(from, to, kind)| {
            serde_json::json!({ "from": from.0, "to": to.0, "kind": kind.as_str() })
        })
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
