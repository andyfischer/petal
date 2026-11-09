//! Build script: embed the bundled packages (`petal::bundled`) in the binary,
//! and record what the binary was built from and when (`petal::version`).
//!
//! A bundled package is Petal source and `native fn` declarations only. They
//! are listed in `Cargo.toml` under [package.metadata.petal.bundled]; a
//! `library` entry is embedded only when its cargo feature is on. Each one is
//! read from its home in the repository; a missing directory fails the build
//! rather than producing a binary that silently lacks a package.

use std::path::Path;

#[path = "build_support/strip_comments.rs"]
mod strip_comments;
#[path = "build_support/toml_lite.rs"]
mod toml_lite;

/// Loose `.ptl` files the crate embeds outside the bundled packages, relative
/// to this crate: each goes through [`embed_source`] to
/// `$OUT_DIR/embedded/<same path>`, where the crate's `include_str!` reads it.
const EMBEDDED_FILES: &[&str] = &["prelude/std.ptl"];

/// Whether embedded Petal sources lose their comments. Default: on for wasm32
/// (every byte is downloaded), off elsewhere (native tools — the LSP, `petal
/// lint` — show comments of bundled code). `PETAL_EMBED_COMMENTS=keep|strip`
/// overrides either way.
fn strip_embedded_comments() -> bool {
    println!("cargo::rerun-if-env-changed=PETAL_EMBED_COMMENTS");
    match std::env::var("PETAL_EMBED_COMMENTS").as_deref() {
        Ok("keep") => false,
        Ok("strip") => true,
        Ok(other) => panic!("PETAL_EMBED_COMMENTS must be `keep` or `strip`, not `{other}`"),
        Err(_) => std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32"),
    }
}

/// Copy the embedded source `abs` to `$OUT_DIR/embedded/<rel>` — with its
/// comments stripped when `strip` and it is a `.ptl` — and return the path of
/// the copy, for an `include_str!`. Every embedded file goes through here, so
/// the crate reads one location whichever way the build goes.
fn embed_source(abs: &Path, rel: &str, strip: bool) -> String {
    println!("cargo::rerun-if-changed={}", abs.display());
    let text = std::fs::read_to_string(abs)
        .unwrap_or_else(|e| panic!("reading embedded source {}: {e}", abs.display()));
    let text = if strip && rel.ends_with(".ptl") {
        strip_comments::strip_comments(&text)
    } else {
        text
    };
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("embedded")
        .join(rel);
    std::fs::create_dir_all(out.parent().unwrap()).expect("creating $OUT_DIR/embedded");
    // Rewrite only on change, so an unchanged file keeps its mtime and does not
    // make cargo rebuild the crate.
    if std::fs::read_to_string(&out).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&out, &text).unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
    }
    out.display().to_string()
}

/// The table in `Cargo.toml` that lists the bundled packages.
const BUNDLED_TABLE: &str = "package.metadata.petal.bundled";

/// One entry of [`BUNDLED_TABLE`].
struct Bundled {
    name: String,
    /// Relative to this crate.
    dir: String,
    /// `core`, `builtin`, `ui` or `library`.
    kind: String,
    /// The cargo feature that embeds a `library`.
    feature: Option<String>,
}

