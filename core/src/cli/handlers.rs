//! Per-subcommand handlers extracted from the `execute()` dispatch, plus the
//! shared front-end helpers (env construction, source compilation, term
//! resolution, and graph/term rendering) they build on.

use std::fs;
use std::path::PathBuf;
use std::process;

use crate::dot_graph::program_to_dot;
use crate::env::Env;
use crate::ir_display::display_program_with;
use crate::lexer::Lexer;
use crate::program::{Program, ProgramId, Term, TermId, base_fn_name};
use crate::program_analysis::EdgeKind;
use crate::source_map::ENTRY_FILE;
use crate::stack::StackKey;

use super::{
    GraphQuery, ProposeEditOpts, RunOpts, SourceInput, die, die_error, die_plain, die_with,
    error_json_value, print_json,
};

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

pub(super) fn handle_run(
    opts: &RunOpts,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let &RunOpts {
        json,
        trace,
        ir,
        dup_stats,
        profile,
        effect_audit,
        static_audit,
        trace_pending,
        observe,
        trace_emits,
        seed,
        ..
    } = opts;
    let record_trace = opts.record_trace.as_deref();
    if trace || std::env::var("PETAL_DEBUG").is_ok() {
        unsafe {
            std::env::set_var("PETAL_TRACE", "1");
        }
    }
    // `--trace-pending` (or PETAL_TRACE_PENDING=1) turns on the absorption log
    // and prints the frame pending report after the run.
    let trace_pending = trace_pending || std::env::var("PETAL_TRACE_PENDING").is_ok();
    let mut env = make_env(include_dirs);
    // Seed before the first run so the whole session replays; the flag wins
    // over PETAL_SEED (which `Env::new` already applied).
    if let Some(seed) = seed {
        env.set_seed(seed);
    }
    match opts.policy {
        Some(policy) => env.set_policy(policy),
        // `petal run` runs the script once and exits, so a memo record made
        // here is never replayed: recording is all cost, and for a recursive
        // program a large one. Unless a policy was asked for by name, the
        // run keeps the rest of the default policy and leaves memoization
        // off. A host that runs a script every frame keeps it on.
        // The same goes for the static-value cache: a value kept for the
        // next run is kept for nothing, and classifying the program to
        // decide what to keep is a cost on every start.
        // `--runs` is a host's frame loop, where both pay their way.
        None if std::env::var_os("PETAL_POLICY").is_none() && opts.runs.is_none() => {
            env.set_policy(env.policy().with_memo(false).with_static_cache(false));
        }
        None => {}
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
        if let Some(us) = opts.profile_slow_us {
            env.profile_mut().slow_native = std::time::Duration::from_secs_f64(us / 1e6);
        }
        env.profile_mut().set_enabled(true);
        if let SourceInput::File(path) = source_input {
            let base = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            env.profile_mut().entry_name = base;
        }
    }
    // The counters are a runtime switch (off in a release build until asked
    // for), so this has to precede the run.
    if dup_stats {
        env.set_heap_stats(true);
    }
    if effect_audit {
        env.set_effect_audit(true);
    }
    let static_audit_on = static_audit != crate::static_audit::StaticAuditMode::Off;
    if static_audit_on {
        env.set_static_audit(static_audit);
    }
    // A static-only build calls no host native, so the script need not link
    // against this host's: that is the point of it.
    if static_audit == crate::static_audit::StaticAuditMode::StaticOnly {
        env.set_check_only(true);
    }
    let pid = load_checked_or_die(&mut env, json, ir, source, source_input);
    let sid = stack_or_die(&mut env, json, pid);
    match opts.state_storage.as_deref() {
        Some(path) => load_state_storage(&mut env, json, pid, sid, path),
        // Under `--runs` state does carry, from each run to the next.
        None if opts.runs.is_none() => warn_state_not_stored(&env, pid),
        None => {}
    }
    if trace_pending {
        env.enable_pending_trace(sid);
    }
    let run_started = std::time::Instant::now();
    let mut run_result = env.run(sid);
    // `--runs n`: the remaining runs, on the same stack, each from the top.
    // The first one that fails ends the session, as it would end a frame.
    for _ in 1..opts.runs.unwrap_or(1) {
        if run_result.is_err() {
            break;
        }
        run_result = env.reset_stack(sid).and_then(|()| env.run(sid));
    }
    // A static value that could not be kept is rebuilt on every run, which
    // is correct and slow; say which one.
    for warning in env.take_static_warnings(sid) {
        eprintln!("warning: {warning}");
    }
    let run_elapsed = run_started.elapsed();

    // Snapshot the observed values now. The map is a snapshot by contract, and
    // reading it here — before anything else touches the env — keeps the
    // reported values the ones the run finished (or died) with.
    let observed = observe.then(|| env.get_observations_json(pid, sid));

    if let Some(path) = record_trace {
        write_trace_to_file(&env, pid, path);
    }

    if profile {
        // Names are resolved here rather than in `VmProfile` because the native
        // table is the `Env`'s, not the profile's.
        let report = env.profile_report(pid, Some(run_elapsed), 15);
        eprint!("{report}");
    }

    if effect_audit {
        eprint!("{}", env.effect_audit_report());
    }

    // A static term that touched a dynamic input fails the run, after the
    // rest of its output (and after the runtime error, if it also raised).
    let static_audit_failed = static_audit_on && {
        let report = env.static_audit_report();
        eprint!("{report}");
        report.has_failures()
    };

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
            print_json(&report);
        } else {
            print_emit_trace_text(&report);
        }
    }

    // Only a run that finished is saved: one that died stopped partway through
    // its updates, and the file keeps what the last good run left.
    if let Some(path) = opts.state_storage.as_deref()
        && run_result.is_ok()
    {
        save_state_storage(&env, json, pid, sid, path);
    }

    // An `expect` that did not hold was recorded and the run carried on:
    // say so now, and fail the command after the rest of its output.
    let expect_failures = env.take_expect_failures(sid);
    for failure in &expect_failures {
        eprintln!("expectation failed: {failure}");
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
            print_json(&obj);
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
    if static_audit_failed || !expect_failures.is_empty() {
        process::exit(1);
    }
}

/// `petal run` without `--state-storage`, on a script that declares `state`:
/// say on stderr that it will not carry over. Every other host of a Petal
/// script (petal-sdl, garden) keeps `state` from frame to frame, and the guides
/// introduce it as "1 on the first run, 2 on the second" — which, from the
/// command line, is only true with a storage file.
fn warn_state_not_stored(env: &Env, pid: ProgramId) {
    let names = env.declared_state_names(pid);
    if names.is_empty() {
        return;
    }
    eprintln!(
        "warning: this script declares `state` ({}), and 'petal run' does not keep state \
         between runs: every run starts from the initial values. Re-run with \
         --state-storage <file> to save state to a file and load it on the next run.",
        crate::env::state_name_list(&names)
    );
}

/// `--state-storage <file>`, before the run: put the file's state into the
/// stack. A file that does not exist yet, or is empty, is a first run. One that
/// cannot be used ends the command before the script runs, so a bad file is
/// never run past and then overwritten.
fn load_state_storage(env: &mut Env, json: bool, pid: ProgramId, sid: StackKey, path: &str) {
    let fail = |why: String| -> ! {
        die(
            json,
            &format!(
                "Cannot load state storage '{path}': {why}. Use a different file, or delete \
                 this one to start from the script's initial values."
            ),
            "state-storage",
        )
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => fail(format!("it cannot be read ({e})")),
    };
    if text.trim().is_empty() {
        return;
    }
    let document: serde_json::Value = match serde_json::from_str(&text) {
        Ok(document) => document,
        Err(e) => fail(format!("it is not valid JSON ({e})")),
    };
    match env.load_state_storage(pid, sid, &document) {
        Ok(load) => {
            if !load.dropped.is_empty() {
                eprintln!(
                    "warning: state storage '{path}' holds state this script does not declare \
                     ({}); it is not loaded and will be removed from the file.",
                    crate::env::state_name_list(&load.dropped)
                );
            }
        }
        Err(e) => fail(e),
    }
}

