//! Hot reload: watch a program's source files and swap in a recompiled program
//! while preserving live state. Shared by every SDL host.

use std::path::Path;
use std::sync::mpsc;

use notify::{RecursiveMode, Watcher};

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

/// If a watched file changed, bring the program up to date with its source
/// ([`Env::reload_program`]): a layout or comment edit only moves source
/// positions, a value edit is written into the running program, anything
/// else is recompiled with live state transferred into it. Compile errors are
/// logged and the old program keeps running.
/// Returns `true` when the program's behavior may have changed — the signal a
/// timeline uses to replay recorded history through the edited code. A
/// layout-only edit returns `false`.
pub fn check_hot_reload(
    reload_rx: &mpsc::Receiver<()>,
    source_path: &str,
    env: &mut Env,
    _program_id: ProgramId,
    stack_id: StackKey,
) -> bool {
    use petal::env::ReloadOutcome;
    // Drain every queued notification: an editor save often fires several
    // modify events, and each one should cost at most one reload.
    let mut pending = false;
    while let Ok(()) = reload_rx.try_recv() {
        pending = true;
    }
    if !pending {
        return false;
    }
    let Ok(new_source) = std::fs::read_to_string(source_path) else {
        return false;
    };
    match env.reload_program(stack_id, &new_source, Some(Path::new(source_path))) {
        Ok(report) => match report.outcome {
            ReloadOutcome::Unchanged | ReloadOutcome::Relocated => false,
            ReloadOutcome::Patched => {
                eprintln!("[hot-reload] values updated in place");
                true
            }
            ReloadOutcome::Recompiled => {
                eprintln!(
                    "[hot-reload] preserved: {}, dropped: {}",
                    report.state_preserved, report.state_dropped
                );
                true
            }
        },
        Err(e) => {
            eprintln!("[hot-reload] compile error: {}", e);
            false
        }
    }
}

/// Watch every directory the program's source files live in — the entry
/// script plus each imported module with a file behind it
/// ([`Env::program_source_paths`]). Editing an imported `palette.ptl`
/// hot-reloads the scripts that import it, not just edits to the entry file.
/// Directories are watched (non-recursively); any modify event triggers a
/// reload check.
pub fn setup_watcher(
    env: &Env,
    program_id: ProgramId,
    source_path: &str,
    tx: mpsc::Sender<()>,
) -> Result<Option<notify::RecommendedWatcher>, String> {
    // Fail early, with the entry's own error, if it cannot be resolved.
    Path::new(source_path)
        .canonicalize()
        .map_err(|e| format!("Failed to resolve path: {}", e))?;

    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
        if let Ok(event) = res {
            if event.kind.is_modify() {
                let _ = tx.send(());
            }
        }
    })
    .map_err(|e| format!("Failed to create watcher: {}", e))?;

    let mut dirs: Vec<std::path::PathBuf> = env
        .program_source_paths(program_id, Some(Path::new(source_path)))
        .into_iter()
        .filter_map(|p| Some(p.canonicalize().ok()?.parent()?.to_path_buf()))
        .collect();
    dirs.sort();
    dirs.dedup();
    for dir in &dirs {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| format!("Failed to watch {}: {}", dir.display(), e))?;
    }

    Ok(Some(watcher))
}
