//! Garden application crate: [`run`] parses the command line, loads the layout
//! script (or builds a plain-file fallback layout), and hands the thread to the
//! chosen frontend — windowed (default), terminal (`--term`), or headless
//! (`--headless`). See `src/frontend/` for the frontend interface and
//! `src/app.rs` for the frontend-independent core. The `garden` binary
//! (`src/main.rs`) is a thin wrapper that just calls [`run`].

mod app;
mod clipboard;
mod command_line;
mod debug;
mod editor_view;
mod event_log;
mod file_finder;
mod frontend;
mod layout;
mod lsp;
mod panel_tess;
mod panel_view;
mod petal_ide;
mod process_pane;
mod script_client;
mod search;
mod setup;
mod state;
mod syntax;
mod theme;
mod vim;
mod window_nav;

use std::path::{Path, PathBuf};

use garden_script::{LayoutNode, ScriptHost};

use frontend::{AppConfig, Frontend};

enum Mode {
    Window,
    Headless,
    Terminal,
}

/// The whole `garden` command-line entry point: parse arguments, resolve the
/// layout (subcommands, files, or the init script), and hand the thread to the
/// chosen frontend. Exits the process directly on usage errors (`exit(2)`),
/// script-load failures, and frontend errors (`exit(1)`) — exactly the binary's
/// behavior, just callable from the library target.
pub fn run() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();

    // `garden setup …` is an administrative subcommand (seed/reset ~/.garden)
    // that never opens an editor; handle it before any frontend wiring.
    if raw_args.first().map(String::as_str) == Some("setup") {
        std::process::exit(setup::run(&raw_args[1..]));
    }

    let mut mode = Mode::Window;
    let mut debug_port: Option<u16> = None;
    let mut init_override: Option<PathBuf> = None;
    let mut positionals: Vec<String> = Vec::new();
    // `--subprocess <cmd> [args…]`: everything after the flag is the GPP client
    // command line, so it must come last (put garden's own flags before it).
    let mut subprocess: Option<Vec<String>> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--subprocess" => {
                let rest: Vec<String> = args.by_ref().collect();
                if rest.is_empty() {
                    eprintln!("garden: --subprocess requires a command (e.g. --subprocess sqlite-browser <arg>)");
                    std::process::exit(2);
                }
                subprocess = Some(rest);
            }
            "--headless" => mode = Mode::Headless,
            "--term" | "--terminal" => mode = Mode::Terminal,
            "--init" => match args.next() {
                Some(path) => init_override = Some(PathBuf::from(path)),
                None => {
                    eprintln!("garden: --init requires a path to a layout script");
                    std::process::exit(2);
                }
            },
            "--debug-port" => {
                let value = args.next().and_then(|v| v.parse().ok());
                match value {
                    Some(port) => debug_port = Some(port),
                    None => {
                        eprintln!("garden: --debug-port requires a port number");
                        std::process::exit(2);
                    }
                }
            }
            // `--stat` is a `diff`-subcommand flag, not a global option; pass it
            // through to the positionals so `resolve_diff_subcommand` can see it
            // wherever it appears (e.g. `garden diff --stat HEAD --headless`).
            "--stat" => positionals.push("--stat".to_string()),
            // `--local` (alias `--diff`) is a `pr`-subcommand flag that reviews
            // the local `git diff <base>` instead of resolving a GitHub PR — no
            // `gh`, no network. Passed through as a positional (like `--stat`) so
            // `resolve_pr_subcommand` can see it.
            "--local" | "--diff" => positionals.push("--local".to_string()),
            other if !other.starts_with('-') => positionals.push(other.to_string()),
            other => {
                eprintln!("garden: unknown option {other}");
                print_usage();
                std::process::exit(2);
            }
        }
    }

    // `garden git <subcommand>` opens a git view directly — e.g. `garden git
    // log` opens the history browser, the CLI counterpart of the `:Git` ex
    // command. Unlike `setup` it opens a normal editor window, so it feeds a
    // fallback layout into the usual frontend wiring rather than exiting.
    //
    // `garden open <file or directory>` is the unambiguous form of `garden
    // <file>`: everything after `open` is treated as a path, never a subcommand,
    // so you can open a file literally named `git` or `setup`.
    // `--subprocess <cmd> [args…]` launches an arbitrary GPP client as the whole
    // layout — the generic form of `garden git`/`diff`/`pr` for any panel- or
    // lines-mode app (e.g. `garden --subprocess sqlite-browser <db.sqlite>`), including
    // out-of-tree clients passed by path.
    // It wins over positionals since it *is* the layout.
    let (mut script, fallback_layout, script_owns_layout) = if let Some(cmd_args) = subprocess {
        let command = process_pane::resolve_client_bin(&cmd_args[0]);
        let node = LayoutNode::Process {
            command,
            args: cmd_args[1..].to_vec(),
        };
        (None, node, false)
    } else {
        match positionals.first().map(String::as_str) {
            Some("git") => (None, resolve_git_subcommand(&positionals[1..]), false),
            Some("diff") => (None, resolve_diff_subcommand(&positionals[1..]), false),
            Some("pr") => (None, resolve_pr_subcommand(&positionals[1..]), false),
            Some("petal-ide") => (None, resolve_petal_ide_subcommand(&positionals[1..]), false),
            Some("open") => resolve_layout(&positionals[1..], init_override.as_deref()),
            // Any builtin GPP client by its raw name: `garden sqlite-browser …`,
            // `garden directory-browser …`, etc. — no `--subprocess` flag needed.
            Some(cmd) if BUILTIN_GPP_APPS.contains(&cmd) => {
                (None, gpp_app_layout(cmd, &positionals[1..]), false)
            }
            _ => resolve_layout(&positionals, init_override.as_deref()),
        }
    };

    // Allocate this window's state once, up front: a unique window id plus the
    // event log it appends to. The id also names the per-window layout overlay,
    // so a loaded script is pointed at it here (replacing the script-relative
    // sibling fallback). Best-effort — a state hiccup leaves both `None`, and
    // the editor still launches with logging and per-window persistence off.
    let event_log = attach_window_state(script.as_mut());

    // Petal-IDE default (scratch) mode protects the scratch file: saving prompts
    // for a filename rather than overwriting it (see App::save_as_paths). An
    // explicit `petal-ide <file>` is a normal file and saves in place.
    let save_as_paths: std::collections::HashSet<PathBuf> =
        if positionals.first().map(String::as_str) == Some("petal-ide") && positionals.len() == 1 {
            [petal_ide_path(None)].into_iter().collect()
        } else {
            std::collections::HashSet::new()
        };

    // Petal-IDE mode: turn on the toolbar / play-pause / IR inspector, with the
    // subcommand's target as the program the IR panel inspects. The IR-inspector
    // drawer is seeded to disk (like the scratch sketch) so its pane is a normal
    // `panel(path)` node that round-trips through layout persistence.
    let ide_target: Option<PathBuf> =
        if positionals.first().map(String::as_str) == Some("petal-ide") {
            seed_petal_ide_ir_view();
            Some(petal_ide_path(positionals.get(1).map(String::as_str)))
        } else {
            None
        };

    let config = AppConfig {
        script,
        fallback_layout,
        script_owns_layout,
        debug_port,
        event_log,
        save_as_paths,
        ide_target,
    };

    let frontend: Box<dyn Frontend> = match mode {
        Mode::Window => Box::new(frontend::window::WindowFrontend),
        Mode::Headless => Box::new(frontend::headless::HeadlessFrontend),
        Mode::Terminal => Box::new(frontend::terminal::TerminalFrontend),
    };
    if let Err(err) = frontend.run(config) {
        eprintln!("garden: {err}");
        std::process::exit(1);
    }
}

