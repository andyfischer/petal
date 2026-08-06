# Programming by direct manipulation

Point at something a program drew, and find the code that drew it. Then say
what it *should* have been, and get back the edit that makes it so.

That is the whole idea, and Petal supports it at the language level: any value a
script *emits* — a draw command, a log line, a row — can be traced back to the
call that produced it, and from there to each argument's source span and literal
value. A host wires that into a pointer and gets an editor where the canvas is
navigable: hover a shape, the code lights up; and with the argument information,
drag a shape and the code can be rewritten. The second half is goal-based: the
host states the outcome ("this argument should be 55") and the runtime answers
with candidate source mutations ([The goal-based protocol](#the-goal-based-protocol)).

This document is the how-to. For the surrounding embedding patterns (output
buffers, observation, bindings) see [embedding-guide.md](embedding-guide.md); the
API reference lives in `petal::provenance`.

## Contents

- [The five-minute version](#the-five-minute-version)
- [How it works](#how-it-works)
- [Recording: turning tracing on](#recording-turning-tracing-on)
- [Resolving: from an id to source](#resolving-from-an-id-to-source)
- [Picking the right frame](#picking-the-right-frame)
- [Arguments, literals, and rewriting](#arguments-literals-and-rewriting)
- [The goal-based protocol](#the-goal-based-protocol)
- [Tracing live code from the CLI and MCP](#tracing-live-code-from-the-cli-and-mcp)
- [Building a hit test](#building-a-hit-test)
- [Rules that keep it correct](#rules-that-keep-it-correct)
- [Cost](#cost)
- [Design notes: two approaches not taken](#design-notes-two-approaches-not-taken)
- [Worked example](#worked-example)

## The five-minute version

```rust
use petal::env::Env;
use petal::provenance::{self, CallSite};
use petal::source_map::ENTRY_FILE;

let mut env = Env::new();
petal_ui::draw::register_draw(&mut env);

env.enable_emit_trace(true);            // off by default — turn it on before the run
let pid = env.load_program(source)?;
let stack = env.create_stack(pid)?;
env.run(stack)?;

// Drain the values and their attribution together — they are index-aligned.
let sym = env.intern_symbol("draw_commands");
let values = env.take_output_buffer(sym);
let sites  = env.take_output_origins(sym);

let program = env.get_program(pid).unwrap();
for (i, site) in sites.iter().enumerate() {
    let Some(term) = provenance::pick_frame(program, &site.chain, ENTRY_FILE) else {
        continue;                        // nothing attributable
    };
    let Some(call) = CallSite::resolve(program, term) else {
        continue;                        // stale id — see "Rules" below
    };
    println!(
        "command {i} came from {} at line {}",
        call.callee.as_deref().unwrap_or("<anonymous>"),
        call.span.map_or(0, |s| s.start.line),
    );
}
```

## How it works

No new machinery, and nothing re-runs. The pieces were already there:

1. **The lowerer stamps each instruction with its IR term.** When the bytecode
   lowerer emits instructions for a term it records that term as their origin
   (`cur_origin`). This is what error messages already use to say *where*.
2. **The VM passes that term to every native it calls.** It arrives as
   `PetalCxt::origin()`.
3. **So an emitting native knows its own call site**, for free, at the moment it
   pushes a value.

Turning `trace_emit` on makes `PetalCxt::push_output` record that call site
alongside the value it emits. Everything else in this document is *derivation*
from the recorded ids, done later, only for the values something actually asks
about.

```
  script          runtime                       host
  ──────          ───────                       ────
  draw_circle ──► native emits value ──────────► draw command  ─┐
                  + records call chain ─────────► EmitSite       │ index-aligned
                                                                 │
                          ┌── pick_frame ────────────────────────┘
                          └── CallSite::resolve ──► span, callee, args
                                                     (computed on demand)
```

## Recording: turning tracing on

```rust
env.enable_emit_trace(true);    // default context
env.emit_trace_enabled();       // -> bool
env.enable_emit_trace(false);   // stops recording and drops what was recorded
```

Tracing must be on **before the run** — recording happens at the emit, so a
switch flipped afterwards records nothing. Turning it *off* discards what was
recorded, so a later drain can't hand back origins that no longer line up with
the values still sitting in the buffers.

Drain the attribution with `take_output_origins(sym)`, which returns one
`EmitSite` per value of the matching `take_output_buffer(sym)`:

```rust
pub struct EmitSite {
    /// The native's own call site first, then the return address of each
    /// enclosing call, out to the top level.
    pub chain: SmallVec<[TermId; 4]>,
}

impl EmitSite {
    pub fn leaf(&self) -> Option<TermId>;   // the innermost call site
}
```

An empty chain means the runtime had nothing to attribute (or tracing was off).
That is a legitimate answer, not an error.

## Resolving: from an id to source

```rust
let call = CallSite::resolve(program, term)?;

call.term;      // the term it resolved
call.span;      // Option<SourceSpan> — where the call is written
call.callee;    // Option<String> — "draw_circle", when the callee is nameable
call.args;      // Vec<ArgSite>, in call order
```

Spans are Petal's own: **1-based** line and column. Hosts with 0-based editors
convert once, at the boundary.

`callee` is resolved for a static `BuiltinCall`, for a `MethodCall`, and for a
dynamic `Call` through a named value (which is what calling an ordinary Petal
function looks like). Module qualification is stripped, so `ui::draw_circle`
reports as `draw_circle` — the name the user typed. A closure called in place has
no name and reports `None`.

The receiver of a method call and the callable of a dynamic call are not
arguments, so they're skipped: `args[0]` is always the first thing written inside
the parentheses.

## Picking the right frame

**This is the step that is easy to skip and wrong to skip.**

The innermost call site is usually not the one a person means. In `petal-ui`
every `draw_*` name is a *Petal function* wrapping a native, so the leaf of the
chain is a line inside the prelude — true, and useless to someone looking at
their own sketch.

`pick_frame` walks outward to the innermost frame whose span belongs to the file
you are showing:

```rust
let term = provenance::pick_frame(program, &site.chain, ENTRY_FILE)?;
```

This gets both cases right at once:

| The script | What the leaf says | What `pick_frame` says |
| --- | --- | --- |
| `draw_circle(120, 110, 60, …)` | a line in the `ui` prelude | the user's line |
| a `draw_rect` inside `fn swatch(…)` the user wrote | the `draw_rect` in `swatch` | the same — it's their file |

The second row is the point: it does not blindly walk to the outermost frame. It
stops at the first frame in the target file, which for a user-defined helper is
the `draw_*` call inside the helper — where the shape is actually made, and what
you would edit.

When no frame is in the file, it falls back to the chain's leaf rather than
returning nothing: an emit from library code with no user frame above it is real
and shouldn't vanish.

## Arguments, literals, and rewriting

Each `ArgSite` reports not just *where* an argument is, but how safely it can be
rewritten — which is what a drag-to-edit mode needs before it touches a file.

```rust
pub struct ArgSite {
    pub index: usize,                 // 0-based position in the call
    pub term: TermId,
    pub span: Option<SourceSpan>,     // where the argument is written in the call
    pub kind: ArgKind,
    pub literal: Option<Literal>,
    pub literal_term: Option<TermId>,
    pub value: Option<StaticValue>,   // any constant type: string, bool, nil too
}

pub enum ArgKind { Literal, Binding, Computed }

pub struct Literal {
    pub value: f64,
    pub is_int: bool,    // written as an integer — preserve it
    pub negated: bool,   // `-5`; `value` is already negative
}
```

Resolution is not number-only: a string, bool, or `nil` written in the call (or
behind a binding) resolves too, reported through `value` as a
[`StaticValue`](../rust/src/static_value.rs) — the same type goal-based editing
renders back into source. `literal` stays the numeric view, carrying the
spelling data (`is_int`, `negated`) a live drag needs.

### The three kinds

- **`Literal`** — the number is written in the call itself:
  `draw_circle(120, 110, 60, …)`. The span belongs to this call and nothing else
  reads it. The safe case: rewrite it directly.
- **`Binding`** — the argument names something defined elsewhere:
  `let r = 30 … draw_circle(x, y, r, …)`. Still rewritable, but the definition may
  feed other shapes, so changing it moves more than the thing being dragged. Tell
  the user rather than surprising them.
- **`Computed`** — arithmetic, a call, a field read. There is no single number to
  edit; a drag has to refuse, or solve for one.

### The span to actually edit

`span` is where the argument appears *in the call*. For a `Binding` that's the
name, not the number — so writing there would replace `r` with `40`, which is not
what anyone wants. Use:

```rust
let range = arg.editable_span(program);
```

which returns the **literal's own** span (at the definition, for a `Binding`),
falling back to the argument's span when no literal was found.

### Preserving how it was written

`is_int` exists so a rewrite puts back `12` rather than churning it into `12.0` —
across a live drag that difference is the whole diff. `negated` records that the
source wrote `-5`, which lowers to two terms; `editable_span` already covers the
`-`, so replacing that range keeps the sign correct.

### What resolution does and doesn't follow

Identity copies (a name reference, `StateInit`) are followed up to 16 hops, so a
short alias chain still finds its literal. `Phi` is deliberately **not** followed:
its value depends on control flow, so the literal reachable through one is not the
value this call actually saw.

## The goal-based protocol

Everything above answers *where a value came from*. The other direction — the
one a drag, a color picker, or an inspector field actually needs — is a
**goal**: "this thing the program emitted should have been X instead. What do I
change?" The runtime answers with source mutations, and the exchange is
designed to be a conversation, because the honest answer is often plural.

The shape of a full round:

```
  host / IDE                                  Petal runtime
  ──────────                                  ─────────────
  1. run the script with tracing on ────────► values + EmitSites per channel
  2. user grabs something
     (drag, drop, inspector edit)
  3. state a goal:
     "emit #7, arg 2, should be 55" ────────► propose_edits(goal, trace, policy)
                                    ◄──────── one proposal…    → apply it
                                              …or several      → step 4
  4. refine: mark variables
     configurable / static          ────────► propose_edits(same goal, policy)
                                    ◄──────── narrowed (ideally to one)
  5. apply the edit, re-run, re-trace ──────► fresh values, fresh ids
```

The pieces, and where they live:

1. **Observe.** Run with `enable_emit_trace(true)` (and, if computed arguments
   should be solvable, the per-term `TraceBuffer`). Every emitted value now has
   an address: *(channel, index)*, plus its resolved call and arguments.
2. **State a goal.** `direct_manipulation::propose_edits` takes a
   `ManipulationGoal { term, arg_index, new_value }` — the term from
   `pick_frame`, the value as a `StaticValue`. The goal describes the
   *outcome*, never the edit; which text changes is the runtime's answer, in
   the same spirit as `goal_based_editing` for config files.
3. **Read the proposals.** Each `EditProposal` is one concrete replacement
   (span + new text) plus what a chooser needs: the `variable` it edits (or
   none, for a call-site literal), and `shared` — whether other code reads
   that binding, so the edit moves more than the grabbed thing.
   - A literal argument yields exactly one proposal.
   - A binding yields one, at the definition, possibly flagged `shared`.
   - A computed argument (`x + offset`) yields **one per contributing
     variable**, each solved with the values the traced run actually saw:
     making `x + offset` equal 42.5 by moving `x`, and by moving `offset`,
     are both offered.
4. **Refine.** The host narrows with `VarPolicy`: `Static` ("never touch
   `x` — it's the layout grid") removes proposals; `Configurable` ("`offset`
   is the tunable") makes its proposals win over unpinned ones. Policy can
   come from anywhere — a per-sketch settings panel, a `// @config` comment
   convention, or the IDE asking the user the first time a goal comes back
   plural: *"Dragging this changes either `x` or `offset` — which did you
   mean?"* The answer is worth remembering; it is the user teaching the
   editor the sketch's intent.
5. **Apply, re-run, re-trace.** Applying is the host's move (the runtime never
   writes files from `propose_edits`). Term ids are indices into the compiled
   program, so after any edit the old ids are stale by design —
   `CallSite::resolve` returns `None` for them — and the re-run's fresh trace
   is the only source of truth. This is also what keeps the arithmetic solver
   honest: it inverts against *last-seen* values, so a loop-varying operand can
   produce a proposal that lands slightly off; the immediate re-trace shows
   the actual result and the next drag frame corrects it, the same way any
   iterative direct-manipulation loop converges.

What the solver deliberately refuses: an argument that flows through a call, a
comparison, or anything else non-invertible produces *no* proposal for that
branch. A guess that silently rewrites code to mean something else is worse
than a refusal the IDE can render as "this value isn't directly editable —
open the function?"

### Where this can grow

- **Multi-goal requests.** A drag changes x *and* y in one gesture; a batch of
  `ManipulationGoal`s that must resolve consistently (and share one policy
  round-trip) is the natural extension, mirroring how `goal_based_editing`
  already applies goal lists.
- **Goals about the emitted value itself**, not an argument — "this row should
  be 'label'" — resolvable when the emitting call passes the value through
  (arg-level goal derived automatically), refusable when it's constructed.
- **Language-level configurability.** Today policy lives host-side. The
  language is open to carrying it in-source — e.g. a `config` modifier
  (`config let offset = 10`) declaring "this binding is the tuning knob";
  `propose_edits` would then default `Configurable` to config bindings and
  `Static` to the rest, so a bare drag resolves to one edit with no dialog.
  That also gives Garden-style hosts an honest place to render sliders. Not
  built yet; the `VarPolicy` map is deliberately the same shape so the feature
  slots in without changing the protocol.
- **Insertion goals.** "There should be a circle here" (paste, palette drop) is
  `Goal::should_call` territory — `goal_based_editing` already inserts calls
  with placement control; wiring it into the same channel-addressed protocol
  makes create and adjust one API.

## Tracing live code from the CLI and MCP

The protocol is exercisable without writing a host. Both halves ship as CLI
commands, and the dev MCP server exposes them to agents as `TraceEmits` and
`ProposeEdit` (see [dev/mcp-server.md](dev/mcp-server.md)).

**Observe** — run a script and dump every emitted value with its attribution:

```
$ petal run --trace-emits sketch.ptl

Channel 'shapes' (2 emits):
  [0] push_output [line 4] <- 30
      arg 1: computed
  [1] push_output [line 5] <- "label"
      arg 1: literal = "label" (edit line 5)
```

`--json` emits the structured report: per channel, each emit's value, the
resolved call (`term`, `callee`, `span`) and per-argument
`kind` / `value` / `span` / `editable_span`. The `index` within a channel is
the emit address `propose-edit` takes.

**Act** — state a goal against one of those emits:

```
$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 sketch.ptl
2 proposals:
  1. set `x` to 32.5 (line 1)
  2. set `offset` to 22.5 (line 2)
Narrow with --configurable <var> / --static <var>, or apply one by hand.

$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 \
    --static x --apply sketch.ptl
1 proposal:
  1. set `offset` to 22.5 (line 2)
Applied.
```

`--json` returns the proposals with exact spans (line/column/offset both ends)
and replacement text for a harness that applies edits itself; `--apply`
rewrites the file only when policy has narrowed the answer to exactly one
proposal, and refuses otherwise — ambiguity is the caller's to resolve, never
the tool's to guess through.

## Building a hit test

Petal attributes *values*; deciding which value the pointer is over is the host's
job, because only the host knows what the values mean. For a draw-command stream
it is simpler than it sounds.

A frame is a flat, ordered command list painted in order, so **the shape a user
sees at a point is the last one covering it**. That is the entire hit test:

```rust
fn hit_test(cmds: &[Cmd], x: i32, y: i32) -> Option<usize> {
    let mut hit = None;
    for (i, cmd) in cmds.iter().enumerate() {
        if contains(cmd, x, y) {
            hit = Some(i);      // keep going: later commands paint over earlier
        }
    }
    hit
}
```

Scan **forward** and keep the last match rather than scanning backwards and
returning early — any stateful command in the stream (a clip rect, a render
target) has to be read in paint order, and reading it backwards applies each one
to the wrong commands.

No spatial index is needed or wanted: the command list is rebuilt every frame, so
there is nothing to index *across* frames, and a linear scan of a few hundred
commands on one mouse move is not a cost worth a data structure.

Things worth getting right in `contains`, learned from Garden's implementation:

- A full-canvas background fill should never match, or every miss reads as a hit
  on the line that cleared the screen.
- An outline is hit on its **stroke**, not through its hollow middle — a frame
  shouldn't intercept the picture inside it.
- Zero-area shapes (lines) need a pick tolerance of a few pixels or they are
  unhittable.
- Use even-odd for polygons; a convexity-assuming test gets concave shapes wrong.

## Rules that keep it correct

- **Enable before the run.** Recording happens at the emit.
- **Drain origins with values.** They are index-aligned. Taking only the values
  leaves the origins behind to be misattributed to the next run's emits. Clearing
  a buffer clears its origins for the same reason.
- **Resolve against the program that ran.** Term ids are *indices*. An id recorded
  before a recompile or hot reload would resolve happily against whatever now sits
  at that index — pointing the user at unrelated code with total confidence.
  `CallSite::resolve` range-checks and returns `None` instead. **Treat `None` as
  "stale, discard", not as an error**; it is the normal state for one frame after
  a live edit.
- **Skipped values shift indices.** If your decode drops a value (malformed, or a
  command your host ignores), index into the origins by the *original* position
  rather than zipping the two lists — zipping silently shifts every later
  command's attribution onto the wrong call site.
- **One context.** Like observation, recorded ids belong to the run that made
  them; a fork inherits the setting but starts with empty origins of its own.

## Cost

- **Off (the default):** one bool check per emit. Nothing is recorded, nothing is
  allocated. This is the state every production run is in.
- **On:** building the call chain costs a walk of the frame stack per native call,
  and each emit stores a `SmallVec<[TermId; 4]>` — four inline slots, which covers
  a draw call through a wrapper or two without allocating.
- **Resolution:** not paid per frame at all. Spans, argument classification and
  literal values are computed from the recorded ids when something asks. A host
  drawing 500 shapes at 60fps and querying one per mouse move pays for exactly
  that one.

## Design notes: two approaches not taken

Both suggest themselves, and both are worse:

**Run the program twice with different bindings** — once to draw, once with the
draw natives rebound to a spatial-map collector. Appealing because tracing then
costs the production path nothing at all. But the second run has to reproduce the
first *exactly*: same `random()`, same clock, same input, same `state`. It is only
ever as sound as the program is deterministic, and the failure mode is silent —
a shape traced to the wrong line, with no signal that anything went wrong.

**Register extra callbacks per native in trace mode** — sound, and it avoids the
determinism problem. But it puts dispatch on the hot draw path for a feature
almost no run uses, and every native has to opt in.

Recording an id the VM has already computed avoids both: nothing re-runs, so
determinism is irrelevant; nothing is added to the call path except one push
behind a bool check.

## Worked example

A sketch with a helper and a loop:

```petal
let radius = 40

fn swatch(x, y, r, g, b)
  draw_rect(x, y, 46, 46, r, g, b)
end

draw_circle(100, 100, radius, 200, 80, 80)
swatch(60, 200, 220, 120, 90)
```

Hovering the **circle** resolves to line 7:

```
callee:  "draw_circle"
span:    line 7
args[0]: Literal,  value 100, is_int   -> edit `100` on line 7
args[2]: Binding,  value 40            -> edit `40` on line 1 (shared!)
```

Note `args[2]`: the pointer is on line 7, but the number to change is on line 1,
and it is a `Binding` — so an editor should either warn, or offer to inline it
before writing.

Hovering the **swatch** resolves to line 4, not line 8:

```
callee:  "draw_rect"
span:    line 4      (inside `swatch`)
args[0]: Computed    (`x` is a parameter — no literal to edit)
args[2]: Literal, value 46, is_int  -> edit `46` on line 4
```

`pick_frame` walked out of the `petal-ui` prelude but stopped at `swatch`,
because `swatch` is in the file being edited. And the size argument is directly
editable while the position isn't — which is exactly right: dragging this swatch
somewhere else would have to edit the *call* on line 8, and the trace says so by
reporting the position as `Computed` at this frame.

## See also

- [embedding-guide.md](embedding-guide.md) — output buffers, observation, and the
  host↔script channels this builds on.
- `petal::provenance` — the API reference for attribution.
- `petal::direct_manipulation` — the goal-based proposal API.
- `petal::goal_based_editing` — declarative, formatting-preserving edits for
  config-shaped files; the insertion machinery the protocol will grow into.
- Garden's `docs/petal-ide-mode.md` — a complete host implementation: hover a
  shape on the canvas, see the code highlight in the editor beside it.
