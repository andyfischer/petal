# programs-as-documents

Petal scripts as application documents. An application's file is a `.ptl`
script; running it produces what the user sees, and every edit the
application makes is written back into the script, keeping the author's
comments and formatting. The design is in
[`docs/aspirational-ideas/programs-as-documents.md`](../../docs/aspirational-ideas/programs-as-documents.md)
("The write path" and part 1). This crate implements the write path and
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
| `Runner` | How a host runs its documents: the path they load as, a setup closure that registers the host's packages and natives, and an `OutputSource` (`ReturnValue`, or `Buffer { symbol, last }` for a vocabulary whose emit native pushes to an output buffer). `Runner::run(source)` runs with element tracing on. |
| `DocumentRun` | One run: the `Env`, the result `outputs`, the printed `log`, the `OutputMap`, `static_outputs()`. |
| `OutputMap` / `OutputNode` | Every element of the result (through lists, record fields, element props and children), with its index `path`, its `Site` (element literal span, call chain innermost first, `fan_out` = how many result elements that literal built) and per-field `FieldProvenance`: `Literal` (span, file, constant, `within` = the record/list literal it was read out of and the path inside it, `shared` = how many outputs read that literal) or `Computed { reason }`. |
| `OutputKey` | Stable identity across runs: `Id` (an `id=` prop), else `Site` (edit-stable keys for the element literal and each call, `fn/<tag>#n` and `fn/callee()#n`, plus an occurrence index that tells loop iterations apart), else `Path`. |
| `Intent` | `SetField { target, field, value }` (`field` may be dotted: `style.fill`) and `RemoveField { target, field }`. |
| `resolve(runner, run, intent, opts)` | The ladder, most direct first. **1, direct literal**: edit the literal the field flows from (on the element, a call's argument, a `let` constant, a field of a record literal). **2, single site**: add or remove the attribute on the element literal that built the output, wherever it is. **3, shared site**: the scopes that verify, as choices (below). **4, solve**: a computed field read per item from a data list is edited in that entry; otherwise a numeric one goes to Petal's direct-manipulation solver (best effort, entry-file numbers only, `opts.hold` fields kept). **5, concretize** (only with `opts.concretize`): replace the call that made the output with the literal of its value, then apply the intent. **6**: a reason that names the line to edit by hand. Returns a `Resolution`. |
| `Resolution` | `Edit(ResolvedEdit)` (verified; `step` says which rung, `also_changes` lists other outputs the rerun saw change, e.g. a solve moving a loop's step), `AlreadyHolds`, `NeedsChoice { reason, options }`, `Unresolved { reason, at }`, `Refused { reason }` (the edit would land in another file, which is named; or the rerun did not make the intent hold). |
| `Choice` / `Scope` | One verified way on: `EditShared` (edit the shared literal or default, all instances change), `EditData` (edit the data entry a `map` callback or a `for` loop read the value from: `rows.map(card)` and `rows[1].title`), `Override` (written by the host's `OverrideHook`). |
| `ResolveOptions` | `scope_policy` (`Ask`, or `Prefer(vec![Scope::Override, …])`: take the first scope that verifies and changes nothing else), `overrides: Option<Rc<dyn OverrideHook>>`, `solve` (default on), `hold`, `concretize` (consent, default off), `allow_other_files`. |
| `OverrideHook` | A host's per-instance override writer (petal-design's instance `overrides`, a named argument): given the run, the output, the intent and its `UseSite` (the innermost call in the document's file), it returns the new source; the resolver verifies it. |
| `data_entry(run, node, prop)` | The data list literal and path (`[2].title`) a field was read from per item, through `map`/`each` callbacks (named or lambda, through helpers) and `for` loops. |
| `solve_field(runner, run, node, field, target, hold, allow_other_files)` | Step 4 alone: a DM v2 best-effort request whose goal names the element itself (`Observable::Element`, from the run's element trace), so no probe native is needed. Holds are added only when the first answer moved a held field (best effort's equal split counts literals that reach only a held goal). |
| `concretize(runner, run, node)` / `concretize_and_apply` | Step 5 alone: replace the innermost call in the document's file whose replacement leaves the result unchanged with the output's literal (`StaticValue::from_value`, keeping field order, int vs float, children; `{r, g, b}` records written back as color literals). |
| `verify(runner, before, node, intent, new_source)` | Re-run and check the intent holds of the same output, and list any other outputs that changed. |
| `Document` | A session, a runner and the latest run: `run()`, `apply(intent, key)` (resolve and commit as one undo step), `commit(edit, key)` for an edit the user chose. |
| `editable_literal(source, name)` | The binding's value when its literal is what the program uses, else `NotEditable::{NotFound, Computed { reason }, RebuiltLater { line }, Parse}`. |
| `splice::edit_span` | Edit one literal at a span with a path edit, changing only its text (the literal is lifted into a scratch binding and spliced back). |

Only the entry file (the document) is written. An edit whose site is in an
imported module or package is refused with a reason naming the file.

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
- Structural intents (insert, remove, reorder), reference intents (bind a
  field to an expression), intents as an undo log.
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
