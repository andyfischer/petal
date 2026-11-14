# Programs as documents

Status: aspirational, 2026-11-14. Nothing here is a commitment; it is a map
of the problem and a direction.

## The idea

An application's file is a Petal script. Opening the file runs the script.
What the script produces (a canvas, a movie timeline, a set of screens) is
what the user sees and edits. Saving writes the script back. Copying the
file copies the document. A person can open the same file in a text editor,
change it by hand, and the application picks the change up.

This gives a document format some unusual properties:

- **It is readable and diffable.** A document is source a person can review.
- **It can abstract.** Where a fixed format would make the user repeat
  themselves, a document can use a variable, a helper function or a loop.
- **The application and the hand editor are peers.** Neither owns the file.

The cost is the write path. A JSON file is written by serializing a data
structure, so any edit is easy. A script is written by *editing a program*,
and the program may be shaped in ways the application did not anticipate.
Most of this note is about that.

A document has up to three parts, and they should be kept apart:

| Part | Where it lives | Who writes it | Required to open? |
|---|---|---|---|
| **Content** | the `.ptl` script (or a folder of modules) | the user, by hand or through the app | yes |
| **Saved state** | an optional JSON file beside the script | the runtime and the app | no |
| **Results** | memory, maybe a cache | the run | no, always recomputed |

## What petal-creative does today

The three tools in `~/petal-creative` (petal-images, petal-movies,
petal-design, plus petal-social-media) already treat scripts as documents.
Their shared pattern:

**A document is a script that ends in one call.**

```petal
import movie

let project = {width: 1080, height: 1920, fps: 30, background: :0b0b12}

let items = [
  <video src="media/city.mp4" start={0.0} duration={4.0} />,
  <text start={0.3} duration={3.5} y={420} size={120}>PETAL MOVIES</text>,
]

movie(project, items)
```

The vocabulary package (`movie`, `canvas`, `design`) supplies the
element vocabulary and a native that hands the result to the host.

**The source text is the single source of truth.**
`creative_core::source::SourceFile` holds the text, undo and redo as text
snapshots, edit coalescing (a drag is one undo step, keyed and timed), save,
and `poll_disk`, which reloads the file when another program changed it and
keeps unsaved local edits if there are any. After each edit the script runs
again and the view is rebuilt from the result.

