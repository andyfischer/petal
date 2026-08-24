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
            if path
                .file_name()
                .is_some_and(|n| n == "node_modules" || n == "target")
            {
                continue;
            }
            collect_ptl(&path, out);
        } else if path.extension().is_some_and(|e| e == "ptl") {
            out.push(path);
        }
    }
}
