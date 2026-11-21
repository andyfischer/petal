//! The files a package directory is made of: its `petal.toml` and every
//! `*.ptl` under it, outside the directories that are not part of what it
//! exports.
//!
//! Shared by `core/build.rs` (the bundled packages) and the `petal-macros`
//! crate (`embed_package!`, a host's own packages), so a package embeds the
//! same files whichever way it is compiled in. Keep this file free of
//! anything but `std` so both can include it with `#[path]`.

use std::path::Path;

/// Directories of a package that are not part of what it exports: its tests
/// and examples are loose scripts beside the modules, and `native/` is the
/// Rust crate that registers a native package's natives.
pub const SKIPPED_DIRS: &[&str] = &["tests", "examples", "docs", "target", "native"];

/// Every file under `dir` a package is made of (`petal.toml`, `*.ptl`), as
/// `/`-joined paths relative to `dir`, sorted.
pub fn package_files(dir: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    Ok(out)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("reading package directory {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let rel_path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if path.is_dir() {
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&path, &rel_path, out)?;
        } else if name == "petal.toml" || name.ends_with(".ptl") {
            out.push(rel_path);
        }
    }
    Ok(())
}
