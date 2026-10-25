//! `petal-ui-propose-edit` — `petal propose-edit` for scripts that `import ui`.
//!
//! ```text
//! petal-ui-propose-edit <app.ptl> [--size WxH] [--frames N] [--scenario s.json]
//!                       [--host-data fixtures.json] [-I <dir>] [--trace-emits]
//!                       <propose-edit flags>
//! ```
//!
//! The stock `petal propose-edit` has no petal-ui natives, so a script that
//! draws through `ui` (a HUD, a panel, the DM v2 examples ladder) cannot be
//! dragged with it. This binary loads the app the way `petal-ui-run` does
//! (petal-ui's natives and `ui` package, Garden's panel stubs, a fixed-`dt`
//! clock), runs `--frames` frames (default 1) with the optional input
//! `--scenario`, and takes the stack after the last one as the checkpoint.
//! From there it is the core command: the goal and filter flags are
//! `petal propose-edit`'s ([`petal::cli::dm_v2::V2Parser`]), the report is
//! the same text or `--json` document ([`petal::cli::dm_v2::propose`]), and
//! `--apply --pick N` writes the chosen alternative to the files.
//!
//! Draw commands are emitted on the channel `draw_commands`: the k-th
//! command drawn is `--channel draw_commands --emit k`, and `--native-arg 1`
//! is the drawing native's second argument (`fill_rect(x, y, w, h, …)`'s
//! `y`). `--trace-emits` prints every emit of the checkpoint's frame (per
//! channel: its key and raw native arguments) and exits, which is how to
//! find the index to drag.
//!
//! Exit codes: 0 a report (even one with no verified alternative), 1 the
//! proposal failed (the error is the report: `--json` prints the CLI's error
//! object), 2 a usage, load or frame error (message on stderr).

use std::path::PathBuf;

use petal::cli::dm_v2::{self, V2Parser};
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;
use serde_json::Value as Json;

const USAGE: &str = "usage: petal-ui-propose-edit <app.ptl> [--size WxH] [--frames N] [--scenario s.json] \
[--host-data fixtures.json] [-I <dir>] [--trace-emits] \
--channel <name> --emit <n> (--arg|--native-arg <k> | --value) [--field <path>] (--to <v> | --by <d>) \
[--allow S] [--deny S] [--tol T] [--alternatives N] [--explain] [--apply [--pick N]] [--seed N] [--json] …";

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("petal-ui-propose-edit: {e}");
            std::process::exit(2);
        }
    }
}

struct Args {
    app: PathBuf,
    size: (i32, i32),
    frames: usize,
    scenario: Option<PathBuf>,
    host_data: Option<PathBuf>,
    module_paths: Vec<PathBuf>,
    trace_emits: bool,
    parser: V2Parser,
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut app = None;
    let mut size = (800, 600);
    let mut frames = 1;
    let mut scenario = None;
    let mut host_data = None;
    let mut module_paths = Vec::new();
    let mut trace_emits = false;
    let mut parser = V2Parser::new();
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].clone();
        let value = |name: &str, i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i).cloned().ok_or_else(|| format!("{name} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--size" => size = parse_size(&value("--size", &mut i)?)?,
            "--frames" => {
                frames = value("--frames", &mut i)?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--frames needs a positive integer")?
            }
            "--scenario" => scenario = Some(PathBuf::from(value("--scenario", &mut i)?)),
            "--host-data" => host_data = Some(PathBuf::from(value("--host-data", &mut i)?)),
            "-I" | "--include" => module_paths.push(PathBuf::from(value("-I", &mut i)?)),
            "--trace-emits" => trace_emits = true,
            // Accepted for symmetry with `petal propose-edit`; v2 is the only engine here.
            "--v2" => {}
            _ if parser.flag(&argv, &mut i)? => {}
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
        i += 1;
    }
    Ok(Args {
        app: app.ok_or_else(|| format!("no script given\n{USAGE}"))?,
        size,
        frames,
        scenario,
        host_data,
        module_paths,
        trace_emits,
        parser,
    })
}

fn run() -> Result<i32, String> {
    let args = parse_args()?;
    // `--trace-emits` needs no goal; everything else does.
    let mut opts = if args.trace_emits {
        args.parser.opts.clone()
    } else {
        args.parser.finish()?
    };
    // Echoed in the report.
    opts.frames = args.frames;
    let seed = dm_v2::effective_seed(opts.seed);

    let mut ui = Headless::panel_from_file_with_paths(&args.app, args.size.0, args.size.1, &args.module_paths)?;
    ui.env.set_echo(false);
    ui.set_seed(seed);
    if let Some(path) = &args.host_data {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let json: Json =
            serde_json::from_str(&text).map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
        let provider =
            petal_ui::host_data::fixture_provider(&json).map_err(|e| format!("{}: {e}", path.display()))?;
        ui.set_data_provider(provider);
    }
    let scenario = match &args.scenario {
        None => None,
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("reading scenario {}: {e}", p.display()))?;
            Some(Scenario::from_json_str(&text).map_err(|e| format!("{}: {e}", p.display()))?)
        }
    };
    for frame in 0..args.frames {
        if let Some(s) = &scenario
            && let Some(id) = s.apply(&mut ui, frame).first()
        {
            return Err(format!("scenario: no ui target named \"{id}\" was registered before frame {frame}"));
        }
        ui.frame().map_err(|e| format!("frame {frame}: {e}"))?;
    }
    // The checkpoint: the stack after the last frame, ready to run the next.
    let (pid, sid) = (ui.program_id(), ui.stack_id());
    ui.env.reset_stack(sid)?;
    let (env, mut host) = ui.spec_parts();

    if args.trace_emits {
        let emits = dm_v2::baseline_emits(env, &mut host, sid, seed)
            .ok_or("the frame cannot be run speculatively (see `--explain` on a goal)")?;
        let mut by_channel = serde_json::Map::new();
        for (channel, e) in emits {
            let list = by_channel.entry(channel).or_insert_with(|| Json::Array(Vec::new()));
            if let Json::Array(xs) = list {
                xs.push(e);
            }
        }
        println!("{}", serde_json::to_string_pretty(&Json::Object(by_channel)).unwrap());
        return Ok(0);
    }

    match dm_v2::propose(env, &mut host, pid, sid, &opts, Some(args.app.clone())) {
        Ok(report) if opts.json => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
        Ok(report) => dm_v2::print_text(&report),
        Err(e) if opts.json => {
            println!("{}", serde_json::to_string_pretty(&dm_v2::error_json(&e)).unwrap());
            return Ok(1);
        }
        Err(e) => {
            eprintln!("petal-ui-propose-edit: {} ({})", e.message, e.phase);
            return Ok(1);
        }
    }
    Ok(0)
}

fn parse_size(s: &str) -> Result<(i32, i32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or_else(|| format!("--size wants WxH, got `{s}`"))?;
    let n = |t: &str| t.trim().parse::<i32>().map_err(|_| format!("--size wants WxH, got `{s}`"));
    Ok((n(w)?, n(h)?))
}
