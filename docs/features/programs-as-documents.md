# Programs as Documents: Editing a Script Through Its Output

An application can use a Petal script as its saved document. Opening the file
runs the script, and what it produces (a canvas of layers, a list of slides)
is what the user sees and edits. Every edit the application makes is stated
against that output ("layer `logo` has `x = 300`", "move this card before
that one") and written back into the script, keeping the author's comments
and formatting. The script can use variables, helpers and loops; the edit
lands wherever the value really comes from, and the application checks it by
running the edited script again before anything is written.

- **Crate:** [`core-libs/programs-as-documents`](../../core-libs/programs-as-documents/)
  (package `programs-as-documents`, `use programs_as_documents::…`). It
  depends on `petal` with `default-features = false` and uses only its public
  API, so an embedder that does not edit documents (the wasm builds) leaves
  it out. Its [README](../../core-libs/programs-as-documents/README.md) has
  the full API table.
- **In core:** element-origin tracing (below), `StaticValue::from_value`,
  direct-manipulation goals on elements (`Observable::Element`) and the
  `entry-file` selector.
- **Used by:** petal-images in `~/petal-creative` (every layer edit of its
  editor).
- **Tests:** `core-libs/programs-as-documents/tests/` (`cargo test` in that
  directory); the core hook in `core/tests/all/` (element tracing) and the
  DM tests.
- **Design:** [programs-as-documents.md](../aspirational-ideas/programs-as-documents.md)
  ("The write path" and part 1). Part 2 there, a schema for documents, is
  not built.

## A document

A document is any script whose result (its value, or what a vocabulary's
emit native pushes to an output buffer) holds elements:

```petal
let gutter = 24

fn card(row)
  <card id={row.id} x={row.x} y={300} title={row.title} />
end

let rows = [
  {id: "plan", x: 40, title: "Plan"},
  {id: "build", x: 260, title: "Build"},
]

[
  <logo id="logo" x={gutter} y={gutter} />,
  rows.map(card),
]
```

The application never edits this text by position. It says what the output
should be, and the crate finds where to write it:

| Intent | What is written |
|---|---|
| card `build` has `title = "Ship"` | `rows[1].title`, the data entry `map` read it from |
| card `plan` has `y = 320` | a question: `y={300}` builds both cards, so "change all 2" (or an override at the use site, if the host writes overrides) |
| logo has `x = 30` | a question: `gutter` is also the logo's `y` |
| move card `build` before card `plan` | the two entries of `rows` swap |
| copy the logo, paste it into another document | `<logo id="logo" x={gutter} y={gutter} />` plus `let gutter = 24`, or `x={24} y={24}` |

## The write path

```text
gesture ─► intent ─► resolve ─► source edit ─► verify ─► commit ─► save
```

1. **Run.** `Runner::run(source)` runs the script in a fresh `Env` with the
   host's packages and natives registered (a setup closure) and element
   tracing on, and reads the result from the return value or from an output
   buffer (`OutputSource`). The `DocumentRun` holds the result values and an
   `OutputMap`.
2. **Identify.** The `OutputMap` lists every element of the result (through
   lists, record fields, element props and children). Each `OutputNode` has
   an `OutputKey` that survives edits: its `id=` prop when no other output
   shares it, else its construction site and call path (`fn/<tag>#n`,
   `fn/callee()#n`, which do not change when values or attributes do) plus
   an occurrence number for loop iterations, else its index path.
3. **State an intent.** `Intent::SetField { target, field, value }` (`field`
   may be dotted, `style.fill`), `Intent::RemoveField`, or a
   `StructureIntent` (below).
4. **Resolve** with `resolve(runner, run, intent, opts)`, which tries the
   ladder below.
5. **Verify.** Every edit the resolver returns has already been run: the
   intent holds of the same output in the rerun, and any other output that
   changed is listed (`ResolvedEdit::also_changes`), or the edit is refused.
   Nothing unverified is returned as an `Edit`.
6. **Commit and save.** `DocumentSession` keeps the text as the single source
   of truth, with undo and redo as text snapshots. Commits with one key
   inside 1.2 s are one undo step, and `begin_gesture` / `end_gesture` make
   a drag one step. `save` writes the file. `poll_disk` reloads a file
   changed elsewhere, and keeps unsaved edits with a `notice` if there are
   any.

`Document` puts a session, a runner and the latest run together:

```rust
use programs_as_documents::*;

let runner = Runner::new("poster.ptl", |env| register_canvas(env))
    .with_output(OutputSource::Buffer { symbol: "canvas_emit".into(), last: Some(1) });
let mut doc = Document::open(runner)?;           // reads the file, runs it
let key = doc.run()?.map.nodes[0].key.clone();

match doc.apply(&Intent::SetField { target: key, field: "x".into(), value: StaticValue::Int(300) }, Some("drag")) {
    Resolution::Edit(e) => println!("{}", e.description), // committed as one undo step
    Resolution::AlreadyHolds => {}
    Resolution::NeedsChoice { reason, options } => { /* ask; then doc.commit(&options[i].edit?, key) */ }
    Resolution::Unresolved { reason, .. } | Resolution::Refused { reason } => println!("{reason}"),
}
doc.session.save()?;
```

## The resolution ladder

`resolve` tries each step in order, most direct first. Every step ends in the
same rerun check.

1. **Direct literal.** The field's value flows unchanged from one literal:
   the element's own attribute, a call argument, a `let` constant, or a field
   of a record literal. That literal is edited with Petal's
   [path edits](edit-paths.md), so only its text changes.
2. **Single site.** The element does not have the field yet, or the intent
   removes an attribute. The attribute is added to (or removed from) the
   element literal that built the output, wherever that literal is. If the
   literal built other outputs too, the rerun sees them change and the
   resolver asks, as in step 3.
3. **Shared site, chosen scope.** The literal builds several outputs, or
   feeds several fields. The resolver returns `NeedsChoice` with every scope
   whose edit verifies:
   - `Scope::EditShared`: edit the shared literal; every instance changes.
   - `Scope::EditData`: edit the data entry the value was read from per item,
     through `map` / `each` callbacks (named or lambdas, through helpers) and
     `for` loops (`data_entry`).
   - `Scope::Override`: written by the host's `OverrideHook` at the use site
     (the innermost call in the document's own file), for example a named
     argument.

   `ScopePolicy::Prefer(vec![…])` picks the first scope that verifies and
   changes nothing else, without asking.
4. **Solve.** The field is computed (`x={40 + i * 220}`). A field read per
   item from a data list is edited in its data entry; any other numeric field
   goes to [direct manipulation](direct-manipulation.md) (v2, best effort),
   whose goal names the element itself (`Observable::Element`), limited to
   numbers in the document's own file (`entry-file`). Fields in
   `ResolveOptions::hold` (the other axis of a drag) are kept. On by default;
   `solve: false` makes a cheap resolve.
5. **Concretize**, only with consent (`ResolveOptions::concretize`, or
   `concretize_and_apply`). The innermost call in the document's file that
   built the output is replaced by the literal of its value
   (`StaticValue::from_value`), keeping field order, int versus float and
   children, and only if the result is unchanged. Then the intent is applied
   to the new literal.
6. **Refuse with a reason** that names the line to edit by hand
   (`Unresolved { reason, at }`).

An edit whose site is in another file (an imported module, a package) is
`Refused`, and the reason names the file. Only the document is written.

## Structural edits

`resolve_structure(runner, run, intent, opts)` takes a `StructureIntent`
against a `Parent`, which is a list in the result (`Parent::List(path)`,
nested lists flattened the way a host draws them) or an element's children
(`Parent::Children(key)`):

| Intent | Effect |
|---|---|
| `Reorder { parent, order }` | the parent reads in this order |
| `Move { target, to: Place }` | within its parent, or into another (into or out of a `<group>`) |
| `Insert { at, template }` | new text (`Template::Source`) or a value (`Template::Value`) |
| `Remove { target }` | |
| `Duplicate { target }` | the entry's text is copied as written |

Each sibling is traced to the source entry that builds it
(`sibling_entries`): an entry of a list or element literal in the document,
whose text is kept as written (a helper call stays a call, its comments
stay); an item of the data list a `map` / `each` / `for` reads, so the data
list is edited instead; or `Locked`, with a reason. For
`shapes.LIBRARY ++ mine` only `mine` is edited, and nothing moves past a
library element. The rerun must read as asked, with nothing else changed.
`StructureOptions::write_ids` gives inserted, duplicated and pasted elements
an `id=` of their own.

## Copy and paste

`copy(run, key)` gives a `Clip`: the element literal if it builds only this
output and reads no function locals, else the call that builds only this
output (`card(5)`), else the literal of the element's value. Free-variable
analysis lists the document names the text reads. `paste(runner, run, clip,
place, opts)` inserts it, in the same document or another, with a
`PasteRef` per reference: `CopyAsSource` (bring the binding and what it
reads), `CopyAsValue` (inline the value read at each use) or `Rebind(name)`.
By default a name the target binds the same way is kept, a constant is
copied as its value, and anything else is refused with a reason naming it.

## Other pieces

- `editable_literal(source, name)`: the binding's value when its literal is
  what the program uses, or why not (`NotFound`, `Computed { reason }`,
  `RebuiltLater { line }`, `Parse`).
- `splice::edit_span`: edit one literal at a span with path edits, changing
  only its text.
- `entries::rewrite_entries`: give a list or element literal a new sequence
  of entries, moving as few entry texts as possible.

## The core hook: element tracing

`Env::enable_element_trace(true)` makes the VM record, for every element it
builds, the `AllocElement` term that built it, the call site of each
enclosing frame (innermost first) and how many elements that site built
before it (`ElementOrigin { term, chain, occurrence }`, read with
`Env::element_origin(id)`). When tracing is off this costs one bool check in
the `AllocElement` instruction. While it is on, the static cache and memo are
off for that run, because a kept element would carry an earlier run's origin.
`provenance::trace_value` follows one prop back through dataflow to the
literal it was copied from, matching parameters to arguments by the recorded
chain. See [provenance.md](provenance.md#where-an-element-came-from-element-tracing).

## Limits

- **Deterministic documents only.** Verification compares reruns, so
  randomness must be seeded and time must be an input.
- **Loop iterations are told apart by order.** An inserted iteration
  renumbers the ones after it; write `id=` props (`write_ids`).
- **Narrow data scope.** A data entry is found only through plain field and
  constant-index reads of the item. A computation over the item
  (`r.x * 10`) is solved instead, and the solver may move the factor. A
  filtered or sorted list is locked for structural edits.
- **Holes among an element's children are not entries.** A helper call
  between an element's tags can't be moved or inserted there; elements a
  `{…}` hole builds are edited through their data list, or are locked.
- **Text children** (`<text>{t}</text>`) have no intent of their own.
- **One literal that builds several outputs** (a helper's `<box>` children)
  is refused for structural edits, not offered as a choice.
- **Comments travel with literal_edit's moves.** A comment after an entry on
  its line moves with it; one on its own line above stays.
- **Not built:** reference intents (bind a field to an expression), wrap and
  unwrap, intents kept as an undo log, a multi-element clip, document
  schemas.
- **Cost.** Every edit reruns the document at least once, and a solve reruns
  it many times. Tracing turns the static cache off. In petal-images a drag
  frame on its largest example takes about 55 ms.
