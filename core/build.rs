//! Build script: embed the bundled packages (`petal::bundled`) in the binary,
//! and record what the binary was built from and when (`petal::version`).
//!
//! A bundled package is Petal source and `native fn` declarations only. Each
//! one is read from its home in the repository; a missing directory fails the
//! build rather than producing a binary that silently lacks a package.

use std::path::Path;

/// The packages compiled into the petal binary: `(name, directory relative to
/// this crate)`. Each is a `petal.toml` package of Petal source and
/// `native fn` declarations; `petal::bundled` serves them. A directory that
/// is missing fails the build — a binary that silently lacks `core` would
/// compile every script against an empty set of builtins.
const BUNDLED_PACKAGES: &[(&str, &str)] = &[
    ("core", "packages/core"),
    // The domain builtins: natives every Env registers, in scope only where
    // imported (`petal::bundled::BUILTIN_PACKAGES`).
    ("noise", "packages/noise"),
    ("color", "packages/color"),
    ("vec2", "packages/vec2"),
    ("autodiff", "packages/autodiff"),
    ("json", "packages/json"),
    ("io", "packages/io"),
    ("creative", "packages/creative"),
    ("ui", "../core-libs/petal-ui/prelude"),
    ("bloom", "../core-runtime/bloom"),
    ("text_layout", "../core-runtime/text-layout"),
    ("glsl", "../core-runtime/glsl"),
];

/// Directories of a package that are not part of what it exports: its tests
/// and examples are loose scripts beside the modules.
const SKIPPED_DIRS: &[&str] = &["tests", "examples", "docs", "target"];

/// Every file under `dir` a package is made of (`petal.toml`, `*.ptl`),
/// as `/`-joined paths relative to `dir`, sorted.
fn package_files(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading bundled package directory {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let rel_path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if path.is_dir() {
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }
            package_files(&path, &rel_path, out);
        } else if name == "petal.toml" || name.ends_with(".ptl") {
            out.push(rel_path);
        }
    }
}

/// Write `$OUT_DIR/bundled_packages.rs`: the table `petal::bundled` includes.
fn embed_bundled_packages(manifest: &str) {
    let mut code = String::from("&[\n");
    for (name, rel) in BUNDLED_PACKAGES {
        let dir = Path::new(manifest).join(rel);
        let dir = dir.canonicalize().unwrap_or_else(|e| {
            panic!(
                "bundled package `{name}`: its source directory {} is missing ({e}). \
                 The petal binary embeds this package; restore the directory.",
                dir.display()
            )
        });
        if !dir.join("petal.toml").is_file() {
            panic!(
                "bundled package `{name}`: {} has no petal.toml",
                dir.display()
            );
        }
        println!("cargo::rerun-if-changed={}", dir.display());
        let mut files = Vec::new();
        package_files(&dir, "", &mut files);
        if !files.iter().any(|f| f.ends_with(".ptl")) {
            panic!(
                "bundled package `{name}`: {} holds no .ptl modules",
                dir.display()
            );
        }
        code.push_str(&format!("    BundledPackage {{ name: {name:?}, files: &[\n"));
        for file in files {
            let abs = dir.join(&file);
            println!("cargo::rerun-if-changed={}", abs.display());
            code.push_str(&format!(
                "        ({file:?}, include_str!({:?})),\n",
                abs.display().to_string()
            ));
        }
        code.push_str("    ] },\n");
    }
    code.push_str("]\n");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(Path::new(&out).join("bundled_packages.rs"), code)
        .expect("writing bundled_packages.rs");
}

/// Run `git <args>` in `dir` and return its trimmed stdout, or `None` when
/// git is missing, fails, or prints nothing.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// `YYYY-MM-DD` (UTC) for a count of seconds since the Unix epoch. The civil
/// calendar by Howard Hinnant's `civil_from_days`; no date crate is worth a
/// build dependency for one line of output.
fn utc_date(epoch_secs: u64) -> String {
    let z = (epoch_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The commit a vendored copy of this tree was taken at: the `Commit: <hash>`
/// line of a `VENDORED.md` beside the crate's parent directory, which is how
/// an embedder that copies Petal into its own repository records it.
fn vendored_commit(repo_root: &Path) -> Option<String> {
    let path = repo_root.join("VENDORED.md");
    println!("cargo::rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let hash = line.trim().strip_prefix("Commit:")?.split_whitespace().next()?;
        (hash.len() >= 7 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hash.to_string())
    })
}

/// Tell the compiler what this binary was built from and when
/// (`petal::version`): `PETAL_GIT_COMMIT`, `PETAL_GIT_DATE`, `PETAL_BUILD_DATE`.
///
/// The commit is read from git only when the crate sits in its own
/// repository: a copy vendored into another project would otherwise report
/// that project's commit. Such a copy says what its `VENDORED.md` records. A
/// tree with uncommitted changes to tracked files is marked `-dirty`.
///
/// The script reruns whenever a source file changes (`src`, the bundled
/// packages) or the checkout moves (`.git/HEAD`, the branch it names), so the
/// build date is the date the code was last compiled from changed sources.
/// `SOURCE_DATE_EPOCH` pins it, for a reproducible build.
fn embed_build_info(manifest: &str) {
    let manifest = Path::new(manifest);
    let repo_root = manifest.parent().unwrap_or(manifest);
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-changed=prelude");
    println!("cargo::rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo::rerun-if-env-changed=PETAL_GIT_COMMIT");

    let own_repo = git(manifest, &["rev-parse", "--show-toplevel"]).is_some_and(|top| {
        match (Path::new(&top).canonicalize(), repo_root.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    });
    let (mut commit, mut date) = (String::from("unknown"), String::new());
    if let Ok(given) = std::env::var("PETAL_GIT_COMMIT")
        && !given.trim().is_empty()
    {
        // A packager that builds from a tarball says what it packaged.
        commit = given.trim().to_string();
    } else if own_repo {
        // The files that move when the checkout does.
        if let Some(git_dir) = git(manifest, &["rev-parse", "--absolute-git-dir"]) {
            let git_dir = Path::new(&git_dir);
            println!("cargo::rerun-if-changed={}", git_dir.join("HEAD").display());
            if let Some(head_ref) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) {
                println!("cargo::rerun-if-changed={}", git_dir.join(head_ref).display());
            }
            println!("cargo::rerun-if-changed={}", git_dir.join("packed-refs").display());
        }
        if let Some(hash) = git(manifest, &["rev-parse", "--short=10", "HEAD"]) {
            let dirty = git(manifest, &["status", "--porcelain", "--untracked-files=no"]).is_some();
            commit = if dirty { format!("{hash}-dirty") } else { hash };
            date = git(manifest, &["log", "-1", "--format=%cs"]).unwrap_or_default();
        }
    } else if let Some(hash) = vendored_commit(repo_root) {
        let short = &hash[..hash.len().min(10)];
        commit = format!("{short}-vendored");
    }

    let now = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        })
        .unwrap_or(0);
    println!("cargo::rustc-env=PETAL_GIT_COMMIT={commit}");
    println!("cargo::rustc-env=PETAL_GIT_DATE={date}");
    println!("cargo::rustc-env=PETAL_BUILD_DATE={}", utc_date(now));
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    embed_bundled_packages(&manifest);
    embed_build_info(&manifest);
}
