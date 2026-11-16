//! The core libraries' documentation (docs/proposals/doc-annotation.md,
//! migration steps 2-3): every `pub` item of `core/prelude/std.ptl` and of
//! the bundled packages under `core/packages/` has a `@doc`, every package
//! root module a `@module_doc` that agrees with its `petal.toml`
//! description, and no doc names a parameter or a `see:` target that is not
//! there. `npm run lint:docs` reports the same in CI for every library; this
//! keeps these at zero findings.

use std::path::{Path, PathBuf};

use petal::lint::{
    DOC_PARAMS, LintOptions, PACKAGE_DESCRIPTION, RuleFilter, UNDOCUMENTED_PUB, lint_source,
};

fn ptl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            ptl_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ptl") {
            out.push(path);
        }
    }
}

#[test]
fn every_export_of_the_core_libraries_is_documented() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("prelude/std.ptl")];
    ptl_files(&root.join("packages"), &mut files);
    files.sort();
    assert!(files.len() >= 19, "the core libraries went missing: {files:?}");

    let rules = [UNDOCUMENTED_PUB, DOC_PARAMS, PACKAGE_DESCRIPTION];
    let mut problems = Vec::new();
    let mut documented = 0;
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        documented += src.matches("@doc(").count();
        let opts = LintOptions {
            origin: Some(file.clone()),
            rules: RuleFilter {
                include: Some(rules.iter().map(|r| r.to_string()).collect()),
                exclude: Vec::new(),
            },
            ..LintOptions::default()
        };
        let out = lint_source(&src, &opts).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let shown = file.strip_prefix(root).unwrap_or(file).display().to_string();
        for f in out.findings {
            problems.push(format!("{shown}:{}:{}: {}: {}", f.line, f.column, f.rule, f.message));
        }
        // A package's root module carries the package's doc.
        let named_for_its_dir = file.parent().and_then(|d| d.file_name()) == file.file_stem();
        if named_for_its_dir || file.ends_with("packages/core/src/core.ptl") {
            assert!(src.contains("@module_doc("), "{shown}: a package root needs a @module_doc");
        }
    }
    assert!(
        problems.is_empty(),
        "documentation findings in the core libraries:\n{}",
        problems.join("\n")
    );
    // Sanity: the rules ran over documented sources, not empty files.
    assert!(documented > 150, "only {documented} @doc annotations");
}