/// Build the [`AppConfig`] for a runtime-spawned window (`:windownew`, File ▸
/// New Window) — what a bare `garden` launch (no args, no subcommand) gets:
/// the default `~/.garden/init.ptl` owning the layout when present, over an
/// empty-editor fallback. Every window loads its **own** [`ScriptHost`] (one
/// Petal `Env` per window) and mints a **fresh window id** — its own layout
/// overlay and event log — via [`attach_window_state`]. The debug server is
/// process-global, so `debug_port` is always `None` here.
///
/// Unlike launch-time resolution ([`resolve_layout`]), a broken init script
/// must not kill the process — other windows are live — so a load failure
/// degrades to a script-less empty editor with a warning.
pub(crate) fn new_window_config() -> AppConfig {
    let empty = LayoutNode::Editor {
        file: None,
        line_numbers: false,
        wrap: true,
    };
    let (mut script, script_owns_layout) = match default_config_script() {
        Some(init) => match load_script(&init) {
            Ok(host) => (Some(host), true),
            Err(err) => {
                eprintln!(
                    "garden: new window: failed to load {} ({err}); opening an empty editor",
                    init.display()
                );
                (None, false)
            }
        },
        None => (None, false),
    };
    let event_log = attach_window_state(script.as_mut());
    AppConfig {
        script,
        fallback_layout: empty,
        script_owns_layout,
        debug_port: None,
        event_log,
        save_as_paths: std::collections::HashSet::new(),
        ide_target: None,
    }
}

