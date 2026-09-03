//! `petal-ui-run` — drive a UI script headlessly and write a JSONL frame trace.
//!
//! ```text
//! petal-ui-run <app.ptl> [--size WxH] [--frames N] [--seed N]
//!              [--scenario s.json|monkey:<seed>] [--host-data fixtures.json]
//!              [--out trace.jsonl] [--error-format full|bare] [-I <dir>]
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
//! Exit codes: 0 clean, 1 a runtime error in some frame (its record is written
//! first, with `error` set), 2 a compile/usage error (message on stderr).

use serde_json::Value as Json;
use std::io::Write;
use std::path::{Path, PathBuf};

use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

const USAGE: &str = "usage: petal-ui-run <app.ptl> [--size WxH] [--frames N] [--seed N] \
[--scenario s.json|monkey:<seed>] [--host-data fixtures.json] [--out trace.jsonl] \
[--error-format full|bare] [-I <dir>]";

const DEFAULT_FRAMES: usize = 60;
const DEFAULT_SIZE: (i32, i32) = (800, 600);

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("petal-ui-run: {e}");
            std::process::exit(2);
        }
    }
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
}

fn parse_args() -> Result<Args, String> {
    let mut app: Option<PathBuf> = None;
    let mut size = None;
    let mut frames = None;
    let mut seed = None;
    let mut scenario = None;
    let mut host_data = None;
    let mut out = None;
    let mut bare_errors = false;
    let mut module_paths: Vec<PathBuf> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| -> Result<String, String> {
            it.next()
                .ok_or_else(|| format!("{name} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
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
            "--error-format" => {
                bare_errors = match value("--error-format")?.as_str() {
                    "bare" => true,
                    "full" => false,
                    other => return Err(format!("unknown --error-format `{other}`")),
                }
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown flag `{other}`\n{USAGE}"));
            }
            other => {
                if app.is_some() {
                    return Err(format!("unexpected argument `{other}`\n{USAGE}"));
                }
                app = Some(PathBuf::from(other));
            }
        }
    }
    Ok(Args {
        app: app.ok_or_else(|| format!("no script given\n{USAGE}"))?,
        size,
        frames,
        seed,
        scenario,
        host_data,
        out,
        bare_errors,
        module_paths,
    })
}

fn run() -> Result<i32, String> {
    let args = parse_args()?;

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

    let mut ui = Headless::from_file_with_paths(&args.app, size.0, size.1, &args.module_paths)?;
    // Garden-panel host natives (`palette`, `query`, `text_view`, …) answer
    // as inert, deterministic stubs, so panel drawers run headlessly instead
    // of dying at `Unknown builtin` (see [`petal_ui::panel_stubs`]).
    petal_ui::panel_stubs::register_panel_stubs(&mut ui.env);
    // Prints belong in the trace's `prints` field and nowhere else: echoing
    // them to stdout as well would interleave them with the JSONL.
    ui.env.set_echo(false);
    if let Some(seed) = args.seed {
        ui.env.set_seed(seed);
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
    for frame in 0..frames {
        if let Some(s) = &scenario {
            s.apply(&mut ui, frame);
        }
        let outcome = ui.frame().map(|_| ());
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
        let record = serde_json::json!({
            "frame": frame,
            "commands": commands,
            "state": ui.state(),
            "prints": prints,
            "result": result,
            "error": error,
        });
        writeln!(out, "{record}").map_err(|e| format!("writing trace: {e}"))?;
        if failed {
            break;
        }
    }
    out.flush().map_err(|e| format!("writing trace: {e}"))?;
    Ok(if failed { 1 } else { 0 })
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
