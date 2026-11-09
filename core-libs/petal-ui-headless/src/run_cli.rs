//! `petal-ui-run` — drive a UI script headlessly and write a JSONL frame trace.
//!
//! This is the library half: [`main`] and [`run`] take the program name and a
//! [`CliHost`], so a host ships the same CLI with its natives registered
//! (Garden's `garden-panel-run`, which adds its panel stubs and
//! `--query-fixtures`). The `petal-ui-run` binary is [`main`] over
//! [`crate::NoHost`]: petal-ui's natives only, so a script that does
//! `import garden` fails to load there.
//!
//! ```text
//! petal-ui-run <app.ptl> [--size WxH] [--frames N] [--seed N]
//!              [--scenario s.json|monkey:<seed>] [--host-data fixtures.json]
//!              [--out trace.jsonl] [--error-format full|bare] [-I <dir>]
//!              [--policy <name>] [--no-gate] [--gate-stats] [--no-memo] [--memo-stats] [--targets]
//!              [--effect-audit] [--static-audit] [--static-only]
//!              [--drag-trace] [--drag-trace-full] [--ui-debug <name>]
//! ```
//!
//! One JSON object per line, one line per frame:
//!
//! ```json
//! {"frame": 12, "commands": [...], "state": {...}, "prints": [...],
//!  "result": null, "error": null}
//! ```
//!
//! Every source of nondeterminism the driver controls is pinned: a fixed dt
//! and clock (the harness's), a scripted input scenario, a seeded RNG, and a
//! fixture-backed `host_data`. Two runs with the same arguments must produce
//! byte-identical output — that is the property the refactor verifier builds
//! on (see `docs/dev/refactor-verification.md`).
//!
//! Frames run under a run policy (`petal::policy`), `fast` unless `--policy`
//! or `PETAL_POLICY` names another. Under `fast`, as in every real host, a
//! frame whose inputs are what the last run read is skipped by the frame gate
//! and its record carries the retained commands and state (with no prints,
//! since nothing printed); and within a frame that runs, user-function calls
//! are memoized (see docs/dev/memo-scopes.md). Every policy must produce the
//! same trace, so a differential is two runs with two names:
//! `--policy baseline` (nothing skipped, replayed or optimized) against
//! `--policy fast`, or `fast-memo` / `replay` to isolate the gate / the memo.
//! `--no-gate` and `--no-memo` switch one layer off in whichever policy is in
//! effect. `--gate-stats` reports frames run vs skipped on stderr, and
//! `--memo-stats` the memo's counters.
//!
//! `--targets` adds a `targets` list to every record: the rects the script
//! registered on that frame (`interact(r, id)`, the prelude widgets,
//! `ui_target`), with their ids — what a scenario's `click_target` can name.
//!
//! `--drag-trace` adds a `drag` record to every frame: each `ui/drag` drag in
//! flight, explained (its target table with each target's distance from the
//! dragged element, the chosen target and the branch of the spec down to it;
//! see [`crate::drag_trace`]), and the drops announced that frame. A line
//! per drag frame also goes to stderr. `--drag-trace-full` adds the spec as
//! built, the base model and every target's drop state (what the
//! `DragExplain` MCP tool reads). `--ui-debug <name>` switches on a
//! debug overlay as a host's debug key would (`ui_debug(name)`; `drag` draws
//! `ui/drag`'s `drag_debug` into the trace).
//!
//! `--effect-audit` watches what every native does over the run and reports,
//! on stderr, each one whose behavior differs from its declared effect row
//! (see `petal::effect_audit`): an under-declared row is a staleness bug
//! the memo cannot see; an over-declared facet is one this run never took.
//!
//! `--static-audit` watches the dynamic inputs (bindings, host data,
//! resources, output, effects, the random stream, `state`, any native not
//! registered constant) while each top-level term the classifier calls
//! static is evaluated, on every frame, and reports each one touched (see
//! `petal::static_audit`). `--static-only` evaluates only those statements:
//! the frames draw nothing, and an error is a failure of the build.
//!
//! Exit codes: 0 clean, 1 a runtime error in some frame (its record is written
//! first, with `error` set), 2 a compile/usage error (message on stderr), 3 a
//! clean run in which `--effect-audit` found an under-declared native, 4 a
//! clean run in which the static audit found a static term touching a dynamic
//! input.