/// Resolve positional arguments into a layout source. The returned `bool` is
/// whether the script (if any) **owns the layout**: true only when the script
/// also drives the panes; false when it is loaded config-only (theme + settings)
/// alongside an explicit layout.
///
/// - A single directory → the GPP directory browser over it (no script).
/// - One or more plain files → the file panes are the layout, but `init.ptl` is
///   still loaded **config-only** so your color scheme / settings apply (the
///   `EDITOR garden --term file` shape). The script's own `layout(...)` is
///   ignored. A `.ptl` file here is just opened as text like any other file —
///   the layout script is selected with `--init`, not by positional extension.
/// - Nothing → the init script drives everything: `init_override` (`--init
///   <path>`) if given, else `~/.garden/init.ptl` ([`default_config_script`]);
///   if neither resolves, a single empty editor.
fn resolve_layout(
    positionals: &[String],
    init_override: Option<&Path>,
) -> (Option<ScriptHost>, LayoutNode, bool) {
    let empty = LayoutNode::Editor {
        file: None,
        line_numbers: false,
        wrap: true,
    };

    // A single directory argument opens the GPP directory browser (like vim's
    // netrw): `garden src/` shows a navigable listing of that directory.
    if let [dir] = positionals {
        if Path::new(dir).is_dir() {
            let node = LayoutNode::Process {
                command: process_pane::directory_browser_bin(),
                args: vec![dir.clone()],
            };
            return (None, node, false);
        }
    }

    if positionals.is_empty() {
        // No file given — fall back to the init script, which owns the layout.
        // An explicit `--init` path that fails to load is fatal; a missing
        // *default* config is not (we just open an empty editor).
        let init = match init_override {
            Some(path) => path.to_path_buf(),
            None => match default_config_script() {
                Some(path) => path,
                None => return (None, empty, false),
            },
        };
        return match load_script(&init) {
            Ok(host) => (Some(host), empty, true),
            Err(err) => {
                eprintln!("garden: failed to load {}: {err}", init.display());
                std::process::exit(1);
            }
        };
    }

    // One or more files: open them directly in editor panes. The files are the
    // layout, but we still load `init.ptl` config-only so its color scheme and
    // settings apply (see [`load_config_script`]). The script does not own the
    // layout, so its `layout(...)` is ignored and runtime rearrangements stay
    // in memory rather than rewriting the config.
    let mut editors: Vec<LayoutNode> = positionals
        .iter()
        .map(|f| LayoutNode::Editor {
            file: Some(f.clone()),
            line_numbers: false,
            wrap: true,
        })
        .collect();
    let layout = if editors.len() == 1 {
        editors.remove(0)
    } else {
        LayoutNode::Row {
            children: editors,
            ratios: None,
        }
    };
    (load_config_script(init_override), layout, false)
}

/// Load `init.ptl` (or the `--init` override) purely for its **config** — theme,
/// color scheme, and permanent settings — when a file argument supplies the
/// layout. Best-effort: a missing config yields `None`, and an unreadable /
/// broken one prints a warning and is skipped rather than blocking the file the
/// user asked to open (unlike the no-argument path, where the init script is the
/// whole point and a load failure is fatal).
fn load_config_script(init_override: Option<&Path>) -> Option<ScriptHost> {
    let init = match init_override {
        Some(path) => path.to_path_buf(),
        None => default_config_script()?,
    };
    match load_script(&init) {
        Ok(host) => Some(host),
        Err(err) => {
            eprintln!("garden: ignoring config {} ({err})", init.display());
            None
        }
    }
}

