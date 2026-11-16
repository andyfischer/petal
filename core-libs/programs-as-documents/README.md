# programs-as-documents

Petal scripts as application documents. An application's file is a `.ptl`
script; running it produces what the user sees, and every edit the
application makes is written back into the script, keeping the author's
comments and formatting. The design is in
[`docs/aspirational-ideas/programs-as-documents.md`](../../docs/aspirational-ideas/programs-as-documents.md)
("The write path" and part 1); the user-facing page is
[`docs/features/programs-as-documents.md`](../../docs/features/programs-as-documents.md).
This crate implements the write path and
the whole resolution ladder (steps 1 to 6):

```text
gesture ─► intent ─► resolve ─► source edit ─► verify ─► commit ─► save
```

It depends on `petal` with `default-features = false` and uses only its
public API, so an embedder that does not edit documents (the wasm builds)
leaves it out. Petal core supplies the one hook it needs: element-origin
tracing (`Env::enable_element_trace`, `provenance::trace_value`; see
`docs/features/provenance.md`).

Out of scope: document schemas (part 2 of the design) and saved-state
files. Everything a document saves is written into the `.ptl`.

## API

| Item | What it is |
|---|---|
| `DocumentSession` | The source text as the single source of truth: `commit(text, key)` (edits with one key within 1.2 s are one undo step), `begin_gesture` / `end_gesture` / `cancel_gesture` (a drag is one step), `undo`, `redo`, `dirty`, `save`, `poll_disk` (reload a file changed elsewhere; unsaved edits are kept and `notice` says so). |
| `Runner` | How a host runs its documents: the path they load as, a setup closure that registers the host's packages and natives, and an `OutputSource` (`ReturnValue`, or `Buffer { symbol, last }` for a vocabulary whose emit native pushes to an output buffer). `Runner::run(source)` runs with element tracing on. `with_derived(indexes)` names result values the vocabulary derives from the others (an index of elements keyed by where they are, like petal-design's instance expansions): mapped, but not compared when an edit is verified (`DocumentRun::compared_outputs`). |
| `DocumentRun` | One run: the `Env`, the result `outputs`, the printed `log`, the `OutputMap`, `static_outputs()`. |
| `OutputMap` / `OutputNode` | Every element of the result (through lists, record fields, element props and children), with its index `path`, its `Site` (element literal span, call chain innermost first, `fan_out` = how many result elements that literal built) and per-field `FieldProvenance`: `Literal` (span, file, constant, `within` = the record/list literal it was read out of and the path inside it, `shared` = how many outputs read that literal) or `Computed { reason }`. |
| `OutputKey` | Stable identity across runs: `Id` (an `id=` prop no other output has), else `Site` (edit-stable keys for the element literal and each call, `fn/<tag>#n` and `fn/callee()#n`, plus an occurrence index, counted over every output of that site with an id or not, that tells loop iterations apart), else `Path`. Outputs that share an `id=` fall back to `Site`, so no two outputs share a key. |
| `Intent` | `SetField { target, field, value }` (`field` may be dotted: `style.fill`), `RemoveField { target, field }`, `SetFieldExpr { target, field, expr }` (a reference: the field is written as a `StaticExpr`, `tok.color.ink`, `shadow(c, radius: 4)`) and `SetText { target, text }` (the element's text children). |
| `SetFieldExpr` | Resolved like `SetField`, writing the expression: at the literal the value flows from when it is the document's whole literal (not a part read out of a token record); else, or when its names are not in scope there, on the element literal that reads the field; or in the data entry a `map`/`for` read it from; a shared site gives the usual choices, and a host's `OverrideHook` sees the intent too. Names are checked by compiling the edited document: a refusal names the one not in scope. Verified by a **probe**: the same edit with the expression wrapped in a probe native (`Runner::probe`, declared at the end of a scratch copy) reads the value the expression has where it is written, and the rerun's field must equal it. `verify` on its own (an override) compares with the expression's value at the end of the document, so only top-level names verify that way. Already written as the expression (compared as expressions) is `AlreadyHolds`. |
| `SetText` | The text of an element, through a ladder of its own: literal text between its tags (`DirectLiteral`), new text on an element with none (`SingleSite`, `<text />` → `<text>Hi</text>`), the string literal a single `{param}` hole traces to (through parameters and names), or the hole's data entry (`EditData`); a shared literal asks. Text with child elements, or text and holes mixed, is refused with the reason. Text that would not read back as bare text is written as a quoted hole (`{"a \{b}"}`). |
| `resolve(runner, run, intent, opts)` | The ladder, most direct first. **1, direct literal**: edit the literal the field flows from (on the element, a call's argument, a `let` constant, a field of a record literal). **2, single site**: add or remove the attribute on the element literal that built the output, wherever it is. **3, shared site**: the scopes that verify, as choices (below). **4, solve**: a computed field read per item from a data list is edited in that entry; otherwise a numeric one goes to Petal's direct-manipulation solver (best effort, entry-file numbers only, `opts.hold` fields kept). **5, concretize** (only with `opts.concretize`): replace the call that made the output with the literal of its value, then apply the intent. **6**: a reason that names the line to edit by hand. Returns a `Resolution`. |
| `Resolution` | `Edit(ResolvedEdit)` (verified; `step` says which rung, `also_changes` lists other outputs the rerun saw change, e.g. a solve moving a loop's step), `AlreadyHolds`, `NeedsChoice { reason, options }`, `Unresolved { reason, at }`, `Refused { reason }` (the edit would land in another file, which is named; or the rerun did not make the intent hold). |
| `ResolvedEdit::base` / `source_hash` | Every edit `resolve`, `resolve_structure` and `paste` return carries the hash of the source it was resolved against. `Document::commit(edit, key)` returns `Err` for an edit whose base is not the current source (the document changed while a choice was open), instead of undoing that change; `None` (a host's own edit) is not checked. |
| `Choice` / `Scope` | One verified way on: `EditShared` (edit the shared literal or default, all instances change), `EditData` (edit the data entry a `map` callback or a `for` loop read the value from: `rows.map(card)` and `rows[1].title`), `Override` (written by the host's `OverrideHook`). |
| `ResolveOptions` | `scope_policy` (`Ask`, or `Prefer(vec![Scope::Override, …])`: take the first scope that verifies and changes nothing else), `overrides: Option<Rc<dyn OverrideHook>>`, `solve` (default on), `hold`, `concretize` (consent, default off), `allow_other_files`. |
| `OverrideHook` | A host's per-instance override writer (petal-design's instance `overrides`, a named argument): given the run, the output, the intent and its `UseSite` (the innermost call in the document's file), it returns the new source; the resolver verifies it, by rerun, or by the hook's own `verify` when the host applies overrides outside the program (petal-design's are applied in Rust, so the rerun's output can't show one). |
| `data_entry(run, node, prop)` | The data list literal and path (`[2].title`) a field was read from per item, through `map`/`each` callbacks (named or lambda, through helpers) and `for` loops. |
| `solve_field(runner, run, node, field, target, hold, allow_other_files)` | Step 4 alone: a DM v2 best-effort request whose goal names the element itself (`Observable::Element`, from the run's element trace), so no probe native is needed. Holds are added only when the first answer moved a held field (best effort's equal split counts literals that reach only a held goal). |
| `concretize(runner, run, node)` / `concretize_and_apply` | Step 5 alone: replace the innermost call in the document's file whose replacement leaves the result unchanged with the output's literal (`StaticValue::from_value`, keeping field order, int vs float, children; `{r, g, b}` records written back as color literals). |
| `verify(runner, before, node, intent, new_source)` | Re-run and check the intent holds of the same output, and list any other outputs that changed. Runs are compared by `DocumentRun::observed_outputs`: a value with no literal form (a function prop) is an opaque marker, so the rest of an output that holds one is still compared. |
| `Document` | A session, a runner and the latest run: `run()`, `apply(intent, key)` (resolve and commit as one undo step), `apply_structure(intent, opts, key)`, `copy(key)`, `paste(clip, at, opts)`, `commit(edit, key)` for an edit the user chose. |
| `StructureIntent` / `resolve_structure(runner, run, intent, opts)` | Structural edits stated against the output: `Reorder { parent, order }`, `Move { target, to: Place }` (within its parent or into another, such as out of a `<group>` or into one), `Insert { at, template }` (`Template::Value` or `Template::Source` text), `Remove`, `Duplicate`. A `Parent` is a list in the result (`Parent::List(path)`, nested lists flattened as a host draws them) or an element's children (`Parent::Children(key)`). Each element of the parent is traced to the source entry that builds it (`sibling_entries`): an entry of a list or element literal in the document (kept text as written: a helper call stays a call, comments stay; among an element's children a `{…}` hole that is not text is an entry too, `Container::holes`), an item of a data list a `map`/`each` callback or a `for` loop reads (the data list is edited instead: `Step::DataList`), or `Locked` with a reason (a library's list in another file, so for `concat(LIBRARY, mine)` only the document's segment is edited; a `{…}` that builds several elements and reads no data list). A helper call moved or inserted among an element's children is written as a `{…}` and must build one element when run. An entry moved out of a list carries the comment that trailed it on its line, as a comment line above it (`{/* … */}` among children). Verified by rerun: the parent must read as asked and nothing else may change. |
| `StructureOptions { write_ids }` | Give new elements (insert, duplicate, paste) an `id=` of their own, so they stay addressable as `OutputKey::Id`. A duplicate of an element with an id always gets a fresh one (`logo-1`); so does a pasted element whose id the target already has. |
| `copy(run, key)` / `copy_text(source, text)` → `Clip` | An output element as source: its literal, or the call that builds it alone (`card(5)` rather than `<card x={x} />`, which reads a parameter), or the literal of its value when no code builds it alone (a loop). Free-variable analysis lists the document's top-level names the text reads (`FreeRef`: constant, function or computed, each use with its path and static value) and the bindings they reach (`ClipBinding`). |
| `paste(runner, run, clip, at, opts)` / `prepare_paste` | Insert a clip at a `Place`, in the same document or another, with one `PasteRef` per reference: `CopyAsSource` (bring the binding and what it reads, before the statement the paste lands in, unless the target binds the name differently), `CopyAsValue` (inline the value read at each use: `tok.ink` → `:112233`), `Rebind(name)` (use the target's binding). Default: a name the target binds the same way is kept, a constant is copied as its value, anything else is refused with a reason naming the reference. |
| `editable_literal(source, name)` | The binding's value when its literal is what the program uses, else `NotEditable::{NotFound, Computed { reason }, RebuiltLater { line }, Parse}`. |
| `splice::edit_span` / `splice::lifted` | Edit one literal at a span with path edits (`LiteralEdit::Set`, `SetExpr`, `Remove`, `SetChildren`), changing only its text (the literal is lifted into a scratch binding and spliced back; new lines are laid out by the document's indent step, `splice::indent_step`). |
| `Runner::probe(source)` | Run a source that calls `PROBE(expr)` around expressions and return the values they had, in call order (how a reference edit is verified). |
| `entries::containers` / `rewrite_entries` | The list and element literals of a source with their entries, and giving one a new sequence of entries (kept, removed, moved, new text) through Petal's style-preserving path edits: the fewest entries move, a removed one takes its line, a new one is laid out like its siblings. |

Only the entry file (the document) is written. An edit whose site is in an
imported module or package is refused with a reason naming the file.

## Used by

- petal-images (`~/petal-creative/petal-images/src/doc.rs`): every layer
  edit of its editor.
- petal-design (`~/petal-creative/petal-design/src/doc.rs`, `src/edits/`,
  `src/overrides.rs`): node attributes (token references as
  `SetFieldExpr`), structure and pages, by output identity through
  `design(...)`'s emitted pages; the instance expansions are a derived
  result value; instance overrides are its `OverrideHook`, which verifies
  them itself.
- petal-movies (`~/petal-creative/petal-movies/src/doc.rs`): every item
  edit, by output identity in the `movie_emit` buffer's items.
- petal-social-media (`~/petal-creative/petal-social-media/src/doc.rs`):
  slides and layers by output identity in the `social_emit` buffer (its
  slide counters are a derived value); "Write out as `<slide>`" is
  `concretize`.

## Not yet

- The data scope finds an entry only when the field is read from the item
  by plain field and constant-index reads; a computation over the item
  (`r.x * 10`) is solved instead, and the solver may move the factor as
  well as the data. Iterations are counted per site and call chain, so a
  filtered loop that skips items can name the wrong entry (the rerun then
  refuses it).
- Solving re-runs the document many times; it is on by default (turn
  `ResolveOptions::solve` off for a cheap resolve). A solve that changes
  other outputs too is returned as an edit with `also_changes`, or as a
  choice beside an override when the host has one.
- Wrap/unwrap as structural intents, intents as an undo log.
- A reference intent's names are checked by compiling the edited document;
  a name bound at the site to something else than at the top of the
  document (a parameter shadowing a token) is refused when the expression
  has a value at the top that differs from its value at the site. A name
  bound only at the site (a parameter, `row.x`) is taken on the probe's
  word. A site that runs several times passes when the field equals one of
  the values the expression had there.
- Structural edits move entry texts with Petal's `move_path`: a comment on
  the line after an entry moves with it (into an element's children as a
  `{/* … */}` line), a comment on its own line above an entry stays where it
  is; moving out of an element's children leaves its `{/* … */}` line
  behind.
- A `{…}` hole among an element's children that builds several elements is
  locked unless it reads a data list; the elements one builds as entries of
  a list literal in the document are that list's entries.
- Text: `SetText` sets text children only; an element whose text is split
  by `<br />` or mixed with holes is refused.
- A data list is recognized only when the `map`/`each`/`for` reads a list
  literal directly (through names); a filtered or sorted list is locked with
  the reason. Several elements built per data item, or a filtered loop, make
  the iteration count disagree with the entry, and the rerun refuses.
- A reorder across two of the document's lists (`xs ++ ys`) is refused with
  a reason; a `Move` carries the entry's text into the other list.
- An edit of an element literal that builds several outputs (a helper's
  `<box>` children) changes them all; the rerun sees that and refuses,
  rather than offering it as a choice.
- Free-variable analysis is syntactic: a name bound anywhere inside the
  copied text counts as bound throughout it, and names the document does not
  bind at top level (builtins, imports) are left as written.
- Loop iterations share one origin in petal's trace, so they are told apart
  by occurrence order; an inserted iteration renumbers the ones after it.
  Write `id=` props.

## Building and testing

The crate builds into petal core's target directory (`.cargo/config.toml`),
so it shares the petal build with core.

```bash
cd core-libs/programs-as-documents
cargo test
```

The tests use a tiny vocabulary (`tests/common`): a `shapes` package with a
`doc(items)` emit native and a helper that builds an element in the package.