use serde_json::Value as Json;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::CliHost;
use crate::harness::Headless;
use crate::scenario::Scenario;

const USAGE: &str = "<app.ptl> [--size WxH] [--frames N] [--seed N] \
[--scenario s.json|monkey:<seed>] [--host-data fixtures.json] \
[--out trace.jsonl] [--error-format full|bare] [-I <dir>] [--policy <name>] [--no-gate] [--gate-stats] [--no-memo] [--memo-stats] [--targets] [--effect-audit] [--static-audit] [--static-only] [--drag-trace] [--drag-trace-full] [--ui-debug <name>]";

const DEFAULT_FRAMES: usize = 60;
const DEFAULT_SIZE: (i32, i32) = (800, 600);

/// Run the CLI over this process's arguments as `name`, and exit with its
/// code (2 with the message on stderr when [`run`] fails).
pub fn main(name: &str, host: &mut dyn CliHost) -> ! {
    match run(name, host, std::env::args().skip(1).collect()) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{name}: {e}");
            std::process::exit(2);
        }
    }
}

fn usage(name: &str, host: &dyn CliHost) -> String {
    let extra = host.usage();
    let sep = if extra.is_empty() { "" } else { " " };
    format!("usage: {name} {USAGE}{sep}{extra}")
}

struct Args {
    app: PathBuf,
    size: Option<(i32, i32)>,
    frames: Option<usize>,
    seed: Option<u64>,
    scenario: Option<String>,
    host_data: Option<PathBuf>,
    out: Option<PathBuf>,
    bare_errors: bool,
    /// Extra module search directories (`-I`), for an app that imports a
    /// shared Petal library from outside its own directory.
    module_paths: Vec<PathBuf>,
    /// `--policy`; `None` keeps the env's (from `PETAL_POLICY`, else `fast`).
    policy: Option<petal::policy::RunPolicy>,
    /// Run the script every frame instead of letting the frame gate skip.
    no_gate: bool,
    /// Run every user-function call instead of replaying memoized ones.
    no_memo: bool,
    /// Report the memo's counters on stderr.
    memo_stats: bool,
    /// Add each frame's registered ui targets to its record.
    targets: bool,
    /// Report frames run vs skipped on stderr at the end.
    gate_stats: bool,
    /// Audit every native's observed behavior against its declared row and
    /// report the differences on stderr at the end.
    effect_audit: bool,
    static_audit: petal::static_audit::StaticAuditMode,
    /// Add each frame's `ui/drag` drags and drops to its record: 0 off, 1
    /// the table, 2 (`--drag-trace-full`) with the spec and the drop states.
    drag_trace: u8,
    /// Debug overlays to switch on (`ui_debug(name)`).
    ui_debug: Vec<String>,
}