/// Resolve a `garden git <subcommand>` invocation into a startup layout — the
/// CLI counterpart of the `:Git` ex command.
///
/// The only subcommand is `log`, the history browser. Like `:Git` it is the
/// `git-log` panel-mode GPP client (`gpp-apps/git-viewers`): a
/// [`LayoutNode::Process`] over the current directory that pushes the history
/// drawer and answers its `query(...)` requests by shelling `git`, mirroring how
/// `garden pr` opens `pr-browser`. More git views (status, blame, …) will hang
/// off this dispatch later.
fn resolve_git_subcommand(args: &[String]) -> LayoutNode {
    match args.first().map(String::as_str) {
        Some("log") => {
            let dir = std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string());
            LayoutNode::Process {
                command: process_pane::git_log_bin(),
                args: vec![dir],
            }
        }
        Some(other) => {
            eprintln!("garden: unknown git subcommand '{other}'");
            eprintln!("Supported subcommands: log");
            std::process::exit(2);
        }
        None => {
            eprintln!("garden: 'git' needs a subcommand");
            eprintln!("Supported subcommands: log");
            std::process::exit(2);
        }
    }
}

/// Resolve `garden diff [base]` / `garden diff --stat [base]` into a startup
/// layout — the CLI counterpart of the `:Diff` ex command.
///
/// Both forms open the one **`garden-diff`** client (`gpp-apps/garden-diff`): a
/// [`LayoutNode::Process`] over the current directory diffing `base` (a ref;
/// default upstream/`main`) against the working tree, with both the unified stream
/// and the after side editable and `^S` writing the files back. `--stat` only
/// chooses which of the client's views it opens in (the per-file summary
/// diagram); the unified / split / stat pills switch freely from there.
fn resolve_diff_subcommand(args: &[String]) -> LayoutNode {
    let stat = args.iter().any(|a| a == "--stat");
    let dir = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let mut client_args = vec![dir];
    if let Some(rev) = args.iter().find(|a| !a.starts_with('-') && !a.is_empty()) {
        client_args.push(rev.clone());
    }
    if stat {
        client_args.push("--stat".to_string());
    }
    LayoutNode::Process {
        command: process_pane::garden_diff_bin(),
        args: client_args,
    }
}

/// Builtin GPP client apps launchable directly as `garden <app> [args…]` — the
/// ergonomic equivalent of `--subprocess <app> [args…]`, no flag required.
/// `git`/`diff`/`pr` keep their own friendlier resolvers (they synthesize the
/// repo dir / PR number); these are the raw app-name subcommands for every
/// builtin GPP client, so `garden sqlite-browser …`, `garden directory-browser …`,
/// etc. all work. Only the clients a *user* would open belong here: out-of-tree
/// clients and in-repo test fixtures are launched with `--subprocess <cmd>`.
const BUILTIN_GPP_APPS: &[&str] = &[
    "directory-browser",
    "sqlite-browser",
    "git-log",
    "garden-diff",
];

/// Launch a builtin GPP client `app` (resolved beside `garden`) as the whole
/// layout, passing `args` through verbatim — the exact node that
/// `--subprocess app args…` builds.
fn gpp_app_layout(app: &str, args: &[String]) -> LayoutNode {
    LayoutNode::Process {
        command: process_pane::resolve_client_bin(app),
        args: args.to_vec(),
    }
}