/// Read [`BUNDLED_TABLE`] and the [features] table from this crate's
/// `Cargo.toml`, checking each entry. Panics (failing the build) on an entry
/// that is malformed, of an unknown kind, or a library whose feature is not
/// declared.
fn bundled_manifest(manifest: &str) -> Vec<Bundled> {
    let path = Path::new(manifest).join("Cargo.toml");
    println!("cargo::rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let features: Vec<String> = toml_lite::table_entries(&text, "features")
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let entries = toml_lite::table_entries(&text, BUNDLED_TABLE);
    if entries.is_empty() {
        panic!("{}: no [{BUNDLED_TABLE}] entries; the petal crate needs at least `core`", path.display());
    }
    let mut out = Vec::new();
    for (name, value) in entries {
        let fields = toml_lite::inline_strings(&value).unwrap_or_else(|| {
            panic!(
                "[{BUNDLED_TABLE}] `{name}`: expected an inline table of strings like \
                 `{{ dir = \"...\", kind = \"library\", feature = \"...\" }}`, got `{value}`"
            )
        });
        let field = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        for (key, _) in &fields {
            if !["dir", "kind", "feature"].contains(&key.as_str()) {
                panic!("[{BUNDLED_TABLE}] `{name}`: unknown field `{key}` (dir, kind, feature)");
            }
        }
        let dir = field("dir").unwrap_or_else(|| panic!("[{BUNDLED_TABLE}] `{name}`: no `dir`"));
        let kind = field("kind").unwrap_or_else(|| panic!("[{BUNDLED_TABLE}] `{name}`: no `kind`"));
        let feature = field("feature");
        match (kind.as_str(), &feature) {
            ("library", None) => {
                panic!("[{BUNDLED_TABLE}] `{name}`: a library needs the `feature` that embeds it")
            }
            ("library", Some(f)) if !features.contains(f) => panic!(
                "[{BUNDLED_TABLE}] `{name}`: its feature `{f}` is not declared in [features] of {}. \
                 Add `{f} = []` there (and to the `libraries` umbrella).",
                path.display()
            ),
            ("library", Some(_)) => {}
            ("core" | "builtin" | "ui", None) => {}
            ("core" | "builtin" | "ui", Some(_)) => panic!(
                "[{BUNDLED_TABLE}] `{name}`: only a `library` is gated by a feature; \
                 a `{kind}` package is always embedded"
            ),
            _ => panic!(
                "[{BUNDLED_TABLE}] `{name}`: unknown kind `{kind}` (core, builtin, ui, library)"
            ),
        }
        out.push(Bundled { name, dir, kind, feature });
    }
    if !out.iter().any(|b| b.kind == "core" && b.name == "core") {
        panic!("[{BUNDLED_TABLE}]: no `core` package of kind `core`");
    }
    out
}

/// Whether cargo feature `feature` is on for this build.
fn feature_enabled(feature: &str) -> bool {
    let var = format!("CARGO_FEATURE_{}", feature.to_uppercase().replace('-', "_"));
    std::env::var_os(var).is_some()
}

/// The import names a package's `petal.toml` lists under [dependencies]
/// (either `name = {...}` lines or `[dependencies.name]` tables).
fn manifest_dependencies(petal_toml: &str) -> Vec<String> {
    let mut deps: Vec<String> = toml_lite::table_entries(petal_toml, "dependencies")
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    for header in toml_lite::headers(petal_toml) {
        if let Some(name) = header.strip_prefix("dependencies.") {
            deps.push(name.trim_matches('"').to_string());
        }
    }
    deps
}

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

/// Write `$OUT_DIR/bundled_packages.rs` (the table `petal::bundled` includes)
/// and `$OUT_DIR/bundled_meta.rs` (the package lists by kind, and the
/// libraries this build leaves out).
fn embed_bundled_packages(manifest: &str, strip: bool) {
    let all = bundled_manifest(manifest);
    let (included, omitted): (Vec<&Bundled>, Vec<&Bundled>) = all
        .iter()
        .partition(|b| b.feature.as_deref().is_none_or(feature_enabled));

    let mut code = String::from("&[\n");
    for b in &included {
        let name = &b.name;
        let dir = Path::new(manifest).join(&b.dir);
        let dir = dir.canonicalize().unwrap_or_else(|e| {
            panic!(
                "bundled package `{name}`: its source directory {} is missing ({e}). \
                 The petal binary embeds this package; restore the directory.",
                dir.display()
            )
        });
        let petal_toml = dir.join("petal.toml");
        let Ok(petal_toml_text) = std::fs::read_to_string(&petal_toml) else {
            panic!(
                "bundled package `{name}`: {} has no petal.toml",
                dir.display()
            );
        };
        for dep in manifest_dependencies(&petal_toml_text) {
            if let Some(left_out) = omitted.iter().find(|o| o.name == dep) {
                let feature = left_out.feature.as_deref().unwrap_or_default();
                let gate = b
                    .feature
                    .as_deref()
                    .map(|f| format!(" (embedded by feature `{f}`)"))
                    .unwrap_or_default();
                panic!(
                    "bundled package `{name}`{gate} depends on `{dep}` ({}), which this build \
                     leaves out. Turn on feature `{feature}` too, or make `{feature}` part of \
                     the feature that embeds `{name}` in core/Cargo.toml [features].",
                    petal_toml.display()
                );
            }
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
            let embedded = embed_source(&dir.join(&file), &format!("bundled/{name}/{file}"), strip);
            code.push_str(&format!("        ({file:?}, include_str!({embedded:?})),\n"));
        }
        code.push_str("    ] },\n");
    }
    code.push_str("]\n");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    write_if_changed(&Path::new(&out).join("bundled_packages.rs"), &code);

    let names = |kind: &str, from: &[&Bundled]| {
        from.iter()
            .filter(|b| b.kind == kind)
            .map(|b| format!("{:?}, ", b.name))
            .collect::<String>()
    };
    let omitted_libs: String = omitted
        .iter()
        .map(|b| format!("({:?}, {:?}), ", b.name, b.feature.as_deref().unwrap_or_default()))
        .collect();
    let meta = format!(
        "/// The bundled packages that declare the domain builtins (kind `builtin`).\n\
         pub const BUILTIN_PACKAGES: &[&str] = &[{}];\n\
         /// The bundled libraries (kind `library`) embedded in this build.\n\
         pub const LIBRARIES: &[&str] = &[{}];\n\
         /// The bundled libraries this build leaves out: `(package, the petal\n\
         /// crate feature that embeds it)`.\n\
         pub const OMITTED_LIBRARIES: &[(&str, &str)] = &[{omitted_libs}];\n",
        names("builtin", &included),
        names("library", &included),
    );
    write_if_changed(&Path::new(&out).join("bundled_meta.rs"), &meta);
}

/// Write `text` to `path` unless it already holds exactly that.
fn write_if_changed(path: &Path, text: &str) {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
    }
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
            #[allow(clippy::disallowed_methods, reason = "build scripts run on the host, never on wasm32")]
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
    let strip = strip_embedded_comments();
    println!(
        "cargo::rustc-env=PETAL_EMBED_COMMENTS_STRIPPED={}",
        if strip { "1" } else { "0" }
    );
    embed_bundled_packages(&manifest, strip);
    for rel in EMBEDDED_FILES {
        embed_source(&Path::new(&manifest).join(rel), rel, strip);
    }
    embed_build_info(&manifest);
}