/// `--state-storage <file>`, after a run that finished: write every state slot
/// back, creating the file. A value JSON cannot hold (a function, a host
/// handle) is warned about and left out, so its `state` starts from its initial
/// value next run.
fn save_state_storage(env: &Env, json: bool, pid: ProgramId, sid: StackKey, path: &str) {
    let save = env.save_state_storage(pid, sid);
    for (name, what) in &save.skipped {
        eprintln!(
            "warning: state `{name}` holds {what}, which cannot be saved to state storage; it \
             will start from its initial value on the next run."
        );
    }
    let mut text = serde_json::to_string_pretty(&save.document).unwrap_or_default();
    text.push('\n');
    if let Err(e) = fs::write(path, text) {
        die(
            json,
            &format!("Cannot write state storage '{path}': {e}"),
            "state-storage",
        );
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
        print_json(map);
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
    let pid = load_checked_or_die(&mut env, json, false, source, source_input);
    let sid = stack_or_die(&mut env, json, pid);
    // Record absorptions too, so a caller inspecting the report sees per-frame
    // absorption counts populated.
    env.enable_pending_trace(sid);
    let run_result = env.run(sid);

    let report = env.pending_report(pid, sid);
    if json {
        print_json(&report);
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
/// (docs/features/direct-manipulation.md); `propose-edit` is the "act" half, and the
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
/// consistently. See docs/features/direct-manipulation.md for the protocol.
pub(super) fn handle_propose_edit(
    opts: &ProposeEditOpts,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    use crate::direct_manipulation::{
        ManipulationGoal, VarPolicy, apply_edits, propose_edits_batch,
    };
    use crate::provenance;

    let &ProposeEditOpts {
        json,
        emit,
        apply,
        ref channel,
        ref goals,
        ref configurable,
        ref pinned,
    } = opts;
    let mut env = make_env(include_dirs);
    env.enable_emit_trace(true);
    // The per-term trace supplies the values the arithmetic solver inverts
    // against; without it only statically-known siblings can be used.
    env.trace_mut().enable();
    let pid = load_checked_or_die(&mut env, json, false, source, source_input);
    let sid = stack_or_die(&mut env, json, pid);
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
        let Some(path) = source_origin(source_input) else {
            die(
                json,
                "--apply needs a file path (not inline code or stdin)",
                "apply",
            );
        };
        let edited = match apply_edits(source, &chosen) {
            Ok(s) => s,
            Err(e) => die(json, &e.message, "apply"),
        };
        if let Err(e) = fs::write(&path, &edited) {
            die(
                json,
                &format!("writing '{}': {}", path.display(), e),
                "apply",
            );
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
        if let ([(arg_index, to)], [ps]) = (goals.as_slice(), &per_goal[..]) {
            out["arg"] = serde_json::json!(arg_index);
            out["goal"] = serde_json::json!(to);
            out["proposals"] = serde_json::Value::Array(proposals_json(ps));
        }
        print_json(&out);
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
    term_query: &str,
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
    let target_id = resolve_term(program, term_query);

    let entries = env.trace().explain(program, env.heap(), target_id, 16);

    // Pretty header — use the resolved term name if available so an
    // `--term 72` query still shows `(total)` instead of `(72)`.
    // `base_fn_name` because an overload variant's term is named `box#1`
    // internally, and this header names the term as the source wrote it.
    let header_name = program
        .get_term(target_id)
        .name
        .as_deref()
        .map(|n| base_fn_name(n).to_string())
        .unwrap_or_else(|| {
            if term_query.parse::<u32>().is_ok() || term_query.starts_with('t') {
                "unnamed".to_string()
            } else {
                term_query.to_string()
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
        print_json(&out);
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

/// Print a program's diagnostics — the findings of its compile, warnings
/// and errors — to stderr (nothing when there are none), as
/// [`crate::check::render_diagnostics`] renders them. Used before running and
/// by `check`; stderr keeps them off the stdout JSON channel.
pub(super) fn eprint_warnings(program: &Program) {
    let text =
        crate::check::render_diagnostics(program, &program.warnings, super::bare_errors());
    if !text.is_empty() {
        eprint!("{}", text);
    }
}

/// Build the JSON array of a program's diagnostics: one object per diagnostic
/// with `message`, `severity` (`"warning"` or `"error"`), `line`, `column`,
/// and `file` (null for the entry file).
fn warnings_json(program: &Program) -> serde_json::Value {
    let items: Vec<serde_json::Value> = program
        .warnings
        .iter()
        .map(|d| {
            let file = program.source_map.file_name_for_span(&d.span);
            serde_json::json!({
                "message": d.message,
                "severity": d.severity.label(),
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
    lenient: bool,
    ir: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let mut env = make_env(include_dirs);
    // A check, not a run: the script may be written for a host whose natives
    // this process does not register. The interface package it imports
    // declares them (`native fn`), which is all a check needs — so compile
    // without linking.
    env.set_check_only(true);
    // Nor does a check keep anything between runs: lowering for the static
    // cache would classify the whole program to decide what, and the pass
    // cannot fail, so there is nothing in it for a check to report.
    env.set_static_cache(false);
    let is_empty = source.trim().is_empty();
    // `--ir` swaps the front end for the IR deserializer, so a third-party
    // emitter's IR can be CI-validated the same way source is. Everything below
    // is unchanged: the lowering gate is the point of `check` either way.
    let pid = load_or_die(&mut env, json, ir, source, source_input);
    // The check itself (`crate::check`) is the library's: the findings of the
    // compile, and whether the program lowers. `petal run` asks the same
    // question the same way before it starts.
    let report = env.check_program(pid);
    let program = env.get_program(pid);
    if let (Some(e), Some(program)) = (&report.lower_error, program) {
        // Warnings are about the source, not the lowering, so report
        // them even though the program can't run — a sweep over a
        // corpus must not score a broken file as warning-free.
        if !json {
            eprint_warnings(program);
        }
        die_with(json, e, "lower", warnings_json(program));
    }
    let warning_count = report.diagnostics.len();
    let error_count = report.error_count();
    // A checker error is a line that fails whenever it runs, so the program
    // does not pass `check` — unless `--lenient` asks only "does it compile?".
    let failed = error_count > 0 && !lenient;
    if json {
        let warnings = program
            .map(warnings_json)
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        let mut obj = serde_json::json!({ "ok": !failed, "warnings": warnings });
        if let Some(program) = program
            && !program.requires.is_empty()
        {
            obj["requires"] = requires_json(&program.requires);
        }
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
        if let Some(summary) = report.summary() {
            eprintln!("{summary}");
        }
        // Otherwise silent on success, like most linters — but for one line
        // saying which host interfaces the program's packages require, since
        // a check passes with no host present and a run will need them.
        if let Some(program) = program
            && !program.requires.is_empty()
        {
            println!(
                "requires host interfaces: {}",
                program
                    .requires
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    // Checker errors fail `check`; `--strict` extends that to warnings (for
    // CI). Output above is unchanged either way.
    if failed || (strict && warning_count > 0) {
        process::exit(1);
    }
}

/// `petal ir-equal <a.ptl> <b.ptl>` — the standalone IR comparison, which is
/// also what the TS verifier's `ir-equal` step shells out to. Exit 0 when the
/// two compile to the same program, 1 with the first difference when they
/// don't, 2 when a side fails to compile (an answer of "can't tell", which
/// must not be mistaken for "not equal").
///
/// `named_args` is `--named-args`: calls may then differ in writing an argument by name instead of position,
/// wherever both provably bind alike — the comparison `petal suggest --apply`
/// proves its named-argument rewrites with.
pub(super) fn handle_ir_equal(
    json: bool,
    other_path: &str,
    named_args: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let other = match fs::read_to_string(other_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file '{}': {}", other_path, e);
            process::exit(2);
        }
    };
    let origin = source_origin(source_input);
    let result = match named_args {
        false => crate::ir_equiv::sources_equivalent(source, &other, include_dirs, origin.as_deref()),
        true => {
            let opts = crate::suggest::SuggestOptions {
                include_dirs: include_dirs.to_vec(),
                ..Default::default()
            };
            crate::suggest::sources_equivalent_modulo_named_args(
                source,
                &other,
                origin.as_deref(),
                &opts,
            )
        }
    };
    match result {
        Err(msg) => {
            if json {
                println!("{}", serde_json::json!({ "equal": false, "error": msg }));
            } else {
                eprintln!("Error: {}", msg);
            }
            process::exit(2);
        }
        Ok(Ok(())) => {
            if json {
                println!("{}", serde_json::json!({ "equal": true }));
            } else {
                println!("IR is equivalent");
            }
        }
        Ok(Err(diff)) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "equal": false, "diff": diff.to_json() })
                );
            } else {
                println!("IR differs\n{}", diff);
            }
            process::exit(1);
        }
    }
}

/// Compact `line:col-line:col` rendering of a token span (1-based,
/// end-exclusive), shared by both `show-tokens` output forms.
fn token_span_compact(span: &crate::source_map::SourceSpan) -> String {
    format!(
        "{}:{}-{}:{}",
        span.start.line, span.start.column, span.end.line, span.end.column
    )
}

pub(super) fn handle_show_tokens(json: bool, source: &str) {
    let mut lexer = Lexer::new(source);
    match lexer.tokenize() {
        Ok(_) => {
            if json {
                // Uniform rows: {"kind", "value"?, "span"}, one per line.
                // Assembled by hand so the key order and the one-row-per-line
                // layout stay stable (serde_json would sort keys and either
                // cram everything on one line or explode the span array).
                let rows: Vec<String> = lexer
                    .tokens_with_spans()
                    .map(|(token, span)| {
                        let mut row = format!("{{\"kind\": \"{}\"", token.kind_name());
                        if let Some(value) = token.value_json() {
                            row.push_str(&format!(", \"value\": {}", value));
                        }
                        row.push_str(&format!(
                            ", \"span\": [{}, {}, {}, {}]}}",
                            span.start.line, span.start.column, span.end.line, span.end.column
                        ));
                        row
                    })
                    .collect();
                println!("[\n  {}\n]", rows.join(",\n  "));
            } else {
                for (i, (token, span)) in lexer.tokens_with_spans().enumerate() {
                    let value = match token.value_json() {
                        // JSON rendering doubles as the text rendering:
                        // numbers print bare, strings print quoted+escaped.
                        Some(v) => format!(" {}", v),
                        None => String::new(),
                    };
                    println!(
                        "{}: {}{} @{}",
                        i,
                        token.kind_name(),
                        value,
                        token_span_compact(span)
                    );
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
                print_json(&stmts);
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
    user_only: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let program = compile_source(source, source_input, include_dirs);
    if json {
        if user_only {
            // Filtered debugging view — see `ir_display::user_only_json`.
            // NOT the `run --ir` interchange format.
            print_json(&crate::ir_display::user_only_json(&program));
        } else {
            print_json(&program);
        }
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
    use crate::backend::bytecode::disasm;
    let program = compile_source(source, source_input, include_dirs);
    // Lowered with the flags a run would use, so the disassembly shows the
    // in-place opcodes it would actually execute: `PETAL_POLICY=baseline`
    // shows the clone-and-alloc lowering. Like `petal run`, which runs a
    // script once, this leaves the static-value cache out unless a policy
    // was asked for by name: `PETAL_POLICY=fast` shows the `static_skip` /
    // `static_store` instructions a host that runs every frame executes.
    let mut flags = crate::policy::RunPolicy::from_env().opts;
    if std::env::var_os("PETAL_POLICY").is_none() {
        flags.static_cache = false;
    }
    match make_env(include_dirs).lower_with(&program, flags) {
        Ok(bc) => {
            if json {
                print_json(&disasm::render_json(&bc, &program));
            } else {
                print!("{}", disasm::render_text(&bc, &program));
            }
        }
        Err(e) => die_plain(&e),
    }
}

/// `petal show-static` — the static-value classifier's answer for every
/// top-level declaration: static data, a function or dynamic, why, whether
/// dynamic code reads it, and what it depends on. Compiles and never runs.
///
/// The natives are the ones this CLI registers. A `native fn` of a host that
/// is not here is dynamic, so a host may report more as static than this
/// does, never less.
pub(super) fn handle_show_static(
    json: bool,
    all: bool,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let mut env = make_env(include_dirs);
    env.set_check_only(true);
    let result = match source_origin(source_input) {
        Some(path) => env.compile_program_at(ProgramId(0), source, &path),
        None => env.compile_program(ProgramId(0), source),
    };
    let program = match result {
        Ok(program) => program,
        Err(e) => die_plain(&e),
    };
    let report = env.static_report(&program);
    if json {
        print_json(&report.to_json(all));
    } else {
        print!("{}", report.render_text(all));
    }
}

/// `petal graph` — one dataflow query, one result shape.
///
/// JSON: `{direction, targets, target_terms, terms, edges, frontier, complete,
/// minimal}`.
/// `complete`/`minimal` are false exactly when the walk met a cell: backward
/// it stopped there (the answer may be missing a writer's chain), forward it
/// crossed a may-edge (the answer over-approximates).
pub(super) fn handle_graph(
    json: bool,
    term_queries: &[String],
    query: GraphQuery,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    use crate::program_analysis::CellFrontier;

    let program = compile_source(source, source_input, include_dirs);
    let target_ids = resolve_terms(&program, term_queries);

    let (term_ids, edges, frontier): (
        Vec<TermId>,
        Vec<(TermId, TermId, EdgeKind)>,
        Vec<CellFrontier>,
    ) = match query {
        GraphQuery::Provenance => {
            let prov = program.trace_provenance(target_ids[0]);
            // Every edge a backward walk emits is a value edge by
            // construction — identity edges are exactly the ones it
            // refuses to cross.
            let edges = prov
                .edges
                .iter()
                .map(|&(a, b)| (a, b, EdgeKind::Dataflow))
                .collect();
            (prov.ancestors, edges, prov.frontier)
        }
        GraphQuery::Dependents => {
            // Several roots: the union of their forward walks, in first-seen
            // order, each term and edge once.
            let mut terms = Vec::new();
            let mut edges = Vec::new();
            let mut frontier: Vec<CellFrontier> = Vec::new();
            let mut seen_terms = std::collections::HashSet::new();
            let mut seen_edges = std::collections::HashSet::new();
            for &root in &target_ids {
                let deps = program.trace_dependents(root);
                for t in deps.dependents {
                    if seen_terms.insert(t) {
                        terms.push(t);
                    }
                }
                for e in deps.edges {
                    if seen_edges.insert(e) {
                        edges.push(e);
                    }
                }
                frontier.extend(deps.frontier);
            }
            (terms, edges, frontier)
        }
        GraphQuery::Slice => {
            // Conservative, not minimal: a slice that is too small silently
            // computes a *different value*, while one that is too big only
            // loses precision. The incompleteness is reported in-band
            // rather than through the exit code — the type-level gate is
            // `SliceResult`, not the process status.
            let (ids, frontier) = program.slice(&target_ids).conservative();
            let edges = program.induced_edges(&program.cell_index(), &ids);
            (ids, edges, frontier)
        }
    };
    // A cell reached along two paths is still one frontier entry.
    let mut frontier = frontier;
    let mut seen_reads = std::collections::HashSet::new();
    frontier.retain(|f| seen_reads.insert((f.read_term, f.cell_decl)));
    let complete = frontier.is_empty();

    if json {
        let terms_json: Vec<_> = term_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        let target_terms: Vec<_> = target_ids
            .iter()
            .map(|&id| term_to_json(program.get_term(id)))
            .collect();
        let output = serde_json::json!({
            "direction": if query == GraphQuery::Dependents { "forward" } else { "back" },
            "targets": target_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
            "target_terms": target_terms,
            "terms": terms_json,
            "edges": edges_to_json(&edges),
            "frontier": frontier_to_json(&program, &frontier),
            "complete": complete,
            "minimal": complete,
        });
        print_json(&output);
        return;
    }

    let describe = |id: TermId| {
        format!(
            "t{} ({})",
            id.0,
            program
                .get_term(id)
                .name
                .as_deref()
                .map(base_fn_name)
                .unwrap_or("unnamed")
        )
    };
    let targets_list = target_ids
        .iter()
        .map(|&id| describe(id))
        .collect::<Vec<_>>()
        .join(", ");
    match query {
        GraphQuery::Provenance => {
            let root_term = program.get_term(target_ids[0]);
            println!("Provenance of {}:", targets_list);
            println!("  op: {:?}", root_term.op);
            println!(
                "  inputs: {:?}",
                root_term.inputs.iter().map(|i| i.0).collect::<Vec<_>>()
            );
            println!();
            println!("Ancestors ({}):", term_ids.len());
        }
        GraphQuery::Dependents => {
            println!("Dependents of {}:", targets_list);
            if let [only] = target_ids.as_slice() {
                println!("  op: {:?}", program.get_term(*only).op);
            }
            println!();
            println!("Downstream ({}):", term_ids.len());
        }
        GraphQuery::Slice => {
            println!(
                "Slice for targets: {}",
                target_ids
                    .iter()
                    .map(|id| format!("t{}", id.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!();
            println!("Terms ({}):", term_ids.len());
        }
    }
    print_term_rows(&program, &term_ids);
    println!();
    println!("Edges ({}):", edges.len());
    print_edges(&program, &edges);

    if frontier.is_empty() {
        return;
    }
    println!();
    match query {
        GraphQuery::Slice => {
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
        GraphQuery::Dependents => {
            print_frontier(&program, &frontier);
            println!(
                "  Reached through a cell may-edge: any of these writes may supply\n  \
                 the read, so the downstream set over-approximates."
            );
        }
        GraphQuery::Provenance => print_frontier(&program, &frontier),
    }
}

/// Edge rows for the text form. A may-edge is a possibility, not a fact, so it
/// is drawn `~>` and labelled rather than printed like a value edge.
fn print_edges(program: &Program, edges: &[(TermId, TermId, EdgeKind)]) {
    let mut index = None;
    for (from, to, kind) in edges {
        match kind {
            EdgeKind::Dataflow => println!("  t{} -> t{}", from.0, to.0),
            EdgeKind::CellMay => {
                let index = index.get_or_insert_with(|| program.cell_index());
                let var = cell_var_for_edge(index, *from, *to)
                    .map(|v| format!(" (cell '{}', may)", v))
                    .unwrap_or_else(|| " (cell, may)".to_string());
                println!("  t{} ~> t{}{}", from.0, to.0, var);
            }
            // Method dispatch finds the function by name at runtime, so this
            // is a possibility, not an operand.
            EdgeKind::DispatchMay => println!("  t{} ~> t{} (dispatch, may)", from.0, to.0),
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
///
/// `-I` also picks up packages: a directory holding a `petal.toml`, and any
/// directly under it, becomes importable as `<package>/<module>` (see
/// docs/module-system.md). A manifest that claims to be a package and then
/// will not load is a hard error here — the user pointed at it, so a silent
/// "no such module" later would be the wrong answer.
/// `petal test`: run the test blocks under the given paths
/// (`crate::test_runner`), print the report, and exit 1 unless all passed.
pub(super) fn handle_test(opts: &super::TestArgs, include_dirs: &[PathBuf]) {
    use crate::test_runner as runner;
    let run_opts = runner::Options {
        include_dirs: include_dirs.to_vec(),
        filter: opts.filter.clone(),
        timeout: match opts.timeout {
            Some(s) if s == 0.0 => None,
            Some(s) => Some(std::time::Duration::from_secs_f64(s)),
            None => Some(std::time::Duration::from_secs(60)),
        },
        list: opts.list,
    };
    let files = match opts.inline.as_deref() {
        Some(code) => vec![runner::run_source("<inline>", None, code, &run_opts)],
        None => runner::run_paths(&opts.paths, &run_opts),
    };
    if opts.json {
        print_json(&runner::to_json(&files));
    } else {
        print!("{}", runner::render(&files, opts.show_output));
    }
    if !runner::Summary::of(&files).ok() {
        process::exit(1);
    }
}

pub(super) fn make_env(include_dirs: &[PathBuf]) -> Env {
    let mut env = Env::new();
    for dir in include_dirs {
        env.add_module_path(dir.clone());
    }
    // `-e` code and stdin have no file: `import ./x` means the working
    // directory.
    env.set_inline_import_dir(std::env::current_dir().ok());
    // A git dependency that is not there yet is fetched when it is imported.
    env.set_install_on_demand(std::env::var_os("PETAL_OFFLINE").is_none());
    if let Some(err) = env.package_errors().first() {
        die_plain(&err.to_string());
    }
    env
}

/// One node of the project's dependency graph, as `petal packages` shows it.
struct DepNode {
    /// The import name (the `[dependencies]` key).
    name: String,
    /// How the manifest says to get it — and, for an installed git package,
    /// the commit it is locked to.
    source: String,
    /// The package it resolved to, or why it did not.
    package: Result<DepPackage, String>,
    /// Its own dependencies; empty when this package was already shown.
    children: Vec<DepNode>,
    /// True when the package appears earlier in the graph (shown once).
    repeated: bool,
}

/// What a resolved dependency is.
struct DepPackage {
    name: String,
    version: Option<String>,
    root: PathBuf,
    requires: Vec<crate::package::Requirement>,
    /// `(commit, tree hash, vendored)` for a git package.
    locked: Option<(String, String, bool)>,
}

impl DepPackage {
    fn of(info: &crate::package::PackageInfo) -> DepPackage {
        DepPackage {
            name: info.name.clone(),
            version: info.version.clone(),
            root: info.root.clone(),
            requires: info.requires.clone(),
            locked: None,
        }
    }
}

/// The graph as the package manager resolves it from what is installed:
/// one version of each package, git packages at their locked commits.
fn installed_dependency_graph(
    graph: &crate::pkgman::Graph,
    owner: &crate::pkgman::Node,
    seen: &mut Vec<String>,
) -> Vec<DepNode> {
    let manifest = crate::package::Manifest::load(&owner.root).ok();
    owner
        .dependencies
        .iter()
        .map(|(key, package_name)| {
            let Some(node) = graph.package(package_name) else {
                return DepNode {
                    name: key.clone(),
                    source: String::new(),
                    package: Err(format!("package '{package_name}' is not in the graph")),
                    children: Vec::new(),
                    repeated: false,
                };
            };
            let source = match &node.source {
                // A path reads best as the owner's manifest writes it.
                crate::pkgman::NodeSource::Path { .. } => manifest
                    .as_ref()
                    .and_then(|m| m.dependencies.iter().find(|d| d.name == *key))
                    .map(|d| d.source.to_string())
                    .unwrap_or_else(|| node.source.to_string()),
                other => other.to_string(),
            };
            let repeated = seen.contains(&node.name);
            let children = if repeated {
                Vec::new()
            } else {
                seen.push(node.name.clone());
                installed_dependency_graph(graph, node, seen)
            };
            DepNode {
                name: key.clone(),
                source,
                package: Ok(DepPackage {
                    name: node.name.clone(),
                    version: node.version.clone(),
                    root: node.root.clone(),
                    requires: node.requires.clone(),
                    locked: match &node.source {
                        crate::pkgman::NodeSource::Git {
                            commit,
                            tree,
                            vendored,
                            ..
                        } => Some((commit.clone(), tree.clone(), *vendored)),
                        _ => None,
                    },
                }),
                children,
                repeated,
            }
        })
        .collect()
}

/// The graph manifest by manifest, for a project whose graph does not
/// resolve as a whole (something is not installed, or two packages
/// conflict): each dependency that does load is shown, the rest say why not.
fn dependency_graph(
    env: &Env,
    owner: &crate::package::PackageInfo,
    seen: &mut Vec<PathBuf>,
) -> Vec<DepNode> {
    let manifest = owner.root.join(crate::package::MANIFEST_FILE);
    env.project_dependencies(&manifest)
        .into_iter()
        .map(|(name, package)| {
            let source = owner
                .dependency(&name)
                .map(|d| d.source.to_string())
                .unwrap_or_default();
            let (children, repeated) = match &package {
                Ok(info) if seen.contains(&info.root) => (Vec::new(), true),
                Ok(info) => {
                    seen.push(info.root.clone());
                    (dependency_graph(env, info, seen), false)
                }
                Err(_) => (Vec::new(), false),
            };
            DepNode {
                name,
                source,
                package: package.map(|info| DepPackage::of(&info)),
                children,
                repeated,
            }
        })
        .collect()
}

/// Every `[requires]` of the project and of each package of its graph.
fn graph_requires(
    project: &crate::package::PackageInfo,
    graph: &[DepNode],
) -> Vec<crate::package::RequiredInterface> {
    fn add(
        name: &str,
        requires: &[crate::package::Requirement],
        out: &mut Vec<crate::package::RequiredInterface>,
    ) {
        for r in requires {
            let req = crate::package::RequiredInterface {
                interface: r.interface.clone(),
                version: r.version.clone(),
                package: name.to_string(),
            };
            if !out.contains(&req) {
                out.push(req);
            }
        }
    }
    fn walk(nodes: &[DepNode], out: &mut Vec<crate::package::RequiredInterface>) {
        for node in nodes {
            if let Ok(package) = &node.package {
                add(&package.name, &package.requires, out);
            }
            walk(&node.children, out);
        }
    }
    let mut out = Vec::new();
    add(&project.name, &project.requires, &mut out);
    walk(graph, &mut out);
    out.sort_by(|a, b| (&a.interface, &a.package).cmp(&(&b.interface, &b.package)));
    out
}

fn requires_json(requires: &[crate::package::RequiredInterface]) -> serde_json::Value {
    serde_json::Value::Array(
        requires
            .iter()
            .map(|r| {
                serde_json::json!({
                    "interface": r.interface,
                    "version": r.version.to_string(),
                    "package": r.package,
                })
            })
            .collect(),
    )
}

fn requirements_json(requires: &[crate::package::Requirement]) -> serde_json::Value {
    serde_json::Value::Array(
        requires
            .iter()
            .map(|r| serde_json::json!({ "interface": r.interface, "version": r.version.to_string() }))
            .collect(),
    )
}

fn own_requires_json(info: &crate::package::PackageInfo) -> serde_json::Value {
    requirements_json(&info.requires)
}

fn dep_nodes_json(nodes: &[DepNode]) -> serde_json::Value {
    serde_json::Value::Array(
        nodes
            .iter()
            .map(|n| match &n.package {
                Ok(package) => {
                    let mut row = serde_json::json!({
                        "name": n.name,
                        "source": n.source,
                        "package": package.name,
                        "version": package.version,
                        "root": package.root.display().to_string(),
                        "requires": requirements_json(&package.requires),
                        "repeated": n.repeated,
                        "dependencies": dep_nodes_json(&n.children),
                    });
                    if let Some((commit, tree, vendored)) = &package.locked {
                        row["commit"] = commit.clone().into();
                        row["tree"] = tree.clone().into();
                        row["vendored"] = (*vendored).into();
                    }
                    row
                }
                Err(e) => serde_json::json!({
                    "name": n.name,
                    "source": n.source,
                    "error": e,
                }),
            })
            .collect(),
    )
}

fn print_dep_nodes(nodes: &[DepNode], depth: usize) {
    let pad = "    ".repeat(depth);
    for n in nodes {
        match &n.package {
            Ok(package) => {
                let renamed = if package.name == n.name {
                    String::new()
                } else {
                    format!(" = {}", package.name)
                };
                println!(
                    "{pad}{}{renamed}{}  ({}){}",
                    n.name,
                    package.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default(),
                    n.source,
                    if n.repeated { "  (shown above)" } else { "" },
                );
                if !n.repeated && !package.requires.is_empty() {
                    println!(
                        "{pad}    requires: {}",
                        package
                            .requires
                            .iter()
                            .map(|r| format!("{} {}", r.interface, r.version))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            Err(e) => println!("{pad}{}  ({})  UNRESOLVED: {e}", n.name, n.source),
        }
        print_dep_nodes(&n.children, depth + 1);
    }
}

/// `petal packages [--json] [path]`: the project's resolved dependency graph
/// and required host interfaces, what the `-I` directories make available,
/// and the packages bundled in the binary.
pub fn handle_packages(json: bool, path: Option<&str>, include_dirs: &[PathBuf]) {
    let mut env = make_env(include_dirs);
    // This command reports what is there; it fetches nothing.
    env.set_install_on_demand(false);
    let packages = env.packages();
    // Manifests that looked like packages and would not load. A `-I` one is
    // already fatal in `make_env`; these are the ambient `PETAL_PATH` ones,
    // which are deliberately non-fatal — but this is the command whose whole
    // job is to say what is available, so a library missing because its
    // manifest is malformed has to be reported rather than simply absent.
    let mut problems: Vec<String> = env
        .package_errors()
        .iter()
        .chain(env.ambient_package_errors())
        .map(|e| e.to_string())
        .collect();

    // The project: the nearest petal.toml at or above the path given (a file
    // or a directory), or the working directory.
    let start = path
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let probe = if start.is_dir() {
        start.join(crate::package::MANIFEST_FILE)
    } else {
        start
    };
    let project = match env.project_of(&probe) {
        Ok(project) => project,
        Err(e) => {
            problems.push(e);
            None
        }
    };
    let graph = project.as_ref().map(|p| {
        match crate::pkgman::installed_graph(&p.root) {
            Ok(resolved) => {
                let mut seen = vec![resolved.project().name.clone()];
                installed_dependency_graph(&resolved, resolved.project(), &mut seen)
            }
            Err(e) => {
                problems.push(format!("the dependency graph does not resolve: {e}"));
                let mut seen = vec![p.root.clone()];
                dependency_graph(&env, p, &mut seen)
            }
        }
    });
    let locked = project
        .as_ref()
        .is_some_and(|p| p.root.join(crate::pkgman::LOCK_FILE).is_file());
    let requires = project
        .as_ref()
        .zip(graph.as_ref())
        .map(|(p, g)| graph_requires(p, g))
        .unwrap_or_default();
    let bundled = Env::bundled_packages();
    let is_builtin = |name: &str| {
        name == crate::bundled::CORE_PACKAGE || crate::bundled::BUILTIN_PACKAGES.contains(&name)
    };

    if json {
        let rows: Vec<serde_json::Value> = packages
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "version": p.version,
                    "root": p.root.display().to_string(),
                    "module_dir": p.module_dir.display().to_string(),
                    "modules": p.modules,
                    "requires": own_requires_json(p),
                })
            })
            .collect();
        let bundled_rows: Vec<serde_json::Value> = bundled
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "version": p.version,
                    "modules": p.modules,
                    "builtin": is_builtin(&p.name),
                    "requires": own_requires_json(p),
                })
            })
            .collect();
        let project_json = match (&project, &graph) {
            (Some(p), Some(g)) => serde_json::json!({
                "name": p.name,
                "version": p.version,
                "root": p.root.display().to_string(),
                "module_dir": p.module_dir.display().to_string(),
                "modules": p.modules,
                "requires": own_requires_json(p),
                "dependencies": dep_nodes_json(g),
                "locked": locked,
                "required_interfaces": requires_json(&requires),
            }),
            _ => serde_json::Value::Null,
        };
        print_json(&serde_json::json!({
            "project": project_json,
            "packages": rows,
            "bundled": bundled_rows,
            "errors": problems,
        }));
        return;
    }
    for problem in &problems {
        eprintln!("warning: {problem}");
    }
    if let (Some(p), Some(graph)) = (&project, &graph) {
        println!("project {}  {}", p.label(), p.root.display());
        if graph.is_empty() {
            println!("    (no dependencies)");
        }
        print_dep_nodes(graph, 1);
        if requires.is_empty() {
            println!("requires no host interfaces");
        } else {
            println!("requires host interfaces:");
            for r in &requires {
                println!("    {} {}  (package {})", r.interface, r.version, r.package);
            }
        }
        println!();
    }
    if packages.is_empty() {
        println!("No packages found on the search path. Point -I at a directory holding a");
        println!("petal.toml, or at a directory of such libraries.");
    }
    for package in &packages {
        println!("{}  {}", package.label(), package.root.display());
        for module in &package.modules {
            println!("    {}/{}", package.name, module);
        }
    }
    println!();
    println!("bundled in this binary:");
    for package in &bundled {
        let mut notes: Vec<String> = Vec::new();
        if is_builtin(&package.name) {
            notes.push("builtins".to_string());
        }
        if !package.requires.is_empty() {
            notes.push(format!(
                "requires {}",
                package
                    .requires
                    .iter()
                    .map(|r| format!("{} {}", r.interface, r.version))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        println!(
            "    {}{}",
            package.label(),
            if notes.is_empty() {
                String::new()
            } else {
                format!("  ({})", notes.join("; "))
            }
        );
    }
}

/// Run the full front end (module resolution included). Returns the compiled
/// Program.
fn compile_source(
    source: &str,
    input: &SourceInput,
    include_dirs: &[PathBuf],
) -> crate::program::Program {
    let mut env = make_env(include_dirs);
    // Static tools read the compiled program and never run it.
    env.set_check_only(true);
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
pub(super) fn load_into(
    env: &mut Env,
    source: &str,
    input: &SourceInput,
) -> Result<ProgramId, crate::error::LoadError> {
    env.load_program_diag(source, source_origin(input).as_deref())
}

/// Load `source` into `env` — as JSON IR under `--ir`, as Petal source
/// otherwise — exiting through the JSON-aware error path on failure.
fn load_or_die(
    env: &mut Env,
    json: bool,
    ir: bool,
    source: &str,
    input: &SourceInput,
) -> ProgramId {
    if ir {
        // The IR loader is a deserializer, not the front end; it has no phase
        // of its own, and reported "parse" before the phase channel existed.
        match env.load_program_ir(source) {
            Ok(pid) => pid,
            Err(e) => die(json, &e, "parse"),
        }
    } else {
        match load_into(env, source, input) {
            Ok(pid) => pid,
            Err(e) => die_error(json, &e, serde_json::Value::Null, source),
        }
    }
}

/// [`load_or_die`] for a command that is about to *run* the program: check,
/// link, and only then hand it back. The check is `petal check`'s
/// ([`Env::check_program`]) — its findings are printed to stderr the same
/// way, and an error among them, or a program that does not lower, ends the
/// command before the first line runs, whether or not the line at fault
/// would have been reached. So a script `petal check` rejects does not
/// start, and one that starts is one `petal check` passes.
///
/// The load itself does not link, so that a program with both problems is
/// told about its own errors before it is told this host cannot run it.
pub(super) fn load_checked_or_die(
    env: &mut Env,
    json: bool,
    ir: bool,
    source: &str,
    input: &SourceInput,
) -> ProgramId {
    // JSON IR is linked as it is loaded: its links are read off its calls.
    let was_check_only = env.is_check_only();
    env.set_check_only(!ir);
    let pid = load_or_die(env, json, ir, source, input);
    env.set_check_only(was_check_only);

    let report = env.check_program(pid);
    if let Some(program) = env.get_program(pid) {
        // Stderr even in --json mode, so JSON consumers of stdout are
        // unaffected.
        eprint_warnings(program);
        if let Some(e) = &report.lower_error {
            die(json, e, "lower");
        }
        if !report.passed() {
            if json {
                let errors: Vec<_> = report
                    .errors()
                    .map(|d| {
                        let file = program.source_map.file_name_for_span(&d.span);
                        (d.clone(), file.map(str::to_string))
                    })
                    .collect();
                die_error(
                    json,
                    &crate::error::LoadError::from_attributed_diagnostics(
                        crate::error::Phase::Check,
                        &errors,
                    ),
                    serde_json::Value::Null,
                    source,
                );
            }
            // The findings are already printed, each with its line.
            if let Some(summary) = report.summary() {
                eprintln!("{summary}; the program was not run");
            }
            process::exit(1);
        }
    }
    // A caller that asked for check-only (the static-only build) runs no
    // host native, so the program need not link against this host's.
    if !was_check_only && let Err(e) = env.link(pid) {
        die_error(json, &e, serde_json::Value::Null, source);
    }
    pid
}

/// Compile `pid`'s program into a runnable stack, exiting on failure.
fn stack_or_die(env: &mut Env, json: bool, pid: ProgramId) -> StackKey {
    match env.create_stack(pid) {
        Ok(sid) => sid,
        Err(e) => die(json, &e, "compile"),
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

/// Resolve one `--term` name/id query, exiting with the `term_not_found`
/// hint when it does not resolve.
fn resolve_term(program: &Program, query: &str) -> TermId {
    program
        .find_term(query)
        .unwrap_or_else(|| term_not_found(program, query))
}

/// Resolve `--term` name/id queries to term ids, exiting with a
/// `term_not_found` hint on the first query that does not resolve.
fn resolve_terms(program: &Program, queries: &[String]) -> Vec<TermId> {
    queries.iter().map(|q| resolve_term(program, q)).collect()
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
            t.name.as_deref().map(base_fn_name).unwrap_or("")
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
        "name": term.name.as_deref().map(base_fn_name),
        "inputs": term.inputs.iter().map(|i| i.0).collect::<Vec<_>>(),
    })
}

/// `petal suggest` — suggest safe refactors for a file.
///
/// Report-only by default. `--apply` writes, but each kind of suggestion only
/// behind the proof that makes accepting it safe; `--verify` runs the same
/// proofs and reports without writing.
///
/// **Type annotations** are *claims* about a type, and the checker is the
/// thing that verifies claims:
///
/// 1. the rewritten source must still compile;
/// 2. it must not have gained a type-checker warning.
///
/// So writing an annotation and then finding the checker unhappy with it
/// means the inference was wrong, and it is dropped.
///
/// **Named arguments** are claimed to change nothing at all, which is a
/// stronger claim with a stronger proof: the rewritten source must compile to
/// the same IR, where a call may differ from its original only in how its
/// arguments are written and only if both provably bind alike
/// (`suggest::verify_named_args`) — and it must gain no warning either. They
/// are proven on top of the accepted annotations, so the two kinds cannot
/// vouch for each other.
///
/// **Return types** for loop-tailed functions come in two strengths. `-> list`
/// names what the function already returns, so it gets the strong proof: the
/// same IR, and no new warning. `-> nil` is the one rewrite here that is
/// *meant* to change the program — it turns the implicit return off, so the
/// loop stops building a list nobody reads — and so it cannot be held to the
/// IR: it must compile and gain no warning, and rests otherwise on the call
/// sites the analysis read (`suggest::return_types`). A function whose callers
/// are not all in view is reported with both options and never written.
///
/// Nothing is written unless at least one suggestion survives.
pub(super) fn handle_suggest(
    args: &super::SuggestArgs,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    use crate::suggest::{NamedArgs, ReturnType, Suggestion};

    let origin = source_origin(source_input);
    let opts = crate::suggest::SuggestOptions {
        include_dirs: include_dirs.to_vec(),
        from: args.from.clone(),
        kinds: args.kinds,
    };
    let outcome = match crate::suggest::suggest_source(source, origin.as_deref(), &opts) {
        Ok(o) => o,
        Err(e) => die_plain(&e),
    };

    if args.json {
        print_suggest_json(&outcome);
        return;
    }
    for note in &outcome.notes {
        eprintln!("suggest: {note}");
    }
    let functions = format!(
        "{} function{}",
        outcome.functions,
        if outcome.functions == 1 { "" } else { "s" }
    );
    if outcome.suggestions.is_empty()
        && outcome.named_args.is_empty()
        && outcome.return_types.is_empty()
        && outcome.advice.is_empty()
    {
        println!("no suggestions ({functions} examined)");
        return;
    }

    let name = match source_input {
        SourceInput::File(p) => p.as_str(),
        _ => "<inline>",
    };
    let annotation = |s: &Suggestion| match s.slot {
        crate::typecheck::infer::Slot::Return => format!("-> {}", s.ty),
        crate::typecheck::infer::Slot::Param(_) => format!("{}: {}", s.param, s.ty),
    };
    let mut last: Option<&(String, usize)> = None;
    for s in &outcome.suggestions {
        if last != Some(&s.function) {
            println!("\n{}:{}  fn {}", name, s.line, s.function.0);
            last = Some(&s.function);
        }
        println!("  suggest: {}", annotation(s));
        println!("  because: {}", s.because);
    }
    for n in &outcome.named_args {
        println!("\n{}:{}:{}  call {}", name, n.line, n.column, n.callee);
        println!("  suggest: {}", one_line(&n.after));
        println!("  because: {}", n.because);
    }
    // A loop tail's return type. Where the callers in view do not settle it
    // there is a choice rather than a suggestion, and nothing to write.
    for r in &outcome.return_types {
        println!("\n{}:{}  fn {}", name, r.line, r.function.0);
        match r.ty {
            Some(ty) => println!("  suggest: -> {ty}"),
            None => println!("  choose: -> list or -> nil"),
        }
        println!("  because: {}", r.because);
    }
    // Advice is a comment about the code, not a rewrite of it: there is no
    // `suggest:` line because there is no text to write.
    for a in &outcome.advice {
        println!("\n{}:{}:{}  {}", name, a.line, a.column, a.subject);
        println!("  advice: {}", a.message);
        println!("  rule: {}", a.rule);
    }

    let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    let mut summary = Vec::new();
    if !outcome.suggestions.is_empty() || (args.kinds.types && !args.kinds.named_args) {
        summary.push(format!(
            "{} across {functions}",
            count(outcome.suggestions.len(), "type annotation")
        ));
    }
    if !outcome.named_args.is_empty() {
        summary.push(format!(
            "{} that could name {} arguments",
            count(outcome.named_args.len(), "call"),
            if outcome.named_args.len() == 1 { "its" } else { "their" }
        ));
    }
    let return_edits: Vec<ReturnType> = outcome
        .return_types
        .iter()
        .filter(|r| r.is_edit())
        .cloned()
        .collect();
    if !outcome.return_types.is_empty() {
        let open = outcome.return_types.len() - return_edits.len();
        summary.push(format!(
            "{} for a loop's implicit return{}",
            count(outcome.return_types.len(), "return type"),
            match open {
                0 => String::new(),
                n => format!(" ({n} left to choose, never applied)"),
            }
        ));
    }
    if !outcome.advice.is_empty() {
        summary.push(format!(
            "{} piece{} of advice (a comment, never applied)",
            outcome.advice.len(),
            if outcome.advice.len() == 1 { "" } else { "s" }
        ));
    }
    println!("\n{}.", summary.join("; "));

    // Advice, or a choice left to the author, leaves nothing to write or to
    // prove.
    if outcome.suggestions.is_empty() && outcome.named_args.is_empty() && return_edits.is_empty() {
        return;
    }
    if !args.apply && !args.verify {
        println!("Re-run with --apply to write them.");
        return;
    }

    // Each side of a proof is compiled once. The original is allowed not to
    // compile here only in the sense that it cannot: `suggest_source` above
    // already compiled it.
    use crate::suggest::Compiled;
    let compile = |src: &str, what: &str| {
        Compiled::new(src, origin.as_deref(), &opts)
            .map_err(|e| format!("the {what} source does not compile ({e})"))
    };
    let no_new_warning = |before: &Compiled, after: &Compiled, what: &str| {
        // Baselined against the original: plenty of working files carry a
        // warning already (a capture-lag note, a discarded result), and
        // counting those would refuse every suggestion in them for a reason
        // that has nothing to do with the suggestion.
        match after.warnings_gained_over(before).as_slice() {
            [] => Ok(()),
            gained => Err(format!(
                "the {what} source gains {} type warning(s): {}",
                gained.len(),
                gained[0]
            )),
        }
    };
    let original = match compile(source, "original") {
        Ok(c) => c,
        Err(e) => die_plain(&e),
    };

    // Annotations first. The fast path: the whole batch verifies, which is
    // what happens almost always. Only when it does not is it worth paying
    // for the search below.
    let annotated = |accepted: &[Suggestion]| crate::suggest::apply(source, accepted);
    let check_annotations = |candidate: &[Suggestion]| {
        let after = compile(&annotated(candidate), "annotated")?;
        no_new_warning(&original, &after, "annotated")
            .map_err(|e| format!("{e} — the inference was wrong"))
    };
    let mut accepted = outcome.suggestions.clone();
    let mut rejected: Vec<(&Suggestion, String)> = Vec::new();
    if !accepted.is_empty() && check_annotations(&accepted).is_err() {
        // One wrong inference must not cost the other thirty. Re-admit the
        // suggestions one at a time, keeping each that still verifies, so the
        // offender is isolated and named instead of sinking the file.
        accepted = Vec::new();
        for s in &outcome.suggestions {
            let mut candidate = accepted.clone();
            candidate.push(s.clone());
            match check_annotations(&candidate) {
                Ok(()) => accepted = candidate,
                Err(e) => rejected.push((s, e)),
            }
        }
    }

    // Then the loop-tail return types, each on top of the accepted
    // annotations and of the return types already kept. One at a time: there
    // are few, and the two strengths have different proofs.
    let mut returns: Vec<ReturnType> = Vec::new();
    let mut unreturned: Vec<(&ReturnType, String)> = Vec::new();
    if !return_edits.is_empty() {
        let with_returns =
            |kept: &[ReturnType]| crate::suggest::apply_all(source, &accepted, kept, &[]);
        let mut base = match compile(&with_returns(&returns), "annotated") {
            Ok(c) => c,
            Err(e) => die_plain(&e),
        };
        for r in outcome.return_types.iter().filter(|r| r.is_edit()) {
            let mut candidate = returns.clone();
            candidate.push(r.clone());
            let checked = compile(&with_returns(&candidate), "rewritten").and_then(|after| {
                no_new_warning(&base, &after, "rewritten")?;
                // `-> list` only tells the checker what was already true.
                // `-> nil` changes the function on purpose, so there is no
                // IR to hold it to.
                if r.ty == Some("list") {
                    base.same_program(&after).map_err(|diff| {
                        format!(
                            "`-> list` changed the compiled program ({}: {} differs)",
                            diff.location, diff.what
                        )
                    })?;
                }
                Ok(after)
            });
            match checked {
                Ok(after) => {
                    returns = candidate;
                    base = after;
                }
                Err(e) => unreturned.push((r, e)),
            }
        }
    }

    // Then the named arguments, proven against the annotated source: the
    // only difference between the two sides is the names.
    let mut named = outcome.named_args.clone();
    let mut unproven: Vec<(&NamedArgs, String)> = Vec::new();
    if !named.is_empty() {
        let base = match compile(
            &crate::suggest::apply_all(source, &accepted, &returns, &[]),
            "annotated",
        ) {
            Ok(c) => c,
            Err(e) => die_plain(&e),
        };
        let check_named = |candidate: &[NamedArgs]| -> Result<(), String> {
            let rewritten = crate::suggest::apply_all(source, &accepted, &returns, candidate);
            let after = compile(&rewritten, "rewritten")?;
            base.same_program_modulo_named_args(&after)
                .map_err(|diff| crate::suggest::not_the_same_program(&diff))?;
            no_new_warning(&base, &after, "rewritten")
        };
        if check_named(&named).is_err() {
            named = Vec::new();
            for n in &outcome.named_args {
                let mut candidate = named.clone();
                candidate.push(n.clone());
                match check_named(&candidate) {
                    Ok(()) => named = candidate,
                    Err(e) => unproven.push((n, e)),
                }
            }
        }
    }

    for (s, why) in &rejected {
        eprintln!(
            "suggest: dropped `{}` on `{}` (line {}) — {why}",
            annotation(s),
            s.function.0,
            s.line
        );
    }
    for (r, why) in &unreturned {
        eprintln!(
            "suggest: dropped `->{}` on `{}` (line {}) — {why}",
            r.text.trim_start_matches(" ->"),
            r.function.0,
            r.line
        );
    }
    for (n, why) in &unproven {
        eprintln!(
            "suggest: dropped `{}` (line {}) — {why}",
            one_line(&n.after),
            n.line
        );
    }
    if !accepted.is_empty() {
        eprintln!(
            "verify: {name}: {} compile{} with no new type warning",
            count(accepted.len(), "type annotation"),
            if accepted.len() == 1 { "s" } else { "" }
        );
    }
    let lists = returns.iter().filter(|r| r.ty == Some("list")).count();
    if lists > 0 {
        eprintln!(
            "verify: {name}: {} proven IR-equal — the same program",
            count(lists, "`-> list` return type"),
        );
    }
    if returns.len() > lists {
        let nils = returns.len() - lists;
        eprintln!(
            "verify: {name}: {} compile{} with no new type warning — the function no \
             longer returns its loop's list, which no call in view reads",
            count(nils, "`-> nil` return type"),
            if nils == 1 { "s" } else { "" }
        );
    }
    if !named.is_empty() {
        eprintln!(
            "verify: {name}: {} proven IR-equal — the same program",
            count(named.len(), "named-argument rewrite"),
        );
    }

    let total = outcome.suggestions.len() + return_edits.len() + outcome.named_args.len();
    let kept = accepted.len() + returns.len() + named.len();
    if args.verify && !args.apply {
        if kept < total {
            eprintln!("verify: {} of {total} suggestions did not pass.", total - kept);
            process::exit(3);
        }
        return;
    }
    let rewritten = crate::suggest::apply_all(source, &accepted, &returns, &named);
    let SourceInput::File(path) = source_input else {
        // Inline code has nowhere to be written; print the result instead.
        print!("{rewritten}");
        return;
    };
    if kept == 0 {
        eprintln!("suggest: refusing to write {path}: no suggestion passed its proof");
        process::exit(3);
    }
    if let Err(e) = fs::write(path, &rewritten) {
        eprintln!("Error writing '{path}': {e}");
        process::exit(1);
    }
    println!("Applied {kept} of {total} to {path}.");
}

/// A suggestion's rewritten text on one line, for the report: runs of
/// whitespace that cross a line break collapse to a single space.
fn one_line(text: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(if i > 0 { line.trim() } else { line.trim_end() });
    }
    out
}

fn print_suggest_json(outcome: &crate::suggest::SuggestOutcome) {
    let mut items: Vec<(usize, serde_json::Value)> = outcome
        .suggestions
        .iter()
        .map(|s| {
            let slot = match s.slot {
                crate::typecheck::infer::Slot::Return => serde_json::json!("return"),
                crate::typecheck::infer::Slot::Param(i) => serde_json::json!(i),
            };
            let item = serde_json::json!({
                "kind": "type-annotation",
                "function": s.function.0,
                "arity": s.function.1,
                "slot": slot,
                "param": s.param,
                "type": s.ty,
                "line": s.line,
                "insert_at": s.at,
                "insert_text": s.text,
                "edits": [{ "insert_at": s.at, "insert_text": s.text }],
                "because": s.because,
                "evidence": s.evidence.iter().map(|e| serde_json::json!({
                    "kind": e.kind,
                    "detail": e.detail,
                    "line": e.line,
                })).collect::<Vec<_>>(),
            });
            (s.at, item)
        })
        .collect();
    items.extend(outcome.named_args.iter().map(|n| {
        let item = serde_json::json!({
            "kind": "named-args",
            "callee": n.callee,
            "line": n.line,
            "column": n.column,
            "names": n.names,
            "call": n.before,
            "rewrite": n.after,
            "edits": n.edits.iter().map(|e| serde_json::json!({
                "insert_at": e.at,
                "insert_text": e.text,
            })).collect::<Vec<_>>(),
            "because": n.because,
        });
        (n.edits.first().map_or(0, |e| e.at), item)
    }));
    // A loop tail's return type. `type` is null and `edits` empty when the
    // callers in view do not settle it; `options` is what the author would
    // choose between either way.
    items.extend(outcome.return_types.iter().map(|r| {
        let edits = match r.is_edit() {
            true => vec![serde_json::json!({ "insert_at": r.at, "insert_text": r.text })],
            false => Vec::new(),
        };
        let item = serde_json::json!({
            "kind": "return-type",
            "function": r.function.0,
            "arity": r.function.1,
            "line": r.line,
            "type": r.ty,
            "options": ["list", "nil"],
            "usage": r.usage.name(),
            "preserves_ir": r.ty != Some("nil"),
            "edits": edits,
            "because": r.because,
        });
        (r.at, item)
    }));
    // Advice has a place and a comment but no rewrite, so its `edits` is
    // empty: a consumer that applies edits blindly does nothing with it.
    items.extend(outcome.advice.iter().map(|a| {
        let item = serde_json::json!({
            "kind": "advice",
            "rule": a.rule,
            "subject": a.subject,
            "line": a.line,
            "column": a.column,
            "message": a.message,
            "edits": [],
        });
        (a.at, item)
    }));
    // One list, in source order, whatever the kind.
    items.sort_by_key(|(at, _)| *at);
    let items: Vec<serde_json::Value> = items.into_iter().map(|(_, item)| item).collect();
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "functions": outcome.functions,
            "notes": outcome.notes,
            "suggestions": items,
        })
    );
}
