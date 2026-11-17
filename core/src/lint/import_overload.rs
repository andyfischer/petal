//! **`import-overload-merge`** — a top-level `fn` that shares its name with a
//! function a plainly imported module exports, where the two were joined into
//! one overload set by argument count instead of the file's declaration
//! replacing the import:
//!
//! ```text
//! import ui
//! fn toggle(label, x, y) … end   // ui's toggle(r, value) and
//!                                // toggle(r, value, style) are kept too:
//! toggle(r, on)                  // this call runs ui.toggle
//! ```
//!
//! The merge is how a library adds an arity to a name another module owns
//! (`Compiler::merge_overload_binding`), but in an application it is usually
//! an accident, and a fragile one: a call's target depends on how many
//! arguments it passes, so an arity the module adds later silently takes over
//! calls that used to reach the file's function, or the other way round.
//!
//! The rule compiles the file and reads which declarations merged
//! ([`crate::program::Program::import_overload_merges`]), so it reports
//! exactly what the compiler built. Only plain imports (`import m`,
//! `import m as q`) count: a selective import of a name the file declares is
//! already an error, a host prelude joining arities is by design, and a
//! file's own several arities of one name are an ordinary overload. A
//! declaration taking every arity the module's function took replaces it
//! outright and is not reported here (`shadows-builtin` covers that).
//!
//! Report only: the fix is a rename, and the callers may be in other files.
//! **Opt-in** (`--rules-include import-overload-merge`): it compiles the
//! file, and in practice most merges are deliberate (a game's modules each
//! offering `draw` and `update`, an app's kit wrapping `bloom`'s widgets) —
//! about 40 across `examples/` and a hundred in the downstream games.

use super::{Fix, LintOptions};

pub(super) fn plan_import_overload_reports(source: &str, opts: &LintOptions) -> Vec<Fix> {
    let Ok((env, pid)) =
        crate::ir_equiv::compile_for_compare(source, &opts.include_dirs, opts.origin.as_deref())
    else {
        return Vec::new();
    };
    let Some(program) = env.get_program(pid) else {
        return Vec::new();
    };
    let mut out: Vec<Fix> = program
        .import_overload_merges
        .iter()
        .map(|m| Fix {
            anchor: m.span.start.offset as usize,
            message: format!(
                "`fn {name}` is merged by argument count with `{module}`'s `{name}`: a call \
                 with {counts} argument{s} runs `{qualifier}.{name}`, and an arity `{module}` \
                 adds later can silently take over calls to this one; rename this function, \
                 or write `{qualifier}.{name}(…)` where the module's is meant",
                name = m.name,
                module = m.module,
                qualifier = m.qualifier,
                counts = counts(&m.imported_arities),
                s = if m.imported_arities == [1] { "" } else { "s" },
            ),
            splices: Vec::new(),
        })
        .collect();
    out.sort_by_key(|f| f.anchor);
    out
}

/// `2`, `2 or 3`, `1, 2 or 3`.
fn counts(arities: &[usize]) -> String {
    let words: Vec<String> = arities.iter().map(usize::to_string).collect();
    match words.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{IMPORT_OVERLOAD_MERGE, LintOptions, RuleFilter, lint_source};
    use std::path::{Path, PathBuf};

    fn opts(origin: Option<&Path>) -> LintOptions {
        LintOptions {
            include_dirs: Vec::new(),
            origin: origin.map(Path::to_path_buf),
            rules: RuleFilter {
                include: Some(vec![IMPORT_OVERLOAD_MERGE.to_string()]),
                exclude: Vec::new(),
            },
        }
    }

    /// `(line, message)` per finding.
    fn merges_at(src: &str, origin: Option<&Path>) -> Vec<(usize, String)> {
        lint_source(src, &opts(origin))
            .expect("lint")
            .findings
            .into_iter()
            .map(|f| (f.line, f.message))
            .collect()
    }

    fn merges(src: &str) -> Vec<(usize, String)> {
        merges_at(src, None)
    }

    #[test]
    fn a_fn_merged_with_an_imported_function_is_reported() {
        // ui's `toggle(r, value)` and `toggle(r, value, style)`: the file's
        // three-parameter one replaces the second and keeps the first.
        let got = merges("import ui\n\nfn toggle(label, x, y)\n  label\nend\n");
        assert_eq!(got.len(), 1, "{got:?}");
        let (line, message) = &got[0];
        assert_eq!(*line, 3);
        assert!(message.contains("`fn toggle`"), "{message}");
        assert!(message.contains("`ui`'s `toggle`"), "{message}");
        assert!(message.contains("a call with 2 arguments runs `ui.toggle`"), "{message}");
        assert!(message.contains("rename"), "{message}");
    }

    #[test]
    fn the_suggestion_uses_the_import_alias() {
        let got = merges("import ui as u\nfn toggle(label, x, y) label end\n");
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].1.contains("`u.toggle(…)`"), "{}", got[0].1);
        assert!(got[0].1.contains("`ui`'s `toggle`"), "{}", got[0].1);
    }

    #[test]
    fn a_declaration_taking_every_imported_arity_replaces_it_and_is_not_reported() {
        // Two to three arguments: everything ui's `toggle` took.
        assert_eq!(merges("import ui\nfn toggle(a, b, c = nil) a end\n"), vec![]);
    }

    #[test]
    fn the_files_own_overloads_and_unimported_names_are_not_reported() {
        assert_eq!(merges("fn f(a) a end\nfn f(a, b) b end\nprint(f(1))\n"), vec![]);
        // The name is ui's, but ui is not imported.
        assert_eq!(merges("fn toggle(label, x, y) label end\n"), vec![]);
        // A nested declaration shadows; it never merges.
        assert_eq!(
            merges("import ui\nfn outer()\n  fn toggle(a, b, c) a end\n  toggle(1, 2, 3)\nend\n"),
            vec![]
        );
    }

    #[test]
    fn it_is_opt_in() {
        let src = "import ui\nfn toggle(label, x, y) label end\n";
        let default = LintOptions::default();
        let got = lint_source(src, &default).expect("lint");
        assert_eq!(got.count(IMPORT_OVERLOAD_MERGE), 0);
        assert_eq!(merges(src).len(), 1);
    }

    #[test]
    fn an_ignore_comment_silences_it() {
        let src = "import ui\n# petal-lint-ignore import-overload-merge -- adds an arity on purpose\n\
                   fn toggle(label, x, y) label end\n";
        assert_eq!(merges(src), vec![]);
    }

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_local_modules_function_counts_and_an_overloaded_declaration_is_reported_once() {
        let root = std::env::temp_dir()
            .join(format!("petal-lint-import-overload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let dir = Dir(crate::module::canonical_path(&root));
        std::fs::write(dir.0.join("shapes.ptl"), "pub fn area(r) r * r end\n").unwrap();
        let main = dir.0.join("main.ptl");
        let src = "import ./shapes\n\nfn area(w, h) w * h end\nfn area(w, h, d) w * h * d end\n\
                   print(area(2, 3))\n";
        std::fs::write(&main, src).unwrap();
        let got = merges_at(src, Some(&main));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, 3, "reported at the first variant: {got:?}");
        assert!(got[0].1.contains("a call with 1 argument runs `shapes.area`"), "{}", got[0].1);
    }
}
