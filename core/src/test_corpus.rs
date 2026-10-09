//! Shared test-only helpers: the repo-wide `.ptl` corpus that the property
//! tests (lint, trivia, CST, projection) sweep.

use std::path::{Path, PathBuf};

/// Every `.ptl` file in the repository (skipping `node_modules` and `target`
/// directories), sorted for deterministic iteration order.
pub fn repo_ptl_files() -> Vec<PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");
    let mut files = Vec::new();
    collect_ptl(repo_root, &mut files);
    files.sort();
    files
}

fn collect_ptl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Dot-directories are scratch (`.temp/` holds whole copies of the
            // tree from verify runs), not the corpus.
            if path.file_name().is_some_and(|n| {
                n == "node_modules" || n == "target" || n.to_string_lossy().starts_with('.')
            }) {
                continue;
            }
            collect_ptl(&path, out);
        } else if path.extension().is_some_and(|e| e == "ptl") {
            out.push(path);
        }
    }
}

/// Run `check` over every corpus file on all cores and return the results in
/// corpus order. The corpus sweeps that compile each file are minutes of
/// single-threaded work in a debug build and independent per file, so they
/// fan out here rather than sample the corpus. A panic in `check` is
/// re-raised on the calling thread with its original message.
pub fn par_map<T: Send>(files: &[PathBuf], check: impl Fn(&Path) -> T + Sync) -> Vec<T> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(files.len().max(1));
    let mut indexed: Vec<(usize, T)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = files.get(i) else { break };
                        done.push((i, check(path)));
                    }
                    done
                })
            })
            .collect();
        let mut all = Vec::with_capacity(files.len());
        for handle in handles {
            match handle.join() {
                Ok(done) => all.extend(done),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        all
    });
    indexed.sort_by_key(|(i, _)| *i);
    indexed.into_iter().map(|(_, value)| value).collect()
}
