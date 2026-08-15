//! Stamps the `garden` binary with the build it came from, so a running (or
//! installed) binary can answer "what am I?" instead of leaving a client to
//! discover a missing feature by triggering an error. The values land as
//! `cargo:rustc-env` vars that `src/version.rs` reads with `env!`.
//!
//! Every git lookup degrades to `"unknown"` rather than failing the build —
//! a source tarball with no `.git` must still compile.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    // `garden/` is a subdirectory of the petal repo and has no `.git` of its
    // own, so ask git where the git dir actually is instead of guessing
    // `../.git` (which would silently stamp "unknown" forever).
    let git_dir = git(&manifest_dir, &["rev-parse", "--absolute-git-dir"]).map(PathBuf::from);

    let commit = git(&manifest_dir, &["rev-parse", "--short", "HEAD"]);
    let commit_date = git(&manifest_dir, &["log", "-1", "--format=%cs"]);
    let dirty = git(&manifest_dir, &["status", "--porcelain"])
        .map(|out| !out.trim().is_empty())
        .unwrap_or(false);

    println!(
        "cargo:rustc-env=GARDEN_GIT_COMMIT={}",
        commit.unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=GARDEN_GIT_DATE={}",
        commit_date.unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=GARDEN_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );
    println!("cargo:rustc-env=GARDEN_BUILD_DATE={}", build_date());

    // Re-stamp when HEAD moves. Day-granularity build dates and these narrow
    // triggers keep a plain `cargo build` from re-running this script (and so
    // relinking the crate) on every invocation.
    if let Some(dir) = git_dir {
        rerun(&dir.join("HEAD"));
        rerun(&dir.join("packed-refs"));
        if let Some(reference) = head_ref(&dir) {
            rerun(&dir.join(reference));
        }
    }
}

fn rerun(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

/// The ref path `HEAD` points at (`refs/heads/main`), or `None` when detached.
fn head_ref(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let reference = head.trim().strip_prefix("ref: ")?;
    Some(reference.to_string())
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim().to_string();
    if text.is_empty() && args[0] != "status" {
        return None;
    }
    Some(text)
}

/// Today's date, UTC, as `yyyy-mm-dd`. Computed from the epoch by hand rather
/// than pulling in a date crate for one line.
fn build_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
