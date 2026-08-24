//! `petal-ui-run` — drive a UI script headlessly and write a JSONL frame trace.
//!
//! ```text
//! petal-ui-run <app.ptl> [--size WxH] [--frames N] [--seed N]
//!              [--scenario s.json|monkey:<seed>] [--host-data fixtures.json]
//!              [--out trace.jsonl] [--error-format full|bare]
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
use petal_ui::host_data::HostData;
use petal_ui::scenario::Scenario;

const USAGE: &str = "usage: petal-ui-run <app.ptl> [--size WxH] [--frames N] [--seed N] \
[--scenario s.json|monkey:<seed>] [--host-data fixtures.json] [--out trace.jsonl] \
[--error-format full|bare]";

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
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        app: PathBuf::new(),
        size: None,
        frames: None,
        seed: None,
        scenario: None,
        host_data: None,
        out: None,
        bare_errors: false,
    };
    let mut app: Option<PathBuf> = None;
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
            "--size" => args.size = Some(parse_size(&value("--size")?)?),
            "--frames" => {
                args.frames = Some(
                    value("--frames")?
                        .parse()
                        .map_err(|_| "--frames needs an integer".to_string())?,
                )
            }
            "--seed" => {
                args.seed = Some(
                    value("--seed")?
                        .parse()
                        .map_err(|_| "--seed needs an integer".to_string())?,
                )
            }
            "--scenario" => args.scenario = Some(value("--scenario")?),
            "--host-data" => args.host_data = Some(PathBuf::from(value("--host-data")?)),
            "--out" => args.out = Some(PathBuf::from(value("--out")?)),
            "--error-format" => {
                args.bare_errors = match value("--error-format")?.as_str() {
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
    args.app = app.ok_or_else(|| format!("no script given\n{USAGE}"))?;
    Ok(args)
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

    let mut ui = Headless::from_file_with_size(&args.app, size.0, size.1)?;
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
        let record = match outcome {
            Ok(()) => {
                let result = petal::value::value_to_json(&ui.result, ui.env.heap());
                serde_json::json!({
                    "frame": frame,
                    "commands": ui.commands,
                    "state": ui.state(),
                    "prints": prints,
                    "result": result,
                    "error": Json::Null,
                })
            }
            Err(e) => {
                failed = true;
                let message = if args.bare_errors { bare_error(&e) } else { e };
                serde_json::json!({
                    "frame": frame,
                    "commands": [],
                    "state": ui.state(),
                    "prints": prints,
                    "result": Json::Null,
                    "error": message,
                })
            }
        };
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

/// Build a `host_data` provider from a fixture file: a JSON array of
/// `{"kind": ..., "arg": ..., "value": ...}` answers. A `(kind, arg)` with no
/// entry answers nil, exactly as a host with no data for the question would.
fn fixture_provider(path: &Path) -> Result<petal_ui::host_data::DataProvider, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: Json = serde_json::from_str(&text)
        .map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
    let entries = json
        .as_array()
        .ok_or_else(|| format!("{}: fixtures must be a JSON array", path.display()))?;
    let mut table: Vec<((String, String), HostData)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let obj = e
            .as_object()
            .ok_or_else(|| format!("{}: fixture {i} must be an object", path.display()))?;
        let as_key = |v: Option<&Json>| match v {
            None | Some(Json::Null) => String::new(),
            Some(Json::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        };
        let kind = as_key(obj.get("kind"));
        let arg = as_key(obj.get("arg"));
        let value = json_to_host_data(obj.get("value").unwrap_or(&Json::Null));
        table.push(((kind, arg), value));
    }
    Ok(Box::new(move |kind: &str, arg: &str| {
        table
            .iter()
            .find(|((k, a), _)| k == kind && a == arg)
            .map(|(_, v)| v.clone())
            .unwrap_or(HostData::Nil)
    }))
}

/// JSON → [`HostData`], keeping the int/float distinction the source made
/// (a truncated `0.42` is unrecoverable downstream — see `host_data.rs`).
fn json_to_host_data(v: &Json) -> HostData {
    match v {
        Json::Null => HostData::Nil,
        Json::Bool(b) => HostData::Bool(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => HostData::Int(i),
            None => HostData::Float(n.as_f64().unwrap_or(0.0)),
        },
        Json::String(s) => HostData::Str(s.clone()),
        Json::Array(items) => HostData::List(items.iter().map(json_to_host_data).collect()),
        Json::Object(fields) => HostData::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), json_to_host_data(v)))
                .collect(),
        ),
    }
}