fn parse_args(name: &str, host: &mut dyn CliHost, argv: Vec<String>) -> Result<Args, String> {
    let usage = usage(name, host);
    let mut app: Option<PathBuf> = None;
    let mut size = None;
    let mut frames = None;
    let mut seed = None;
    let mut scenario = None;
    let mut host_data = None;
    let mut out = None;
    let mut bare_errors = false;
    let mut module_paths: Vec<PathBuf> = Vec::new();
    let mut policy = None;
    let mut no_gate = false;
    let mut gate_stats = false;
    let mut no_memo = false;
    let mut memo_stats = false;
    let mut targets = false;
    let mut effect_audit = false;
    let mut static_audit = petal::static_audit::StaticAuditMode::Off;
    // 0 off, 1 the per-frame table, 2 with the spec and every target's drop.
    let mut drag_trace = 0u8;
    let mut ui_debug = Vec::new();
    let mut it = argv.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| -> Result<String, String> {
            it.next()
                .ok_or_else(|| format!("{name} needs a value\n{usage}"))
        };
        match a.as_str() {
            "-h" | "--help" => {
                println!("{usage}");
                std::process::exit(0);
            }
            "--size" => size = Some(parse_size(&value("--size")?)?),
            "--frames" => {
                frames = Some(
                    value("--frames")?
                        .parse()
                        .map_err(|_| "--frames needs an integer".to_string())?,
                )
            }
            "--seed" => {
                seed = Some(
                    value("--seed")?
                        .parse()
                        .map_err(|_| "--seed needs an integer".to_string())?,
                )
            }
            "--scenario" => scenario = Some(value("--scenario")?),
            "-I" | "--include" => module_paths.push(PathBuf::from(value("-I")?)),
            "--host-data" => host_data = Some(PathBuf::from(value("--host-data")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--policy" => policy = Some(petal::policy::RunPolicy::parse(&value("--policy")?)?),
            "--no-gate" => no_gate = true,
            "--gate-stats" => gate_stats = true,
            "--no-memo" => no_memo = true,
            "--memo-stats" => memo_stats = true,
            "--targets" => targets = true,
            "--effect-audit" => effect_audit = true,
            "--drag-trace" => drag_trace = drag_trace.max(1),
            "--drag-trace-full" => drag_trace = 2,
            "--ui-debug" => ui_debug.push(value("--ui-debug")?),
            "--static-audit" => {
                if static_audit == petal::static_audit::StaticAuditMode::Off {
                    static_audit = petal::static_audit::StaticAuditMode::Watch;
                }
            }
            "--static-only" => static_audit = petal::static_audit::StaticAuditMode::StaticOnly,
            "--error-format" => {
                bare_errors = match value("--error-format")?.as_str() {
                    "bare" => true,
                    "full" => false,
                    other => return Err(format!("unknown --error-format `{other}`")),
                }
            }
            other if other.starts_with('-') && other != "-" => {
                let mut value = || value(other);
                if !host.flag(other, &mut value)? {
                    return Err(format!("unknown flag `{other}`\n{usage}"));
                }
            }
            other => {
                if app.is_some() {
                    return Err(format!("unexpected argument `{other}`\n{usage}"));
                }
                app = Some(PathBuf::from(other));
            }
        }
    }
    Ok(Args {
        app: app.ok_or_else(|| format!("no script given\n{usage}"))?,
        size,
        frames,
        seed,
        scenario,
        host_data,
        out,
        bare_errors,
        module_paths,
        policy,
        no_gate,
        gate_stats,
        no_memo,
        memo_stats,
        targets,
        effect_audit,
        static_audit,
        drag_trace,
        ui_debug,
    })
}

/// Run the CLI over `argv` (the arguments after the program name) as `name`:
/// the trace goes to stdout or `--out`, and the result is the exit code.
/// `Err` is a usage or load error (exit code 2).
pub fn run(name: &str, host: &mut dyn CliHost, argv: Vec<String>) -> Result<i32, String> {
    let args = parse_args(name, host, argv)?;

    let scenario = match args.scenario.as_deref() {
        None => None,
        Some(spec) => Some(load_scenario(spec, &args)?),
    };
    let size = args
        .size
        .or_else(|| scenario.as_ref().and_then(|s| s.size))
        .unwrap_or(DEFAULT_SIZE);
    let frames = args
        .frames
        .or_else(|| scenario.as_ref().and_then(|s| s.frames))
        .unwrap_or(DEFAULT_FRAMES);

    // The host's natives (Garden's panel stubs, for `garden-panel-run`)
    // register before the script loads.
    let mut ui = Headless::from_file_with_setup(&args.app, size.0, size.1, &args.module_paths, |env| {
        host.setup(env)
    })?;
    // What `petal lint` says about the app, on stderr beside the trace.
    eprint!(
        "{}",
        ui.core().env.diagnostics_text(ui.core().program_id())
    );

    // Prints belong in the trace's `prints` field and nowhere else: echoing
    // them to stdout as well would interleave them with the JSONL.
    ui.env.set_echo(false);
    let mut policy = args.policy.unwrap_or_else(|| ui.policy());
    if args.no_gate {
        policy.gate = false;
    }
    if args.no_memo {
        policy.memo = false;
    }
    ui.set_policy(policy);
    if args.effect_audit {
        ui.env.set_effect_audit(true);
    }
    let static_audit_on = args.static_audit != petal::static_audit::StaticAuditMode::Off;
    if static_audit_on {
        ui.env.set_static_audit(args.static_audit);
    }
    if let Some(seed) = args.seed {
        ui.env.set_seed(seed);
    }
    for name in &args.ui_debug {
        ui.input.set_debug(name, true);
    }
    if let Some(path) = &args.host_data {
        ui.set_data_provider(fixture_provider(path)?);
    }

    let mut out: Box<dyn Write> = match &args.out {
        None => Box::new(std::io::stdout()),
        Some(p) if p.as_os_str() == "-" => Box::new(std::io::stdout()),
        Some(p) => Box::new(std::io::BufWriter::new(
            std::fs::File::create(p).map_err(|e| format!("{}: {e}", p.display()))?,
        )),
    };

    let mut failed = false;
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    for frame in 0..frames {
        // A scenario that clicks a target the app did not register is a
        // failed frame, like a script error: the run stops there with the
        // reason in the record, rather than driving on having clicked nothing.
        let missing = match &scenario {
            Some(s) => s.apply(&mut ui, frame),
            None => Vec::new(),
        };
        let outcome = match missing.first() {
            Some(id) => Err(format!(
                "scenario: no ui target named \"{id}\" was registered on the frame before frame {frame}"
            )),
            None => ui.frame().map(|_| ()),
        };
        if args.gate_stats
            && let Some(reason) = &ui.last_run_reason
        {
            let label = match reason {
                petal::run_deps::RunReason::BindingChanged(sym) => {
                    format!("binding {}", ui.env.symbol_name(*sym).unwrap_or("?"))
                }
                other => format!("{other:?}"),
            };
            *reasons.entry(label).or_default() += 1;
        }
        // `print` output is drained per frame, so each record holds only what
        // that frame printed — including the frame that failed.
        let prints = ui.env.take_output();
        // The success and failure records differ only in these three fields.
        let (commands, result, error) = match outcome {
            Ok(()) => (
                serde_json::to_value(&ui.commands).unwrap(),
                petal::value::value_to_json(&ui.result, ui.env.heap()),
                Json::Null,
            ),
            Err(e) => {
                failed = true;
                let message = if args.bare_errors { bare_error(&e) } else { e };
                (Json::Array(Vec::new()), Json::Null, Json::String(message))
            }
        };
        let mut record = serde_json::json!({
            "frame": frame,
            "commands": commands,
            "state": ui.state(),
            "prints": prints,
            "result": result,
            "error": error,
        });
        // Drops are announced on an output channel nothing else drains.
        let drops = crate::drag_trace::take_drops(&mut ui.env);
        if args.drag_trace > 0 {
            let pointer = (ui.input.mouse_x, ui.input.mouse_y);
            let full = args.drag_trace > 1;
            let sessions = crate::drag_trace::explain_all(&ui.state(), pointer, full);
            for s in &sessions {
                eprintln!("{name}: frame {frame}: {}", crate::drag_trace::summary(s));
            }
            for d in &drops {
                eprintln!("{name}: frame {frame}: drop {} -> {}", d["key"], d["drop"]);
            }
            record["drag"] = if sessions.is_empty() && drops.is_empty() {
                Json::Null
            } else {
                serde_json::json!({"sessions": sessions, "drops": drops})
            };
        }
        if args.targets {
            // What `interact`, the widgets and `ui_target` registered on this
            // frame: what a `click_target` on the next frame can name.
            let targets: Vec<Json> = ui
                .targets()
                .into_iter()
                .filter(|t| !t.modal)
                .map(|t| {
                    serde_json::json!({
                        "id": t.id, "x": t.x, "y": t.y, "w": t.w, "h": t.h,
                        "layer": t.layer, "blocks": t.blocks,
                    })
                })
                .collect();
            record["targets"] = Json::Array(targets);
        }
        writeln!(out, "{record}").map_err(|e| format!("writing trace: {e}"))?;
        if failed {
            break;
        }
    }
    out.flush().map_err(|e| format!("writing trace: {e}"))?;
    if args.gate_stats {
        eprintln!(
            "{name}: frames run {} skipped {}",
            ui.frames_run, ui.frames_skipped
        );
        for (reason, n) in &reasons {
            eprintln!("  {n:>5}  {reason}");
        }
    }
    if args.memo_stats {
        let m = ui.memo_stats();
        eprintln!(
            "{name}: memo hits {} misses {} records {} inlined {} effectful {} reexecs {} cutoffs {} cold {} evicted {} static-skips {} static-args {} commands {} slots {}",
            m.hits,
            m.misses,
            m.records,
            m.inlined,
            m.effectful,
            m.reexecs,
            m.cutoffs,
            m.cold,
            m.evicted,
            m.static_skips,
            m.static_args,
            m.commands,
            ui.env.memo_slots(ui.stack_id()),
        );
    }
    let mut under_declared = false;
    if args.effect_audit {
        let report = ui.env.effect_audit_report();
        eprint!("{name}: {report}");
        under_declared = report.has_under_declared();
    }
    let mut static_failed = false;
    if static_audit_on {
        let report = ui.env.static_audit_report();
        eprint!("{name}: {report}");
        static_failed = report.has_failures();
    }
    Ok(if failed {
        1
    } else if under_declared {
        3
    } else if static_failed {
        4
    } else {
        0
    })
}

fn parse_size(s: &str) -> Result<(i32, i32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("--size wants WxH, got `{s}`"))?;
    let n = |t: &str| {
        t.trim()
            .parse::<i32>()
            .map_err(|_| format!("--size wants WxH, got `{s}`"))
    };
    Ok((n(w)?, n(h)?))
}

