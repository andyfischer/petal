//! `petal migrate-old-code --only comment-to-doc` (docs/proposals/doc-annotation.md,
//! Migration step 1): comment blocks that document a declaration or a file
//! by convention become `@doc(…)` / `@module_doc(…)`. One fixture per rule;
//! every output parses, compiles to IR equivalent to its input's, and is a
//! fixed point. The CLI half (opt-in, the diff) is
//! cli/tests/migrate_old_code_cli.rs.

use petal::migrate::{COMMENT_TO_DOC, MigrateOptions, MigrateOutcome, migrate_source};
use petal::source_map::FileId;

#[derive(Clone, Copy, Default)]
struct Flags {
    private: bool,
    settings: bool,
}

const PUB: Flags = Flags { private: false, settings: false };
const PRIVATE: Flags = Flags { private: true, settings: false };
const SETTINGS: Flags = Flags { private: false, settings: true };

fn convert(src: &str, flags: Flags) -> MigrateOutcome {
    let opts = MigrateOptions {
        only: Some(vec![COMMENT_TO_DOC.to_string()]),
        doc_private: flags.private,
        doc_settings: flags.settings,
        ..Default::default()
    };
    migrate_source(src, &opts).unwrap_or_else(|e| panic!("migrate: {e}\n{src}"))
}

/// `src` converted, checked against `want`: the output parses (module doc
/// included), compiles to the IR `src` compiles to, and converting it again
/// changes nothing.
fn check(src: &str, flags: Flags, want: &str) -> MigrateOutcome {
    let out = convert(src, flags);
    assert_eq!(out.output, want, "\n--- from ---\n{src}");
    petal::cst::parse_source_with_module_doc(&out.output, FileId(0))
        .unwrap_or_else(|e| panic!("does not parse: {e}\n{}", out.output));
    match petal::ir_equiv::sources_equivalent(src, &out.output, &[], None) {
        Ok(Ok(())) => {}
        Ok(Err(diff)) => panic!("IR changed: {}\n{}", diff.to_json(), out.output),
        Err(e) => panic!("{e}\n{}", out.output),
    }
    let again = convert(&out.output, flags);
    assert_eq!(again.output, out.output, "not idempotent");
    assert!(again.findings.iter().all(|f| !f.fixable), "{:?}", again.findings);
    out
}

