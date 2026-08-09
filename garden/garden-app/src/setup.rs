//! `garden setup …` subcommands — seed and reset the per-user config directory
//! at `~/.garden`. These mirror the steps the `install-local.sh` installer used
//! to perform inline, so the installer (and a fresh checkout) can share one
//! source of truth for what a default config looks like.

use std::fs;
use std::path::{Path, PathBuf};

/// Garden's per-user config directory, `~/.garden`. `Err` if `$HOME` is unset.
fn config_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".garden"))
        .ok_or_else(|| "$HOME is not set".to_string())
}

/// The default `init.ptl` seeded into a fresh config directory. `config_dir` is
/// the absolute `~/.garden` path so the scratch file resolves regardless of the
/// directory Garden is later launched from.
fn default_init_ptl(config_dir: &Path) -> String {
    let scratch = config_dir.join("scratch.md");
    format!(
        "// Garden layout script — your personal config (~/.garden/init.ptl).\n\
         // Edit while Garden is running and the layout hot-reloads.\n\
         //\n\
         // editor()       → an empty scratch pane\n\
         // editor(\"path\") → open a file (relative paths resolve to the launch directory)\n\
         // row([...]) / column([...], [ratios]) → split panes\n\
         \n\
         layout(\n\
         \x20   column([\n\
         \x20       editor(),\n\
         \x20       editor(\"{scratch}\"),\n\
         \x20   ], [0.7, 0.3])\n\
         )\n",
        scratch = scratch.display()
    )
}

/// Create `~/.garden` with a default `init.ptl` and `scratch.md` if they are
/// missing, leaving any existing files untouched. Idempotent — safe to run on
/// every install.
pub fn initialize_config_if_missing() -> Result<(), String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;

    let init = dir.join("init.ptl");
    if init.exists() {
        println!("init.ptl already exists — left untouched");
    } else {
        fs::write(&init, default_init_ptl(&dir))
            .map_err(|e| format!("writing {}: {e}", init.display()))?;
        println!("wrote default {}", init.display());
    }

    let scratch = dir.join("scratch.md");
    if !scratch.exists() {
        fs::write(&scratch, "# Scratch\n\n")
            .map_err(|e| format!("writing {}: {e}", scratch.display()))?;
    }
    Ok(())
}

/// Reset the config files in `~/.garden` back to defaults: remove everything
/// (config + transient overlays) *except* the `state/` directory, then re-seed.
/// Preserves the per-window layout overlays and the SQLite state DB so window
/// ids stay stable and local state survives the reset.
pub fn reset_config() -> Result<(), String> {
    let dir = config_dir()?;
    if dir.exists() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
            if entry.file_name() == "state" {
                continue; // preserve ~/.garden/state (SQLite DB + window overlays)
            }
            let path = entry.path();
            let result = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            result.map_err(|e| format!("removing {}: {e}", path.display()))?;
            println!("removed {}", path.display());
        }
    }
    initialize_config_if_missing()
}

/// Dispatch a `garden setup <subcommand>` invocation. Returns the process exit
/// code. `args` is everything after `setup`.
pub fn run(args: &[String]) -> i32 {
    let result = match args.first().map(String::as_str) {
        Some("initialize-config-if-missing") => initialize_config_if_missing(),
        Some("reset-config") => reset_config(),
        Some(other) => {
            eprintln!("garden setup: unknown subcommand {other}");
            print_usage();
            return 2;
        }
        None => {
            print_usage();
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("garden setup: {err}");
            1
        }
    }
}

fn print_usage() {
    eprintln!("Usage: garden setup <command>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  initialize-config-if-missing  Seed ~/.garden with a default init.ptl");
    eprintln!("                                and scratch.md if they don't exist");
    eprintln!("  reset-config                  Wipe ~/.garden and re-seed the defaults");
}
