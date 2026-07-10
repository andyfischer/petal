//! Hot reload: watch a program's source files and swap in a recompiled program
//! while preserving live state. Shared by every SDL host.

use std::path::Path;
use std::sync::mpsc;

use notify::{RecursiveMode, Watcher};

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

/// If a watched file changed, recompile the program and transfer live state
/// into it. Compile errors are logged and the old program keeps running.
pub fn check_hot_reload(
    reload_rx: &mpsc::Receiver<()>,
    source_path: &str,
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
) {
    if let Ok(()) = reload_rx.try_recv() {
        if let Ok(new_source) = std::fs::read_to_string(source_path) {
            let new_program =
                match env.compile_program_at(program_id, &new_source, Path::new(source_path)) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[hot-reload] compile error: {}", e);
                        return;
                    }
                };
            match env.transfer_state(stack_id, new_program) {
                Ok(result) => eprintln!(
                    "[hot-reload] preserved: {}, dropped: {}",
                    result.state_preserved, result.state_dropped
                ),
                Err(e) => eprintln!("[hot-reload] error: {}", e),
            }
        }
    }
}

/// Watch every directory the program's source files live in — the entry
/// script's directory plus the directory of each imported module in the
/// program's module manifest. Editing an imported `palette.ptl` hot-reloads the
/// scripts that import it, not just edits to the entry file. Directories are
/// watched (non-recursively); any modify event triggers a reload check.
pub fn setup_watcher(
    env: &Env,
    program_id: ProgramId,
    source_path: &str,
    tx: mpsc::Sender<()>,
) -> Result<Option<notify::RecommendedWatcher>, String> {
    let path = Path::new(source_path)
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

    let mut dirs: Vec<std::path::PathBuf> =
        vec![path.parent().unwrap_or(Path::new(".")).to_path_buf()];
    for entry in env.module_manifest(program_id) {
        if let Some(origin) = entry.origin
            && let Ok(canonical) = origin.canonicalize()
            && let Some(parent) = canonical.parent()
        {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.sort();
    dirs.dedup();
    for dir in &dirs {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| format!("Failed to watch {}: {}", dir.display(), e))?;
    }

    Ok(Some(watcher))
}