**Editable places are found by convention, in Rust.** Each tool knows which
top-level bindings hold its content: the last `let items = [...]` and
`let project = {...}`, the last `let pages = [...]`, the record passed to
`tokens({...})`, and the `component("name", f)` entries. Helpers such as
`editable_list`, `pages_locked` and `component_sites` check that each one is
a literal the editor can write into, and give a reason when it isn't
("`pages` is built by code, not written as a list; edit the script by
hand"). The editor shows such parts read-only.

**Edits are goals on literals, plus text splices.** Attribute changes are
Petal [goal-based edits](../features/goal-based-editing.md). Because goals
don't step into an element's children, `edit_in_place` lifts one element into
a scratch source, edits it there, and splices it back. Structural edits
(insert, remove, move, wrap, paste) are line splices re-indented by hand.
Petal's [edit paths](../features/edit-paths.md) now reach into element
children and do insert, remove and move as minimal splices, so most of this
can move upstream.

**Computed parts are moved by outcome.** A layer made by a `for` loop or a
helper has no literal of its own. `solve_computed` asks [direct manipulation](../features/direct-manipulation.md)
(v2, best effort) which literals to change so the layer lands where it was
dropped. If no answer stays in the document's own file, the edit is refused
and nothing is written.

**What is missing.** The gaps are logged in
`~/petal-creative/docs/petal-upstream-ideas.md`. The ones that matter here:

- No single call says "this binding's literal is the value the program
  uses". Each host repeats a text check plus a static read.
- A goal can only write a value. The petal-design inspector can't write
  `fill={tok.color.ink}` or named arguments (`modes(light: …, dark: …)`), so
  it splices text.
- Direct manipulation can't see an element's attributes, so the solver
  injects a probe native.
- There is no way to turn an evaluated value back into source ("write out as
  `<slide>`"), and no way to limit a solve to the entry file.
- The host reads the result from an output buffer, not from the script's
  return value.

Each tool also has its own idea of the document's shape, written as Rust
that walks the AST. That is a schema, but nobody can read it, check it or
share it. The two halves of this note are about making that resolution more
flexible (part 1) and making the schema explicit (part 2).

## The write path

Every edit from the application should go through one pipeline:

```
gesture ─► intent ─► resolve ─► source edit ─► verify ─► commit ─► save
```

1. **Gesture.** A drag, an inspector field, a menu command, a paste.
2. **Intent.** What the user meant, stated against the *output*, not the
   source: "layer `logo` has `x = 300`", "page `home` comes before page
   `about`", "this node uses token `color.ink`", "these three nodes become a
   component".
3. **Resolve.** Find where in the program that intent can be satisfied (see
   part 1). The result is a set of goals or splices, a question for the user
   ("change all twelve cards, or just this one?"), or a refusal with a
   reason.
4. **Source edit.** Apply the goals, keeping formatting and comments.
5. **Verify.** Parse, run, and check that the intent now *holds in the
   output*. This is the outcome check, and the safety net for every heuristic
   in step 3: an edit that parses but doesn't produce what the user asked for
   is refused, and nothing is written.
6. **Commit.** One undo step. A continuous gesture writes the running program
   without writing the file (`Env::set_config_value`) and commits once at the
   end.
7. **Save.** Write the text, and the saved state if there is any.

### Kinds of edit

| Kind | Example | Resolves to |
|---|---|---|
| Value | set `x`, change a colour | a path goal on a literal |
| Structure | insert, remove, reorder, duplicate, wrap | list and element goals, or splices |
| Reference | bind a fill to a token; use the `gutter` spacing | an *expression* goal, not a value |
| Abstraction | make a component from a selection; extract a helper | new declarations plus replacements at the use sites |
| Concretion | detach an instance; "write out as `<slide>`" | replace a call with the literal of its evaluated value |
| Computed | drag a card that a loop placed | a direct-manipulation solve |
| External | the file changed on disk | reload, and reconcile any unsaved local edits |

### Intents as the undo log

Because each edit starts as an intent, the application can keep the intents
as well as the text snapshots. That opens two things a text-only undo stack
can't do:

- **Rebasing onto outside changes.** When the file changes on disk while the
  app holds unsaved edits, the app can re-resolve its intents against the new
  text instead of asking the user to choose one version. An intent whose
  target is gone is reported, not silently dropped.
- **Replaying intents on a copy.** "Apply the same edits to the other
  three variants" is the same intents resolved against different programs.

## Part 1: Resolving edits against programs of many shapes

The convention approach finds edit sites by *where they sit in the source*:
"the last `let pages`, entry 2, element child 0". That breaks as soon as the
author structures the document differently: a helper function, a loop, a
`{...}` hole that shifts the child numbering, a component in another module.

[Outcome-based programming](../../README.md#outcome-based-programming) turns
it around: find edit sites by *what the program produced*. The intent names a
place in the output, and the runtime traces backward to the source that made
it. It works for any document whose output can be traced back to source,
whatever its layout.

### An output source map

The primitive this needs is a source map for outputs. For each value the
script hands the host, the run records:

- **Construction site.** The span of the expression that built it (an
  element literal, a record literal, a call).
- **Call path.** How execution reached that site, with any loop `key`. The
  same idea is already used to identify `state` slots.
- **Per-field provenance.** For each attribute or field, the term it came
  from, and whether that term is a literal reached through plain data flow
  (bindings, arguments, field reads) or computed.
- **Fan-out.** How many outputs this run built from the same site.

Most of the pieces exist. Emit tracing (`petal run --trace-emits`) answers
"which call drew this, and which literal moves it". `explain` walks value
chains. Call paths identify state. What's missing is putting them together
for *element-valued* output and handing the result to the host with the
result value. This would also replace the index-path addressing in
petal-design and the probe native in `solve_computed`.

### A resolution ladder

With an output source map, an intent is resolved by trying strategies in
order, from the most direct to the least:

1. **Direct literal.** The field's value flows unchanged from one literal
   (`x={300}`, `let m = 96` … `x={m}`, a helper called with `x: 300`). Edit
   that literal with a path goal. This alone covers helpers, named constants
   and components that pass props through.
2. **Single site.** The output was built by a site that built only this
   output (fan-out 1), say an element literal inside a helper called once.
   Edit the site in place, wherever it is.
3. **Shared site, chosen scope.** The site built many outputs (a loop, a
   component used twelve times). Editing it changes all of them, so the
   application asks, or follows a policy:
   - edit the shared site (all instances change),
   - write an override at the use site (petal-design's instance `overrides`),
   - edit the data that drives the loop. If `cards = rows.map(card)` and the
     user changes card 3's title, the title probably lives in `rows[3]`. Per-field
     provenance says whether it does.
4. **Solve.** The field is computed (`x={100 + i * 200}`). Ask direct
   manipulation which literals to change, under scope limits (entry file
   only, `tunable` preferred, `rigid` never touched).
5. **Concretize.** Nothing can be edited in place, but the user insists.
   Replace the construction with the literal of its evaluated value (detach,
   "write out as source"), then edit that. This trades abstraction for
   editability, so it needs the user's consent.
6. **Refuse with a reason** that names the place to edit by hand.

Steps 1 and 2 are cheap and certain. Steps 3 to 5 are where the user steers.
Every step ends in the same verify-by-rerun check.

### Structural edits by outcome

Reordering, inserting and removing are harder to state as outcomes, but the
same idea applies. The intent is "the output list should read `[a, c, b]`".
Resolution finds the list literal the output list traces to:

- **The output list *is* a literal** (maybe flattened, maybe with `nil`s
  dropped). Map output positions back to entries and diff on identity.
  `drop_edit::diff_goals` already does this with an LCS on `id`/`key`.
- **The output list is a `map` or `for` over a data list.** Reorder the
  *data* list instead. An order-preserving map is easy to recognize in the
  term graph.
- **The output list is a concatenation** (`concat(IOS_COMPONENTS, mine)`).
  Edit only the segment that comes from the document's own literal, and lock
  the rest.

Insertion needs a template: the new entry's source text. It comes from the
vocabulary (a default element), from a copied sibling, or from the
clipboard.

Built: `programs_as_documents::resolve_structure` takes `Reorder`, `Move`
(also between parents, such as into or out of a `<group>`), `Insert`,
`Remove` and `Duplicate`, traces each element of the parent to a literal's
entry, a data list's item or a locked segment (`sibling_entries`), edits the
entries with Petal's path edits, and verifies by rerun.

### Identity across runs

Outcome edits need outputs to keep their identity from one run to the next:
the drag starts on one run's output and commits against the next. The order
of preference:

1. an explicit `id=` written in the source,
2. the construction site plus call path plus loop `key`,
3. the index path, as a last resort. It is fragile, because a hole that
   makes a different number of elements shifts every index after it.

Applications should write ids when they create elements, so a document made
in the GUI stays easy to address after hand edits.
`StructureOptions::write_ids` does this for inserted, duplicated and
pasted elements; a duplicate or paste of an element with an id gets a fresh
one.

### Copy and paste

Copying an element copies its source text, but the text may reference names
the target document doesn't have (`x={m}`, `fill={tok.color.ink}`, a helper).
Free-variable analysis of the copied span gives three options per reference:

- **Copy as source.** Bring the binding along, if the target has no
  conflicting name.
- **Copy as value.** Inline the reference's static value (`x={96}`).
- **Rebind.** Map it to the target's equivalent (the same token name in the
  target's token set).

petal-design copies node source today (`copy_nodes`). Pasting into a
different document is where this matters.

Built: `programs_as_documents::copy` and `paste`, with `PasteRef::{CopyAsSource,
CopyAsValue, Rebind}` per reference. By default a name the target binds the
same way is kept, a constant is copied as its value, and anything else is
refused with a reason naming the reference.

## Part 2: A schema for documents

Arbitrary programs can't all be edited safely, and part 1 only widens what
can be. An application still needs a *contract*: the shape a script must have
so that the app can always open it, edit it and save it without surprises.
Today that contract is implicit in each tool's Rust.

### Editability levels

A schema doesn't have to be all or nothing. It can sort each part of a
document into a level, the generalized version of petal-creative's "locked,
with a reason":

| Level | The part is… | The app can… |
|---|---|---|
| **Literal** | a literal of the expected shape | edit anything in it |
| **Literal with holes** | a literal whose entries are partly code (`[<slide/>, hero(x)]`) | edit the literal entries, reorder all of them, edit holes by outcome |
| **Traceable** | built by code, with outputs that trace to literals (part 1, steps 1 and 2) | edit fields by outcome |
| **Solvable** | computed, smooth in its literals | move it by solving, with consent |
| **Opaque** | anything else, or in another module | show it read-only, and say why |

### Where the schema lives

The schema belongs with the vocabulary package, written in Petal, so the
tool, the CLI and an agent all read the same one. A sketch, with every name
open:

```petal
@document(
  emit: movie,
  bindings: {
    project: literal(MovieProject),
    items: list_literal(of: one_of(<video/>, <image/>, <text/>, <audio/>), ids: "optional"),
  },
  imports: ["movie", "./*"],
  natives: [],
)
pub native fn movie(project, items)
```

What it states:

- **The entry call.** Which call produces the document, and that the script
  makes it exactly once. That also covers the "host reads an output buffer"
  gap: the runtime checks the single call.
- **The editable bindings.** For each one, its name, its expected shape and
  its minimum level. "`items` must be a list literal" is a hard requirement.
  "Each item should be literal" is a preference, with the rest traceable or
  locked.
- **Element vocabulary.** Allowed tags, attributes and types, so the
  inspector knows what fields exist, and inserted elements get defaults.
- **Capabilities.** Which imports and natives a document may use. A document
  is code, so opening an untrusted one runs code. A schema that allows only
  pure vocabulary functions makes opening a document as safe as opening an
  image.

The schema is checked statically, like
[static values](../features/static-values.md): `petal check --document`
reports, per binding, its level and the reason for any lock, without running
anything. The editor runs the same check on open and after each external
change.

### Strict and lenient documents

- **Lenient** (the default, and what petal-creative does): open anything that
  runs and makes the entry call. Edit what the levels allow and lock the
  rest. Hand-written documents stay welcome.
- **Strict**: refuse to open (or open read-only) a document that doesn't
  meet the schema's hard requirements. For formats that must always be
  editable end to end, such as a template marketplace, or files a non-programmer
  hands to another.

### Invariants a conforming document gets

For a document that meets its schema, the runtime and the app should
guarantee:

1. **Round trip.** Open then save with no edits gives the same bytes.
2. **Locality.** An edit changes only the text it has to. Comments and
   formatting elsewhere survive (already true of goals; see
   [source-preservation.md](../features/source-preservation.md)).
3. **Still conforming.** Every app edit leaves source that parses, runs and
   meets the schema. An edit that would break this is refused.
4. **Outcome holds.** After an edit, the intent holds in the output
   (verify-by-rerun).
5. **Deterministic.** The same source (and saved state) gives the same
   output. Randomness is seeded, and time is an input. Without this, steps 4
   and 5 of the write path mean nothing.

## Saved state

This section is a general idea for other hosts (games and their save files,
scripts with long-running `state`). **petal-creative won't use a state file:**
everything a creative tool saves is written into the `.ptl` itself, so a
document stays one file that a person can read, diff and copy.

Some things should survive closing a document but aren't content:

- **Script state.** `state` slots the document itself keeps: a simulation's
  progress, a procedural generator's accepted seed, a game's save.
- **Session state.** Zoom, scroll, selection, open panels, the playhead.
- **Expensive derived data.** A transcription, a thumbnail, layout caches.

None of these belong in the script. They would churn the diff and fight with
hand edits. They go in a JSON file beside it, for example `poster.ptl` and
`poster.ptl.state.json`:

```json
{
  "petal_state_version": 1,
  "source_hash": "b3:91c4…",
  "state": { "counter/n": 4, "[3]/row/hovered": false },
  "session": { "zoom": 1.5, "selection": ["logo"] },
  "cache": { "transcripts": { "media/city.mp4@sha:…": "…" } }
}
```

Rules:

- **Optional.** Deleting the file gives a valid document in its initial
  state. A document never *needs* its state file to open.
- **Keyed by name and call path**, the identities `state` already has
  ([inline-state.md](../features/inline-state.md#state-from-the-host-side)),
  never by source position. Hosts can already dump every slot as JSON and
  restore them.
- **Reconciled like a hot reload.** On open, the saved `source_hash` is
  compared with the script's. If they differ (someone edited the file
  elsewhere), the state is restored with the same rules as `transfer_state`:
  slots whose declarations still exist keep their values, and the others are
  dropped and reported.
- **Sections with different lifetimes.** `state` is part of the document and
  travels with it. `session` is per user, and may belong in the app's own
  storage rather than beside the file. `cache` can be thrown away at any
  time and should be keyed by content hash.

**Content or state?** If the user would expect to see the change in a diff or
a review, it is content and goes in the script. In petal-creative everything
is content. petal-movies' captions show where the line would fall for a host
that does keep a state file. Transcription is expensive and derived, but the
`<captions>` item's text is something the user edits, so it belongs in the
script. Today the recognizer's word timings (`words={[["Keystone", 0.99,
1.32], …]}`) go into the script too, with a note to leave them alone. That is
data the user never edits but the output depends on. Moving the timings to
the state file's `cache` section, keyed by the clip's hash, would tidy the
script. But a cache can be thrown away, and then rendering means running
recognition again. Until the state file reliably travels with the script,
keeping the timings in the script is the safer choice, and for
petal-creative it is the rule.

**Copying.** "Duplicate" copies the script and its `state` section.
"Use as template" copies only the script. A folder document (modules,
`petal.toml`, relative assets) copies as a folder.

## What Petal would need

| Need | For | Today |
|---|---|---|
| Output source map (site, call path, field provenance, fan-out) for element values | part 1, steps 1 to 3; stable ids | done: element tracing in core, `OutputMap` in `core-libs/programs-as-documents` (steps 1 to 6 of the ladder resolve there) |
| `editable_literal(source, name)` with reasons | schema levels | done: `programs_as_documents::editable_literal` |
| Goals that write expressions and named arguments | reference edits; tokens | text splices in petal-design |
| Goals into element children | structure edits | done in Petal: [edit paths](../features/edit-paths.md) and path goals; petal-creative still uses `edit_in_place` lift-and-splice |
| Structural intents by outcome (reorder, move, insert, remove, duplicate) | structure edits | done: `resolve_structure` in `core-libs/programs-as-documents` (list literals, element children, data lists read by `map`/`for`, concatenation segments) |
| Copy and paste with free-variable analysis | copy and paste | done: `copy` / `paste` in `core-libs/programs-as-documents` |
| `StaticValue::from_value` | concretize ("write out as source") | done in core |
| An `entry` scope selector for direct manipulation | step 4 | done: `ParamSelector::EntryFile` (`entry-file`) |
| Direct-manipulation goals on an element's props | step 4 | done: `Observable::Element { term, occurrence, path }` (no probe native), `ObsValue::Element` |
| The script's single result, read by the host | the schema's entry call | output buffers |
| Document schema annotation and `petal check --document` | part 2 | conventions in Rust |
| State sidecar format with a source hash, and restore-with-report | saved state | `get_state_json` and restore exist; no file format |
| Capability limits for a run (imports, natives) | opening untrusted documents | none |

## Other projects this touches

- **petal-creative.** `SourceFile`, `editable_list`, `list_entries` and the
  lift-and-splice helpers could move upstream, so each tool's `edits/`
  shrinks to its vocabulary and schema. **petal-images is ported**
  (2026-11-14): its `Doc` runs documents with `programs_as_documents::Runner`
  and makes every layer edit (fields, drags, add/insert/remove/duplicate/
  move, group/ungroup, copy/paste) as an intent against the layer's output
  identity, with `ScopePolicy::Prefer([EditData])` and a named-argument
  `OverrideHook`; choices go to the status bar. Its tests check literal
  documents edit byte for byte as the old goal-based editor did. petal-design,
  petal-movies and petal-social-media are not ported yet.
- **garden.** Already turns drag-mode drops into goals with `drop_edit`, and
  restores top-level `state` from JSON for panels. It is the natural place
  for the hand-edit side of the "peers" model, with the intent rebase when
  both sides edit at once.
- **cheesecake and worlds-fair.** The saved-state section is a game save
  file. The content/state split, and reconciling state across a script
  edit, apply directly.

## Open questions

- Should a schema be a Petal value, an annotation, or a type? Types would
  give checking for free, if Petal's types grow element and record shapes.
- How far should the app go to keep abstraction intact? Concretizing is
  always possible and always loses something. Should the app ever do it
  without asking?
- Can intents be stable enough to rebase reliably, or only for the simple
  kinds (value, reorder)?
- Where does session state belong: beside the file, or in the app's own
  storage keyed by path?
- Multi-file documents: when an edit's best site is in an imported module,
  should a document-level schema allow writing there?
