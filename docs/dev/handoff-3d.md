# Handoff: Step 3d — make the CST the authoritative parse

Part of the source-preservation work; read
[source-preservation-plan.md](source-preservation-plan.md) first for the full
context. This document is self-contained for the next step: a fresh session
needs only this plus the plan.

## Where 3c left things

- `cst::parse_cst(src) -> Rc<GreenNode>` — lex + parse (recording events) +
  `build_tree`. Lossless: `parse_cst(src)?.text() == src` over the whole repo
  corpus. Entry-file spans only (uses `Lexer::new`).
- `cst_project::project(&SyntaxNode) -> Vec<Stmt>` — rebuilds the exact AST
  (shapes **and** `SourceSpan`s) from the red tree. Proven Debug-identical to
  the parser's direct AST over the whole corpus
  (`projected_ast_matches_parser_over_repo_corpus`).
- `cst_project::project_in_file(root, FileId)` — same, for imported modules
  (file-local spans).
- The parser still builds the AST directly; CST recording is a side channel
  behind `record_cst` (`Parser::new_recording`), off by default.

## Goal

Flip authority: every consumer parses **once** into a CST and derives the AST
from it via projection. After this step the green tree is the primary artifact
of parsing, which is what tree-splice editing (`rewrite.rs`) and the `petal
lint` re-indenter need.

## Sub-increments

### 3d-i — Single-parse entry point, all call sites migrated

1. Add to `cst.rs`:
   ```rust
   /// Parse once: lex, parse with CST recording, build the tree, project the
   /// AST from it. The tree is authoritative; the parser's own AST is dropped.
   pub fn parse_source(source: &str, file: FileId)
       -> Result<(Rc<GreenNode>, Vec<Stmt>), String>
   ```
   Body: `Lexer::new_in_file(source, file)` → `Parser::new_recording` →
   `parse_program()?` (keep the returned AST only for the debug assert below) →
   `build_tree` → `cst_project::project_in_file`.
2. During the transition, differential-check in debug builds:
   `debug_assert_eq!(format!("{direct:#?}"), format!("{projected:#?}"))`.
   The corpus tests already prove equivalence; this catches drift on inputs the
   corpus lacks.
3. Migrate the four `Parser::new` call sites to `parse_source` (each currently
   lexes + parses by hand):
   - `module.rs:281` — parses imported modules; pass the module's `FileId`
     (it already lexes via `new_in_file`). This is why `parse_source` takes
     `file`.
   - `rewrite.rs:29` (`parse_ast`) — return the tree alongside the AST; 3d-ii
     builds on it.
   - `cli.rs:630` (`show-ast`) — projection output is identical, so the JSON
     serialization is unchanged.
   - `dot_graph.rs:78`.
4. `parse_cst` becomes `parse_source(src, ENTRY_FILE).map(|(g, _)| g)`.

Validation: full suite green (`cargo test`); no behavioral change anywhere —
projection is proven span-identical, so even error messages and IR spans are
unaffected.

### 3d-ii — Tree splices in `rewrite.rs`

The motivating capability: an edit that replaces a node keeps the comments
inside the replaced region.

1. Add green-tree editing to `cst.rs`: `GreenNode::with_children` /
   `replace_child(index, GreenChild)` (green nodes are immutable + `Rc`-shared,
   so an edit rebuilds the spine — parent chain only, O(depth)). Recompute
   `text_len` on the way up.
2. In `rewrite.rs`, locate the target node in the *tree* (by span, as today),
   splice a replacement subtree (parse the replacement snippet with
   `parse_cst`, take its statement/expression node), and emit `root.text()`
   instead of string-splicing the original source.
3. Trivia policy for a replaced node: keep the old node's leading/trailing
   trivia leaves around the new subtree (that is the comment-preservation win —
   write the failing test first: replace a call that has an inline comment,
   assert the comment survives).

Validation: existing rewrite tests green, plus the new comment-survival
round-trip test.

### 3d-iii — Optional cleanup (do only if 3d-i/ii land cleanly)

- Remove the `record_cst` flag and `Parser::new` (recording always on), or
  leave the flag for the WASM/hot paths if profiling says recording costs —
  measure before deciding.
- Drop the debug assert from 3d-i once the tree has been authoritative for a
  while.
- Unblock the `petal lint` re-indenter
  ([linter-plan.md](linter-plan.md)) — it can now reprint from the tree.

## Gotchas

- **Error paths**: on a parse error the event stream is unbalanced;
  `parse_source` must return the error without calling `build_tree`
  (`parse_cst` already does this — keep that shape).
- **State ids**: projection allocates them in parse order (after a state's
  init expression — nested states in lambdas number first). Already handled in
  `cst_project.rs`; don't reorder its traversal.
- **Spans are char-indexed** and line/column are recomputed from the tree's
  own text inside `cst_project` — imported modules must project with their own
  `FileId` or diagnostics point at the wrong file.
- **`rewrite.rs` splices today are char-offset string edits** on the original
  source. When switching to tree splices, spans of *projected* ASTs from an
  edited tree are positions in the **new** text — anything caching old spans
  across an edit must re-project.
- Shared helpers `parse::parse_color_hex` / `parse::expr_to_assign_target` are
  `pub(crate)` — keep the projection using them so semantics can't drift.

## Validation (definition of done)

1. `cargo test` fully green after each sub-increment.
2. Corpus tests still prove: `parse_cst(src)?.text() == src` and projected AST
   ≡ direct AST (until the direct path is deleted).
3. New test: rewrite of a node containing a comment preserves the comment
   (3d-ii's reason to exist).
