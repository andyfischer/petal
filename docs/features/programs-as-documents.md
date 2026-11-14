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
  direct-manipulation goals on elements (`Observable::Element`), the
  `entry-file` selector, and [expression goals](goal-based-editing.md#expression-goals)
  (`StaticExpr`), which reference intents write with.
- **Used by:** in `~/petal-creative`, petal-images (every layer edit of its
  editor) and petal-design (node attributes and token references, the page
  tree, tokens and instance overrides, by output identity through
  `design(...)`'s pages).
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
   `OutputMap`. A host whose vocabulary also emits an index made from the
   document's elements (petal-design's instance expansions, keyed by each
   instance's path) names it with `Runner::with_derived`: its elements are
   mapped, but verifying an edit does not compare it.
2. **Identify.** The `OutputMap` lists every element of the result (through
   lists, record fields, element props and children). Each `OutputNode` has
   an `OutputKey` that survives edits: its `id=` prop when no other output
   shares it, else its construction site and call path (`fn/<tag>#n`,
   `fn/callee()#n`, which do not change when values or attributes do) plus
   an occurrence number for loop iterations, else its index path.
3. **State an intent.** `Intent::SetField { target, field, value }` (`field`
   may be dotted, `style.fill`), `Intent::RemoveField`,
   `Intent::SetFieldExpr` (a [reference](#references-a-field-written-as-an-expression)),
   `Intent::SetText` ([text](#text)), or a `StructureIntent` (below).
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
     argument. It is verified by rerun like any edit, unless the hook's
     `verify` says whether it holds: a host that applies overrides outside
     the program (petal-design's instance `overrides`) checks them itself.

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

## References: a field written as an expression

`Intent::SetFieldExpr { target, field, expr }` says "this field reads
`tok.color.ink`": binding a node to a token, using the `gutter` spacing,
writing a shadow as `shadow(c, radius: 4, y: 3)`. `expr` is a
[`StaticExpr`](goal-based-editing.md#staticexpr--expressions), built
structurally or parsed from text that must be exactly one expression, so a
host's string can never become code.

```rust
let ink = StaticExpr::path("tok.color.ink")?;
doc.apply(&Intent::SetFieldExpr { target: key, field: "fill".into(), expr: ink }, None);
// <rect fill={:0A0A0E} />  →  <rect fill={tok.color.ink} />
```

It goes through the ladder like a value, with these differences:

- **Where it is written.** At the literal the value flows from, when that
  literal is the document's and the value is the whole of it (`fill={:f00}`,
  a call's argument, `let accent = :f00`). A value read out of a record (a
  token, `C.ink` with `let C = tok.color`) or out of another file is not
  rewritten (that would change the token): the expression goes on the
  element literal that reads the field, replacing what is written there. A
  field not written yet is added there. A value read per item from a data
  list goes in its data entry. A literal shared by other outputs gives the
  usual choices, and the host's `OverrideHook` is asked too.
- **Names in scope.** Its names must be bound where it is written: the
  file's top-level bindings and imports, and the parameters and locals of
  the function it lands in. The compiler checks that on the edited document;
  a name it does not know is a refusal naming it. When the literal the value
  flows from is outside the scope of a name (a top-level `let` and a
  function parameter), the element literal inside the function is tried
  instead.
- **Verification by probe.** The value an expression has depends on where
  it is written, so the rerun's field is compared with that: the same edit,
  with the expression wrapped in a probe native (`Runner::probe`), reports
  the value it had there, and the field after the edit must be that value,
  with nothing else changed. (`verify` on its own, as for a host's override,
  compares with the expression's value at the end of the document, which
  needs its names to be top-level ones.)
- **Already holds** when the site is already written as the expression
  (compared as expressions: `m["k"]` is `m.k`).

## Text

`Intent::SetText { target, text }` sets an element's text (its text
children):

| The element | What is written |
|---|---|
| `<text>Hello</text>` | the text between its tags |
| `<text />` | `<text>Hi</text>` |
| `<text>{s}</text>` in `fn hint(s)`, called `hint("Swipe")` | the string `"Swipe"` the hole traces to |
| `<text>{r.title}</text>` in a `map` over `rows` | `rows[1].title` |

A literal built for several outputs asks, as a field does. An element with
child elements, or with text and holes mixed (`<text>{n} items</text>`), has
no one place its text is written, and is refused with the line to edit.

## Committing a choice later

Every edit carries the hash of the source it was resolved against
(`ResolvedEdit::base`). `Document::commit(edit, key)` refuses one whose
source is no longer current (the user picked a choice after another edit,
an undo or a reload), so a stale choice never undoes newer work; resolve
the intent again instead.

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
stay; among an element's children, a `{…}` hole such as `{badge("x")}` is
an entry too, so a helper-made element moves into a group and out again);
an item of the data list a `map` / `each` / `for` reads, so the data list
is edited instead; or `Locked`, with a reason (a `{…}` that builds several
elements and reads no data list is one). A call moved or inserted among an
element's children is written as a `{…}`, and must build one element when
run. An entry moved out of a list takes the comment that trailed it on its
line along, as a comment line above it (`{/* … */}` among children). For
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
- **A `{…}` that builds several elements** is locked unless it reads a
  data list.
- **Text** is set only as one run of text: text split by `<br />` or mixed
  with holes is refused.
- **Names in a reference** are checked by the compiler, and a name bound at
  the site to something else than at the top of the document (a parameter
  shadowing a token) is refused when the two values differ. A name bound
  only at the site (a parameter) is taken; the probe checks the value it
  has there. At a site that runs several times (a loop body, a helper
  called twice) the probe accepts a field equal to any one of the values
  the expression had there.
- **One literal that builds several outputs** (a helper's `<box>` children)
  is refused for structural edits, not offered as a choice.
- **Comments travel with literal_edit's moves.** A comment after an entry on
  its line moves with it; one on its own line above stays. Moving an entry
  out of an element's children leaves its `{/* … */}` comment line behind.
- **Not built:** wrap and unwrap, intents kept as an undo log, a
  multi-element clip, document schemas.
- **Cost.** Every edit reruns the document at least once, and a solve reruns
  it many times. Tracing turns the static cache off. In petal-images a drag
  frame on its largest example takes about 55 ms.