/// Resolve `garden pr [number]` / `garden pr --local [base]` into a startup
/// layout — the CLI counterpart of the `:PR` ex command. Both forms open the same
/// **`garden-diff`** client (`gpp-apps/garden-diff`) over the current directory,
/// as an editable before/after review with `^S` write-back:
///
/// - Default: PR mode (`--pr [number]`) — `gh` resolves the PR (an absent number
///   means the current branch's), the diff runs against its merge base, and the
///   PR description, conversation, and inline review comments come along. When
///   no PR can be resolved (none for this branch, no `gh`, not authenticated)
///   the client degrades to the `--local` review of the pending working-tree
///   changes, with a banner saying so — no PR, no error, still a useful diff.
/// - `--local` (alias `--diff`): a purely local review of `git diff <base>` — no
///   `gh`, no network. An explicit base ref may follow the flag (`garden pr
///   --local origin/main`); an empty base resolves the branch's upstream /
///   `main` / `master`.
fn resolve_pr_subcommand(args: &[String]) -> LayoutNode {
    let local = args.iter().any(|a| a == "--local");
    // The non-flag arg: the base ref (`--local`) or the PR number (PR mode).
    let positional = args.iter().find(|a| !a.starts_with("--"));
    let dir = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let mut client_args = vec![dir];
    if !local {
        client_args.push("--pr".to_string());
    }
    if let Some(arg) = positional.filter(|a| !a.is_empty()) {
        client_args.push(arg.clone());
    }
    LayoutNode::Process {
        command: process_pane::garden_diff_bin(),
        args: client_args,
    }
}

/// Resolve a `garden petal-ide [file.ptl]` invocation into a startup layout —
/// the **Petal IDE**: an editor pane and a live rendered-canvas pane side by
/// side over the *same* Petal script, so editing the source on the left updates
/// the canvas on the right as you type (via [`App::sync_editor_panels`], no save
/// needed). The pane pairing is by path, so both leaves name one absolute path:
///
/// - With a `file` argument, that file (absolutized against the cwd). A file
///   that does not exist yet is seeded with a starter sketch so the canvas shows
///   something immediately — like opening a new buffer, but non-empty.
/// - With no argument, a persistent scratch file at
///   `~/.garden/petal-ide/scratch.ptl`, seeded on first use.
///
/// The script does not own the layout (there is no `layout(...)` call to
/// rewrite) — the two panes are the layout, exactly like a file argument.
fn resolve_petal_ide_subcommand(args: &[String]) -> LayoutNode {
    let path = petal_ide_path(args.first().map(String::as_str));

    // Seed a starter sketch the first time this file is opened, so the canvas is
    // never blank on launch. Never overwrites an existing file.
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&path, PETAL_IDE_STARTER) {
            eprintln!("garden: could not seed {} ({err})", path.display());
        }
    }

    let file = path.to_string_lossy().into_owned();
    LayoutNode::Row {
        children: vec![
            LayoutNode::Editor {
                file: Some(file.clone()),
                line_numbers: true,
                wrap: true,
            },
            LayoutNode::Panel {
                script: file,
                screens: Vec::new(),
            },
        ],
        ratios: Some(vec![0.5, 0.5]),
    }
}

/// The absolute Petal-IDE script path for an optional `file` argument: the file
/// absolutized against the cwd, or the persistent `~/.garden/petal-ide/scratch.ptl`
/// scratch when none is given (falling back to a cwd-relative scratch if `$HOME`
/// is unset). Absolute so the editor and panel leaves — matched by resolved path —
/// always agree, and the panel finds the file regardless of the launch cwd.
fn petal_ide_path(file: Option<&str>) -> PathBuf {
    match file {
        Some(arg) => {
            let p = PathBuf::from(arg);
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p)
            }
        }
        None => match config_dir() {
            Some(dir) => dir.join("petal-ide").join("scratch.ptl"),
            None => PathBuf::from("petal-ide-scratch.ptl"),
        },
    }
}

/// The absolute path the Petal-IDE **IR-inspector drawer** is seeded to:
/// `~/.garden/petal-ide/ir_view.ptl`. Shared by all IDE windows; a panel pane on
/// this path is recognized as the IR inspector and given a data provider (see
/// [`petal_ide`]). Falls back to a cwd-relative name if `$HOME` is unset.
fn petal_ide_ir_view_path() -> PathBuf {
    match config_dir() {
        Some(dir) => dir.join("petal-ide").join("ir_view.ptl"),
        None => PathBuf::from("petal-ide-ir_view.ptl"),
    }
}