/// `src` is left exactly as written, with no report.
fn untouched(src: &str, flags: Flags) {
    let out = convert(src, flags);
    assert_eq!(out.output, src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
}

#[test]
fn a_one_line_comment_becomes_a_plain_doc() {
    let out = check(
        "# Sum of a list; 0 for the empty list.\npub fn sum(xs)\n  reduce(xs, 0, fn(a, x) a + x end)\nprint(sum([1]))\n",
        PUB,
        "@doc(\"Sum of a list; 0 for the empty list.\")\npub fn sum(xs)\n  reduce(xs, 0, fn(a, x) a + x end)\nprint(sum([1]))\n",
    );
    assert_eq!((out.rewritten(), out.left()), (1, 0));
    assert_eq!(out.findings[0].line, 2);
    assert_eq!(out.findings[0].message, "the comment block above `sum` becomes its `@doc`");
}

#[test]
fn a_block_becomes_a_raw_string_with_its_layout() {
    check(
        "#  Draws a field.\n#\n#  Takes a {bg, fg} style:\n#      draw(r, {bg: :000})\npub fn draw(r, style)\n  r\n",
        PUB,
        "@doc(\"\"\"\nDraws a field.\n\nTakes a {bg, fg} style:\n    draw(r, {bg: :000})\n\"\"\")\npub fn draw(r, style)\n  r\n",
    );
}

#[test]
fn braces_quotes_and_backslashes_take_a_raw_string() {
    check("# The {x, y} of it.\npub fn f()\n  1\n", PUB, "@doc(\"\"\"The {x, y} of it.\"\"\")\npub fn f()\n  1\n");
    check("# Say \"hi\" to it\npub fn f()\n  1\n", PUB, "@doc(\"\"\"Say \"hi\" to it\"\"\")\npub fn f()\n  1\n");
    check("# A \\ path.\npub fn f()\n  1\n", PUB, "@doc(\"\"\"A \\ path.\"\"\")\npub fn f()\n  1\n");
    // Ending in a quote: the closing `"""` goes on a line of its own.
    check("# Returns \"ok\"\npub fn f()\n  1\n", PUB, "@doc(\"\"\"\nReturns \"ok\"\n\"\"\")\npub fn f()\n  1\n");
    check("# \"ok\" or nil.\npub fn f()\n  1\n", PUB, "@doc(\"\"\"\n\"ok\" or nil.\n\"\"\")\npub fn f()\n  1\n");
    // A `"""` in the text: an escaped plain string.
    check(
        "# Strips \"\"\" from {s}.\npub fn f(s)\n  s\n",
        PUB,
        "@doc(\"Strips \\\"\\\"\\\" from \\{s}.\")\npub fn f(s)\n  s\n",
    );
}

#[test]
fn every_declaration_kind_takes_its_doc() {
    let src = "# A point.\npub class P\n  x: int,\n\n# A shape.\npub enum Shape\n  dot,\n  box(w),\n\n\
               # X of a point.\nfn P.left(self)\n  self.x\n\n# The host's clock.\npub native fn host_now() -> float = \"time\"\n\n\
               # The limit.\npub let LIMIT = 4\n\n# A counter.\npub var n = 0\n\n# The frame count.\npub state frames = 0\n\n\
               # Cached.\npub fn cached(k)\n  # The square.\n  memo(k) sq = k * k\n  sq\n";
    let want = "@doc(\"A point.\")\npub class P\n  x: int,\n\n@doc(\"A shape.\")\npub enum Shape\n  dot,\n  box(w),\n\n\
                # X of a point.\nfn P.left(self)\n  self.x\n\n@doc(\"The host's clock.\")\npub native fn host_now() -> float = \"time\"\n\n\
                @doc(\"The limit.\")\npub let LIMIT = 4\n\n@doc(\"A counter.\")\npub var n = 0\n\n@doc(\"The frame count.\")\npub state frames = 0\n\n\
                @doc(\"Cached.\")\npub fn cached(k)\n  # The square.\n  memo(k) sq = k * k\n  sq\n";
    let out = convert(src, PUB);
    assert_eq!(out.output, want);
    assert_eq!(out.rewritten(), 7);
    // `--private` takes the method too (a method is never `pub`).
    let out = convert(src, PRIVATE);
    assert!(out.output.contains("@doc(\"X of a point.\")\nfn P.left(self)"), "{}", out.output);
    assert_eq!(convert(&out.output, PRIVATE).output, out.output);
}

#[test]
fn a_private_declaration_needs_private() {
    let src = "# Helper.\nfn helper()\n  1\n# Shown.\npub fn shown()\n  helper()\n";
    check(src, PUB, "# Helper.\nfn helper()\n  1\n@doc(\"Shown.\")\npub fn shown()\n  helper()\n");
    check(src, PRIVATE, "@doc(\"Helper.\")\nfn helper()\n  1\n@doc(\"Shown.\")\npub fn shown()\n  helper()\n");
}

#[test]
fn only_a_block_directly_above_counts() {
    // A blank line between: a comment about what follows, not a doc.
    // (Not the file's first block, which would be its `@module_doc`.)
    untouched("print(0)\n# Loose note.\n\npub fn f()\n  1\n", PUB);
    // A trailing comment, and one in the body.
    untouched("pub fn f() # trailing\n  # in the body\n  let x = 1\n  x\n", PRIVATE);
    // The end of the previous body, indented, is not above the next one.
    untouched("pub fn f()\n  1\n  # about f's body\npub fn g()\n  2\n", PUB);
    // A declaration already documented keeps its comment.
    untouched("# Old words.\n@doc(\"New words.\")\npub fn f()\n  1\n", PUB);
}

#[test]
fn a_nested_declaration_is_left() {
    untouched("pub fn outer()\n  # Inner helper.\n  fn inner()\n    1\n  inner()\n", PRIVATE);
}

#[test]
fn a_ruler_stays_a_comment() {
    // A ruler above a doc, in one block: the doc is what follows it.
    check(
        "# ── Switch ────────\n# The on/off switch.\npub fn switch(on)\n  not on\n",
        PUB,
        "# ── Switch ────────\n@doc(\"The on/off switch.\")\npub fn switch(on)\n  not on\n",
    );
    // A ruler directly above the declaration: no doc.
    untouched("# The switch.\n# ==========\npub fn switch(on)\n  not on\n", PUB);
    untouched("# ── Switch ──\n\npub fn switch(on)\n  not on\n", PUB);
}

#[test]
fn notes_and_licences_stay_comments() {
    untouched("# TODO: make this faster.\npub fn f()\n  1\n", PUB);
    untouched("# FIXME(andy) wrong for 0\n# NOTE: see g\npub fn f()\n  1\n", PUB);
    // Mixed with text: reported, left for a hand edit.
    let src = "# Adds one.\n# TODO: overflow\npub fn inc(x)\n  x + 1\n";
    let out = convert(src, PUB);
    assert_eq!(out.output, src);
    assert_eq!((out.rewritten(), out.left()), (0, 1));
    assert!(out.findings[0].message.contains("split it by hand"), "{}", out.findings[0].message);
    // A word that only starts like one is text.
    check("# NOTEBOOK pages.\npub fn f()\n  1\n", PUB, "@doc(\"NOTEBOOK pages.\")\npub fn f()\n  1\n");
}

#[test]
fn a_directive_stays_directly_above_its_declaration() {
    check(
        "# Adds one.\n# petal-lint-ignore unused-binding\npub fn inc(x)\n  x + 1\n",
        PUB,
        "@doc(\"Adds one.\")\n# petal-lint-ignore unused-binding\npub fn inc(x)\n  x + 1\n",
    );
    // An ignore of the migration itself keeps the comment.
    untouched("# Adds one.\n# petal-lint-ignore comment-to-doc\npub fn inc(x)\n  x + 1\n", PUB);
    untouched("# petal-lint-ignore-file comment-to-doc\n\n# Adds one.\npub fn inc(x)\n  x + 1\n", PUB);
}

#[test]
fn other_annotations_stay_with_the_declaration() {
    check(
        "# Speed in px/s.\n@tunable(0, 10)\npub let SPEED = 4.0\n",
        PUB,
        "@doc(\"Speed in px/s.\")\n@tunable(0, 10)\npub let SPEED = 4.0\n",
    );
}

#[test]
fn the_opening_block_becomes_the_module_doc() {
    check(
        "# Arithmetic and rounding.\n#\n# Every name is pure.\n\nimport std\n\n# Adds one.\npub fn inc(x)\n  x + 1\n",
        PUB,
        "@module_doc(\"\"\"\nArithmetic and rounding.\n\nEvery name is pure.\n\"\"\")\n\nimport std\n\n@doc(\"Adds one.\")\npub fn inc(x)\n  x + 1\n",
    );
    // A one-liner, and the module doc comes whether or not anything is `pub`.
    check("# A script.\n\nprint(1)\n", PUB, "@module_doc(\"A script.\")\n\nprint(1)\n");
    // Only the first block; the next ones stay comments.
    check(
        "# The file.\n\n# A section note.\n\nprint(1)\n",
        PUB,
        "@module_doc(\"The file.\")\n\n# A section note.\n\nprint(1)\n",
    );
}

#[test]
fn a_block_on_the_first_statement_is_that_statements() {
    // No blank line: the block documents the declaration, not the file.
    check("# Adds one.\npub fn inc(x)\n  x + 1\n", PUB, "@doc(\"Adds one.\")\npub fn inc(x)\n  x + 1\n");
    untouched("# Imports.\nimport std\nprint(1)\n", PUB);
    // A file of comments only has no first statement to be separated from.
    untouched("# Just notes.\n", PUB);
}

#[test]
fn a_shebang_and_a_licence_are_passed_over() {
    check(
        "#!/usr/bin/env petal\n# Copyright 2026 Someone.\n# SPDX-License-Identifier: MIT\n\n# The tool.\n\nprint(1)\n",
        PUB,
        "#!/usr/bin/env petal\n# Copyright 2026 Someone.\n# SPDX-License-Identifier: MIT\n\n@module_doc(\"The tool.\")\n\nprint(1)\n",
    );
    // The shebang directly above the header.
    check("#!/usr/bin/env petal\n# The tool.\n\nprint(1)\n", PUB, "#!/usr/bin/env petal\n@module_doc(\"The tool.\")\n\nprint(1)\n");
    untouched("# Copyright 2026 Someone.\n\nprint(1)\n", PUB);
}

#[test]
fn rulers_framing_the_header_stay() {
    check(
        "# ──────────\n#  PLAYGROUND — an app.\n#\n#  Designed for 1240x820.\n# ──────────\n\nprint(1)\n",
        PUB,
        "# ──────────\n@module_doc(\"\"\"\nPLAYGROUND — an app.\n\nDesigned for 1240x820.\n\"\"\")\n# ──────────\n\nprint(1)\n",
    );
    // A ruler inside the header, or a note in it: a hand edit.
    for src in [
        "# The file.\n# ── Part ──\n# More.\n\nprint(1)\n",
        "# The file.\n# TODO: more\n\nprint(1)\n",
    ] {
        let out = convert(src, PUB);
        assert_eq!(out.output, src);
        assert_eq!((out.rewritten(), out.left()), (0, 1), "{src}");
    }
    untouched("# TODO: everything\n\nprint(1)\n", PUB);
}

#[test]
fn a_file_with_a_module_doc_keeps_its_comments() {
    untouched("# Licence-free header.\n@module_doc(\"M.\")\n\nprint(1)\n", PUB);
    untouched("@module_doc(\"M.\")\n\n# More notes.\n\nprint(1)\n", PUB);
}

#[test]
fn a_settings_trailing_comment_needs_settings() {
    let src = "let gravity  = tunable(980.0)   # px/s^2 at 1.0 g\nlet gain     = tunable(5.0)     # slingshot {pull} gain\n\
               @tunable let steps = 8   # solver steps\nlet plain = 3   # not a setting\nprint(gravity + gain + steps + plain)\n";
    untouched(src, PUB);
    untouched(src, PRIVATE);
    let out = check(
        src,
        SETTINGS,
        "@doc(\"px/s^2 at 1.0 g\") let gravity  = tunable(980.0)\n@doc(\"\"\"slingshot {pull} gain\"\"\") let gain     = tunable(5.0)\n\
         @doc(\"solver steps\") @tunable let steps = 8\nlet plain = 3   # not a setting\nprint(gravity + gain + steps + plain)\n",
    );
    assert_eq!(out.rewritten(), 3);
    // A comment block above a `pub` setting wins over its trailing comment.
    check(
        "# Gravity.\npub let g = tunable(9.8)   # m/s^2\n",
        SETTINGS,
        "@doc(\"Gravity.\")\npub let g = tunable(9.8)   # m/s^2\n",
    );
    // A directive trailing a setting is not a label.
    untouched("let g = tunable(9.8) # petal-lint-ignore unused-binding\n", SETTINGS);
}

#[test]
fn the_migration_is_opt_in() {
    let src = "# Adds one.\npub fn inc(x)\n  x + 1\n\n# The file.\n";
    let all = migrate_source(src, &MigrateOptions::default()).unwrap();
    assert_eq!(all.output, src);
    assert_eq!(all.count(COMMENT_TO_DOC), 0);
    let m = petal::migrate::migration(COMMENT_TO_DOC).unwrap();
    assert!(m.opt_in && !m.batch);
}

#[test]
fn other_text_is_preserved_exactly() {
    // Unformatted code, odd spacing and a later comment block all stay.
    let src = "#   The   header,  spaced.\n\nimport std\n# Adds   one.\npub fn inc( x )\n    x+1   # sum\n\n\n#  stray\nprint( inc(1) )\n";
    check(
        src,
        PUB,
        "@module_doc(\"The   header,  spaced.\")\n\nimport std\n@doc(\"Adds   one.\")\npub fn inc( x )\n    x+1   # sum\n\n\n#  stray\nprint( inc(1) )\n",
    );
}

#[test]
fn library_files_convert_and_keep_their_ir() {
    // The libraries themselves are migrated by hand review (proposal step
    // 5); here the converter only has to produce a gated, idempotent result
    // for each.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for rel in [
        "core/prelude/std.ptl",
        "core-runtime/bloom/src/controls.ptl",
        "examples/games/physics-playground/app.ptl",
    ] {
        let path = root.join(rel);
        let src = std::fs::read_to_string(&path).unwrap();
        let opts = MigrateOptions {
            only: Some(vec![COMMENT_TO_DOC.to_string()]),
            origin: Some(path.clone()),
            doc_private: true,
            doc_settings: true,
            ..Default::default()
        };
        let out = migrate_source(&src, &opts).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(out.rewritten() > 0, "{rel}");
        assert!(out.output.contains("@module_doc("), "{rel}");
        petal::cst::parse_source_with_module_doc(&out.output, FileId(0)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let again = migrate_source(&out.output, &opts).unwrap();
        assert_eq!(again.output, out.output, "{rel}: not idempotent");
    }
}
