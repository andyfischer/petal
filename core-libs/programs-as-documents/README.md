# programs-as-documents

Petal scripts as application documents. An application's file is a `.ptl`
script; running it produces what the user sees, and every edit the
application makes is written back into the script, keeping the author's
comments and formatting. The design is in
[`docs/aspirational-ideas/programs-as-documents.md`](../../docs/aspirational-ideas/programs-as-documents.md)
("The write path" and part 1). This crate implements the write path up to
step 2 of the resolution ladder:

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
| `resolve(runner, run, intent, opts)` | Step 1, direct literal: edit the literal the field flows from (on the element, a call's argument, a `let` constant, a field of a record literal). Step 2, single site: add or remove the attribute on the element literal that built the output, wherever it is. Returns a `Resolution`. |
| `Resolution` | `Edit(ResolvedEdit)` (verified), `AlreadyHolds`, `NeedsChoice { reason, options }` (the literal or site is shared by several outputs, statically or as seen on rerun; each `Choice` can carry the edit that changes them all), `Unresolved { reason, at }` (computed value, no recorded origin), `Refused { reason }` (the edit would land in another file, which is named; or the rerun did not make the intent hold). |
| `verify(runner, before, node, intent, new_source)` | Re-run and check the intent holds of the same output, and list any other outputs that changed. |
| `Document` | A session, a runner and the latest run: `run()`, `apply(intent, key)` (resolve and commit as one undo step), `commit(edit, key)` for an edit the user chose. |
| `editable_literal(source, name)` | The binding's value when its literal is what the program uses, else `NotEditable::{NotFound, Computed { reason }, RebuiltLater { line }, Parse}`. |
| `splice::edit_span` | Edit one literal at a span with a path edit, changing only its text (the literal is lifted into a scratch binding and spliced back). |

Only the entry file (the document) is written. An edit whose site is in an
imported module or package is refused with a reason naming the file.

## Not yet

- Steps 3 to 5 of the ladder (shared site with a chosen scope, solve,
  concretize): `NeedsChoice` and `Unresolved` are where they plug in.
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