/// Seed the bundled IR-inspector drawer to [`petal_ide_ir_view_path`] on first
/// IDE launch, so opening the IR panel is just `panel(ir_view.ptl)`. Never
/// overwrites an existing copy (the user may have tweaked it).
fn seed_petal_ide_ir_view() {
    let path = petal_ide_ir_view_path();
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(err) = std::fs::write(&path, petal_ide::IR_VIEW_SCRIPT) {
        eprintln!("garden: could not seed {} ({err})", path.display());
    }
}

/// The starter panel sketch written into a fresh Petal-IDE file. A small, safe
/// animated scene using only documented panel natives, so a brand-new canvas is
/// alive on launch and every line is an obvious thing to tweak.
const PETAL_IDE_STARTER: &str = r#"// ── Petal IDE ──────────────────────────────────────────────────────────────
// Edit this file on the left; the canvas on the right updates live as you type
// (no save needed — though Cmd+S still writes to disk). `state` variables
// survive each live reload, so animation keeps its place while you edit.
//
// Draw:   clear / draw_rect / draw_rect_outline / draw_line / draw_circle /
//         fill_triangle / fill_poly / draw_text
// Timing: dt() / frame_count() / screen_width() / screen_height()
// Try changing a number below and watch the canvas react.

state t = 0.0
t = t + dt()

let w = screen_width()
let h = screen_height()
let cx = int(float(w) * 0.5)
let cy = int(float(h) * 0.5)

clear(14, 16, 24)
draw_rect_outline(0, 0, w, h, 40, 48, 66)

// A ring of pulsing dots orbiting the center.
let n = 12
for i in range(0, n) do
  let a = t + float(i) * (6.2832 / float(n))
  let rad = float(h) * 0.28
  let x = cx + int(cos(a) * rad)
  let y = cy + int(sin(a) * rad)
  let pulse = (sin(t * 2.0 + float(i)) + 1.0) * 0.5
  draw_circle(x, y, 4 + int(pulse * 6.0), 90, int(150.0 + pulse * 100.0), 230)
end

// A heartbeat disc in the middle.
let beat = (sin(t * 3.0) + 1.0) * 0.5
draw_circle(cx, cy, 14 + int(beat * 8.0), 240, 130, 90)

draw_text("Petal IDE — live canvas", 12, 12, 14, 210, 220, 240)
draw_text("frame " ++ str(frame_count()), 12, 32, 14, 120, 132, 156)
draw_text("edit me and watch the canvas update", 12, h - 24, 14, 120, 132, 156)
"#;

/// Locate the layout script to use when Garden is launched with no file
/// argument and no `--init` override.
///
/// This is the user's personal `~/.garden/init.ptl` (vim-style). A project-local
/// `./init.ptl` is *not* picked up automatically — point `--init` at it to use
/// one. Returns `None` when the file does not exist. Runtime layout changes do
/// not feed back in here — they persist to a per-window overlay (see
/// [`load_script`]), keeping the launch config stable.
fn default_config_script() -> Option<PathBuf> {
    config_dir()
        .map(|dir| dir.join("init.ptl"))
        .filter(|p| p.exists())
}

/// Load a layout script. The per-window transient overlay is pointed at the
/// state directory later, in [`run`] (see [`open_window_state`]), so the same
/// window id backs both layout persistence and the event log.
fn load_script(path: &Path) -> Result<ScriptHost, String> {
    ScriptHost::load(path)
}

/// Allocate a fresh window id from the state database, returning that window's
/// layout-overlay path (`~/.garden/state/window-<id>/window.ptl`) and the event
/// log that keeps the database connection open for the session. Returns
/// `(None, None)` (after a warning) when `$HOME` is unset or the state DB cannot
/// be opened, so a state-layer hiccup never blocks launching the editor — it
/// only disables per-window layout persistence and event logging.
fn open_window_state() -> (Option<PathBuf>, Option<event_log::EventLog>) {
    let Some(state_dir) = config_dir().map(|d| d.join("state")) else {
        return (None, None);
    };
    let result = state::State::open(&state_dir).and_then(|state| {
        let id = state.new_window_id()?;
        let overlay = state.window_overlay_path(id);
        Ok((overlay, state.into_event_log(id)))
    });
    match result {
        Ok((overlay, log)) => (Some(overlay), Some(log)),
        Err(err) => {
            eprintln!("garden: window state unavailable ({err}); layout changes and event logging won't persist to ~/.garden");
            (None, None)
        }
    }
}