/// `--scenario` is either a path to a JSON file or `monkey:<seed>`, which
/// generates one over the resolved size and frame count.
fn load_scenario(spec: &str, args: &Args) -> Result<Scenario, String> {
    if let Some(seed) = spec.strip_prefix("monkey:") {
        let seed: u64 = seed
            .parse()
            .map_err(|_| format!("monkey scenario wants a u64 seed, got `{seed}`"))?;
        let size = args.size.unwrap_or(DEFAULT_SIZE);
        let frames = args.frames.unwrap_or(DEFAULT_FRAMES);
        return Ok(Scenario::monkey(seed, frames, size));
    }
    let text =
        std::fs::read_to_string(spec).map_err(|e| format!("reading scenario {spec}: {e}"))?;
    Scenario::from_json_str(&text).map_err(|e| format!("{spec}: {e}"))
}

/// Strip everything position-dependent from a runtime error: the
/// `[line N, column M]` suffixes and the echoed source snippet. What is left
/// is the message and its provenance, which a re-indenting refactor cannot
/// change — the point of `--error-format bare`.
fn bare_error(msg: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in msg.lines() {
        let t = line.trim_start();
        // Snippet lines: "  |", "2 | print(x)", "  |    ^^^".
        if t == "|" || t.starts_with("| ") {
            continue;
        }
        if let Some((num, rest)) = t.split_once(" | ")
            && num.chars().all(|c| c.is_ascii_digit())
            && !num.is_empty()
        {
            let _ = rest;
            continue;
        }
        out.push(strip_position(line).to_string());
    }
    out.join("\n")
}

/// Drop a trailing `[line N, column M]` or `[file.ptl line N, column M]`.
fn strip_position(line: &str) -> &str {
    let Some(open) = line.rfind(" [") else {
        return line;
    };
    let body = &line[open + 2..];
    let Some(close) = body.find(']') else {
        return line;
    };
    let body = &body[..close];
    if body.contains("line ") && body.ends_with(|c: char| c.is_ascii_digit()) {
        line[..open].trim_end()
    } else {
        line
    }
}

/// Build a `host_data` provider from a fixture file (see
/// [`petal_ui::host_data::fixture_provider`] for the format). This wrapper
/// only reads the file and prefixes errors with its path.
fn fixture_provider(path: &Path) -> Result<petal_ui::host_data::DataProvider, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: Json = serde_json::from_str(&text)
        .map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
    petal_ui::host_data::fixture_provider(&json).map_err(|e| format!("{}: {e}", path.display()))
}