/// Allocate a fresh window id and wire it to `script` (both [`run`] and
/// [`new_window_config`] share this): the id's layout overlay becomes the
/// script's transient path, and the id's event log is returned for the
/// window's `App`. Best-effort like [`open_window_state`].
fn attach_window_state(script: Option<&mut ScriptHost>) -> Option<event_log::EventLog> {
    let (overlay, event_log) = open_window_state();
    if let (Some(script), Some(overlay)) = (script, overlay) {
        script.set_transient_path(overlay);
    }
    event_log
}

/// Garden's per-user config directory, `~/.garden`. `None` if `$HOME` is unset.
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".garden"))
}

fn print_usage() {
    eprintln!("Usage: garden [options] [file or directory]");
    eprintln!("       garden open <file or directory>   Same, but never parsed as a subcommand");
    eprintln!("       garden git log           Open the git history browser (like `:Git`)");
    eprintln!("       garden diff [--stat] [base]  Editable before/after review of the diff vs <base> (like `:Diff`); --stat opens the summary view");
    eprintln!("       garden pr [number]       Review a GitHub pull request: merge-base diff, description + inline comments (needs `gh`)");
    eprintln!("       garden pr --local [base]  Review the LOCAL diff vs <base> (default: upstream/main); no GitHub (like `:Review`)");
    eprintln!("       garden petal-ide [file.ptl]  Live Petal editor: source on the left, rendered canvas on the right");
    eprintln!(
        "       garden <gpp-app> [args…] Launch a builtin GPP app directly (no --subprocess):"
    );
    eprintln!(
        "                   directory-browser, sqlite-browser, git-log, garden-diff"
    );
    eprintln!("       garden setup <command>   Seed or reset ~/.garden (see `garden setup`)");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --init <path>    Use <path> as the layout script (default ~/.garden/init.ptl)");
    eprintln!("  --term           Run in the terminal (TUI); usable as $EDITOR");
    eprintln!(
        "  --headless       Run without any UI; drive via the debug server (needs --debug-port)"
    );
    eprintln!("  --debug-port <n> Start the debug server on port n (0 picks a free port)");
    eprintln!("  --subprocess <cmd> [args…]  Launch an arbitrary GPP client as the layout.");
    eprintln!("                   A bare <cmd> resolves beside `garden` (e.g. sqlite-browser),");
    eprintln!("                   else on $PATH. Must be last — put garden's flags before it.");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `petal-ide` invocation over an explicit file produces the editor|canvas
    /// split (both leaves on the same absolute path) and seeds a fresh file.
    #[test]
    fn petal_ide_builds_editor_and_panel_over_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("canvas.ptl");
        let node = resolve_petal_ide_subcommand(&[file.to_string_lossy().into_owned()]);

        let abs = file.to_string_lossy().into_owned();
        match node {
            LayoutNode::Row { children, .. } => {
                assert!(matches!(
                    &children[0],
                    LayoutNode::Editor { file: Some(f), line_numbers: true, .. } if *f == abs
                ));
                assert!(matches!(
                    &children[1],
                    LayoutNode::Panel { script, .. } if *script == abs
                ));
            }
            other => panic!("expected a Row, got {other:?}"),
        }
        // The new file was seeded with a runnable starter sketch.
        let seeded = std::fs::read_to_string(&file).unwrap();
        assert!(seeded.contains("Petal IDE"));
        assert!(seeded.contains("screen_width()"));
    }

    /// An existing file is opened as-is — never overwritten by the starter.
    #[test]
    fn petal_ide_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mine.ptl");
        std::fs::write(&file, "draw_rect(0,0,1,1,0,0,0)\n").unwrap();
        resolve_petal_ide_subcommand(&[file.to_string_lossy().into_owned()]);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "draw_rect(0,0,1,1,0,0,0)\n"
        );
    }

    /// A builtin GPP app name launches directly as a Process layout (the
    /// `--subprocess`-free form), passing its args through verbatim.
    #[test]
    fn builtin_gpp_app_subcommand_launches_as_a_process() {
        let node = gpp_app_layout("sqlite-browser", &["db.sqlite".to_string()]);
        match node {
            LayoutNode::Process { command, args } => {
                // resolve_client_bin returns the bare name when no sibling
                // binary exists (as in the test environment).
                assert!(command.ends_with("sqlite-browser"), "got {command}");
                assert_eq!(args, vec!["db.sqlite".to_string()]);
            }
            other => panic!("expected a Process node, got {other:?}"),
        }
        // Every builtin GPP client is reachable by its raw name.
        for app in [
            "directory-browser",
            "sqlite-browser",
            "git-log",
            "garden-diff",
        ] {
            assert!(
                BUILTIN_GPP_APPS.contains(&app),
                "{app} should be a builtin GPP subcommand"
            );
        }
        // `gpp-test-app` is a test fixture, not a tool: no bare subcommand, so it
        // is reachable only through `--subprocess`. The retired viewers are gone
        // too — `garden-diff` covers both.
        for app in ["gpp-test-app", "pr-browser", "git-diff"] {
            assert!(
                !BUILTIN_GPP_APPS.contains(&app),
                "{app} was replaced by garden-diff"
            );
        }
    }

    /// The client args every diff/PR entry point builds: `[dir, …extra]`.
    fn client_args(node: LayoutNode) -> Vec<String> {
        match node {
            LayoutNode::Process { command, args } => {
                assert!(command.ends_with("garden-diff"), "got {command}");
                assert!(!args.is_empty(), "the repo dir is always arg 0");
                args[1..].to_vec()
            }
            other => panic!("expected a garden-diff Process node, got {other:?}"),
        }
    }

    /// Plain `garden pr [number]` opens `garden-diff` in PR mode — the GitHub
    /// reviewer, now the same client as `:Diff`/`:Review`.
    #[test]
    fn pr_subcommand_opens_garden_diff_in_pr_mode() {
        assert_eq!(client_args(resolve_pr_subcommand(&[])), vec!["--pr"]);
        // An explicit PR number rides along after the flag.
        assert_eq!(
            client_args(resolve_pr_subcommand(&["42".to_string()])),
            vec!["--pr", "42"]
        );
    }

    /// `garden pr --local [base]` opens the same client on a purely local diff —
    /// no `--pr`, so no `gh` / network involvement.
    #[test]
    fn pr_subcommand_local_flag_reviews_the_local_diff() {
        // Bare `--local`: default base (resolved by the client).
        assert!(client_args(resolve_pr_subcommand(&["--local".to_string()])).is_empty());

        // `--local <base>` in either order carries the explicit base ref.
        for args in [
            vec!["--local".to_string(), "origin/main".to_string()],
            vec!["origin/main".to_string(), "--local".to_string()],
        ] {
            assert_eq!(
                client_args(resolve_pr_subcommand(&args)),
                vec!["origin/main"],
                "args {args:?}"
            );
        }
    }

    /// `garden diff [--stat] [base]` opens `garden-diff` too: the base ref rides
    /// along, and `--stat` only picks the view it opens in.
    #[test]
    fn diff_subcommand_opens_garden_diff_with_an_optional_stat_view() {
        assert!(client_args(resolve_diff_subcommand(&[])).is_empty());
        assert_eq!(
            client_args(resolve_diff_subcommand(&["origin/main".to_string()])),
            vec!["origin/main"]
        );
        assert_eq!(
            client_args(resolve_diff_subcommand(&["--stat".to_string()])),
            vec!["--stat"]
        );
        assert_eq!(
            client_args(resolve_diff_subcommand(&[
                "--stat".to_string(),
                "HEAD~2".to_string()
            ])),
            vec!["HEAD~2", "--stat"]
        );
    }

    /// A relative file argument is absolutized so the two panes agree on identity.
    #[test]
    fn petal_ide_path_absolutizes_a_relative_argument() {
        let p = petal_ide_path(Some("rel/canvas.ptl"));
        assert!(p.is_absolute(), "{} should be absolute", p.display());
        assert!(p.ends_with("rel/canvas.ptl"));
    }
}
