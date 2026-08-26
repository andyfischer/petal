# Call-path keyed `state` — rules of record

Landed 2026-08-25 on `state-callsite-keying`. This is what survived the plan
that produced it (deleted once executed, as `docs/lowering-confusion-20260726.md`
was before it): the semantics as shipped, the design decisions behind them, and
the one piece still open.

The *user-facing* description lives in docs/language-guide.md ("State"); the
runtime overview is docs/dev/Architecture.md ("State" → "Keying").

## 1. Why

The original intent of `state` (docs/dev/goals.md Goal 2: "Inline `state`
(React-`useState`-like, but a language primitive)") is that a declaration site
allocates state *per use*, the way a React component instance gets its own hook
slots. For two years it was a C `static` local instead: the slot key was a hash
of the variable name alone, so **every callsite of a function shared one slot**,
and two functions declaring the same state name collided silently.

That fought users everywhere it was reached for — `examples/console/particles.ptl`
documented per-particle keying it did not have, `reactive_ui.ptl` advertised a
component model its `button()` widget could not deliver, and `petal-ui`'s
`_theme_slot` accessor existed *only* to launder the one-slot rule.

Now **each call path gets its own slot**: a helper with a `state` in it, called
from three places, holds three independent values. Code that wants one shared
value declares a top-level `state var` and reads/writes it with `get`/`set`.

## 2. Semantics

### 2.1 The call path

Every runtime frame chain from the program root down to a `state` declaration
defines a **path**: an ordered list of parts, one per dynamic step.

- **Call part** — pushed when a function/lambda/method is called; identifies the
  callsite (§3.1).
- **Index part** — pushed per loop iteration (`for`/range/`while`), at *every*
  level of the live frame stack, not just the declaring function's.
- The declaration itself contributes its **declaration id** (§3.1).

A slot is `(decl_id, path)`. Two executions reach the same slot iff they arrive
at the same declaration through the same chain of callsites and loop iterations:

- Multiple callsites → independent slots (the headline change).
- Recursion → one slot per depth, like nested React components.
- A widget function called inside a `for` gets per-iteration slots automatically
  (React's positional list keying) — so reordering the list moves the values.
- Same-named `state` in two functions cannot collide: distinct decl ids by
  construction.

### 2.2 `state(key)` stays **absolute**

`state(expr) name` keys the slot by `(decl_id, hash(key value))` and **ignores
the call path**. It is the escape hatch for "same entity ⇒ same slot, no matter
who asks" — React's `key=` prop, but absolute — and it is load-bearing:

- `garden/examples/panels/plant.ptl` — lineage keying: the same leaf is reached
  one recursion level deeper each growth step; a path component would reset it
  every frame.
- `petal-fantasy-nes/prelude/nes.ptl` (`btn_repeat`) — two widgets asking about
  one button share one repeat phase *by documented design*.
- `nes.ptl`'s `_font_rows` — the build varies by argument, so an absolute key
  caches per ink across all callers.

### 2.3 Top-level `state` / `state var`: unchanged

Module-scope declarations run on the root path (empty), so their behavior — and
their persisted values across the version upgrade, since decl-id derivation is
byte-compatible there (§3.1) — is untouched. Every top-level declaration in the
ecosystem (855 of them at the flip) was unaffected.

### 2.4 The sharing idiom

Intentionally shared, cross-function state is a top-level cell:

```petal
state var theme = default_theme()   // top level: one cell, one path

fn ui_theme()      get theme end
fn theme_set(t)    set theme = t end
```

This replaced the `_theme_slot(write, v)` accessor pattern everywhere it
appeared. `get` is required for a bare cell read inside a function, which keeps
the sharing visible at the read site — a feature, not a cost. `set` goes through
the cell and emits no `StateWrite` at all.

The same idiom covers the *cache* shape, which looks nothing like shared state:
`fn table() state var t = build() … end` has one declaration and no writer and
reads as a memo, but under call-path keying its initializer runs once per path,
so a caller inside a loop rebuilds it every iteration and never hits. Hoist it
to a top-level cell. (Placement matters: a cell whose builder reads a value the
file computes at run time cannot be hoisted above that value, and the compiler
says so — "cannot be hoisted: its body reads a value the file computes at run
time" — before the run fails on `Cannot call nil`.)

### 2.5 Host entry points

`Env::call_function` runs with no caller frame, so a host-invoked function gets
a root path of one call part derived from the function's qualified name
(`hash("host " ‖ name)`). Repeated host calls of one function therefore share
slots with each other — matching embedder expectations for event handlers — but
*not* with in-program calls of the same function. The workaround, as always, is
a top-level `state var`.

## 3. Design

### 3.1 Stable identity

The whole hot-reload contract is "the name hash survives the edit". Nothing
positional survives a reload: `TermId`/`BlockId`/`ClosureId` are dense indexes
rebuilt per compile, source spans shift on any edit above them, and CST nodes
have no persistent ids. So both identity components are **name/structure-derived**.

**Declaration id** (`Compiler::state_key_for`; still a `u64` in the `StateKey`
newtype):

- Top level: `hash(name)` for the entry file, `hash("module::name")` for modules
  — **byte-identical to the pre-flip keys**, so existing programs' hot state
  survived the upgrade.
- In a function: `hash(module ‖ enclosing-fn-name-chain ‖ var name ‖ shadow
  ordinal)`. The fn-name chain handles lexical nesting; lambdas contribute their
  binding name when bound (`let f = x -> …`) and a per-function lambda ordinal
  otherwise. The shadow ordinal disambiguates re-declarations of one name in one
  function.

**Callsite id** (`Compiler::call_site_for`): `hash(canonical callee text ‖
ordinal among identically-spelled callees within the enclosing function)`,
qualified by the same lexical prefix. "Canonical callee text" is the callee
expression with trivia stripped (`f`, `obj.method`, `m::f`). Stability profile:

- Edits anywhere else in the file: path unchanged. ✅
- Renaming the callee, or adding/removing an *earlier* call to the same callee
  in the same function: that callsite's id changes and its subtree of state
  drops on reload. Same class of event as "renaming a state variable drops it",
  already the documented contract (docs/program-modification.md). Accepted.

Rejected: TermId / span identity (reload-fragile); explicit user-visible site
ids in source (invasive — `state(key)` already covers the cases needing manual
control). Also accepted: a **dynamic callee** (`f(x)` where `f` is a parameter)
is keyed by the *text* `f`, so two different closures passed to one callsite
share the callsite part; their state diverges only below, via their own decl
ids. Hashing closure identity instead would be reload-unstable.

### 3.2 Runtime key shape

```rust
enum PathPart { Call(u64), Index(usize), Key(u64) }
struct RuntimeStateKey { base: StateKey /* decl id */, path: SmallVec<[PathPart; 4]> }
```

Explicit-key slots are `{base, path: [Key(h)]}` (absolute, §2.2); top-level
slots keep an empty path. `Stack.state`, `touched_state_keys`, the sweep and
`gc_roots` are all shape-compatible with the old loop-index vector.

Recursion makes paths grow with depth; `SmallVec<[_; 4]>` covers typical UI
trees without allocating. A rolling per-frame hash is designed but deferred; see
§4 for what it would buy.

### 3.3 VM

- `VmFrame` carries the **whole path**, not a parent pointer: `frame_from_pool`
  extends the caller's parts into the pooled vector (`recycle()` clears it but
  keeps the buffer), so a warm pool copies without allocating per call. That
  choice took the shallow-call cost from ~15% to ~5%.
- Loop instructions push/bump/pop `Index` parts into the frame's path
  unconditionally — the old `idx_ctx` gate (always `true`) and the `in_loop`
  static flag are gone.
- `Vm::state_key` is: explicit key ⇒ `[Key(h)]`; else the frame's own path,
  minus `path_pop` innermost parts. There is no walk of the frame stack — the
  composition happened incrementally at push time.
- **Intrinsic closures get the intrinsic's callsite, not a per-element index.**
  `map`/`filter`/`reduce`/`sort`/`forEach` thread the `BuiltinCall` term's
  `call_site` through to the closure, so `map(xs, widget)` gives `widget`'s
  `state` one slot per `map` callsite, shared across elements. §2.1 defines
  `Index` parts as coming from `for`/`while` only; per-element keying here would
  be a follow-up.

### 3.4 Compiler / IR / bytecode

- `Term::call_site: Option<u64>` holds the §3.1 callsite hash — a `Term` field
  rather than a side table or an `Inst` operand, because `Program.terms` already
  *is* the `TermId`-indexed table (array index on the hot call path) and the
  field rides the IR serialization and `ir_equiv` comparison `state_key` already
  used. `Compiler::emit_call_term` is the single construction site, so no call
  reaches the runtime without a path part.
- `Term::path_pop: u32` — the count of loop bodies between a `state` declaration
  and a later access to it, which the VM drops from the live path. Without it a
  top-level accumulator breaks: `state xs = []` with `xs = append(xs, i)` inside
  a `for` would write `{xs,[Index(i)]}` while the `StateInit` and every reader
  address `{xs,[]}`, leaving the persisted slot `[]` plus orphan `[0]/xs …`
  slots. It is always well-defined, because assigning to a captured binding is a
  compile error, so a declaration and its writes are always in one function.
- `state_inits` collisions are a **compile error** rather than a silent
  overwrite: decl ids are unique by construction, so a duplicate means one
  declaration would be silently unreachable.
- `StmtKind::State.id` was deleted from both parse pipelines: it was a global
  parse-order counter, and the shadow ordinal has to be per-function and
  per-name, so the compiler derives its own.
- `ir_equiv` compares `call_site` and `path_pop`. Note the sensitivity change:
  extracting a helper or moving a call is now a *semantic* difference, and
  `ir-equal` reporting it is correct, not noise (docs/CLI.md,
  docs/dev/refactor-verification.md).
- **Hand-written IR and legacy documents degrade rather than fail.** A call term
  with no `call_site` contributes id 0, so every such call shares one part —
  exactly the pre-flip one-slot-per-declaration behavior. A stale `in_loop`
  field deserializes away.

### 3.5 Hot reload

`transfer_stack_state` matches on `base` only and treats the rest of the key as
opaque; path parts are the new opaque tail, so no new mechanism was needed.

- Decl survives the edit ⇒ entries retained. Paths that no longer occur are
  swept by the untouched-key GC after the next full run.
- Call-structure edits (§3.1) silently orphan the old path and init a fresh
  slot. Same failure mode as a rename; documented.

### 3.6 Host surfaces

- `get_state_json` renders top-level slots as bare (module-qualified) names —
  **no change for any existing embedder**, since all ecosystem host-inspected
  state is top-level. Pathed slots render through **one renderer** for every
  part shape: the parts root-to-leaf, `/`-separated, variable name last
  (`counter/count`, `[3]/row/hovered`, `k1234…/leaf`). A pathed name always
  contains a `/`, which no bare name can.
- `Call` parts render as the callee's spelling recovered from the term graph.
  These labels are **display-only and numbered globally**: the `#n` suffix is
  assigned per identical callee spelling in *term* order across the program, not
  per enclosing function as the compiler numbers ordinals. The slot is keyed by
  the hash, never by the string, so a shifted label costs a reader accuracy and
  nothing else.
- `set_state_from_json` / `set_state_map_from_json` stay top-level-only; pathed
  entries are not addressable (already-documented limitation).
- `Env::get_state`/`set_state` synthesize empty-path keys — correct for
  top-level, which is all hosts touch. Debug protocol, SDL protocol and
  web-canvas state sync all keep working.

### 3.7 Optimizer (escape.rs)

State-rooted copy elision applies only where the `StateInit`'s path is
**statically empty** — top-level state outside every loop — because that is the
only case where a base key names one runtime slot. Accesses are checked against
the same rule rather than assumed: a `StateWrite` nested deeper in loops than
its declaration is fine, since `path_pop` drops exactly those `Index` parts,
which is what keeps the top-level accumulator idiom eligible. A later "path is
statically fixed at this site" analysis could win back in-function cases if
profiles demand it.

## 4. Measured cost

Release binaries, pre-flip vs post-flip on identical sources: deep recursion
(depth 300, 6M calls) 3.62 s → 5.06 s (**1.40×**, the O(depth) path copy per
call); `fib(27)` within noise; a 3M-iteration top-level state loop ~6%; a
shallow call-heavy widget tree with no state ~5%. The rolling-hash mitigation
(§3.2) stays designed-but-deferred.

## 5. Still outstanding

- **Structural path repair in `transfer_state`** (out of scope for v1):
  remapping old paths onto new ones when a single callsite ordinal shifted.
  Worth revisiting if the accepted-loss class (deleting the first of two `f()`
  calls hands the survivor the first one's state) causes pain in the Garden
  live-editing workflow.
- **Per-element keying for intrinsic closures** (§3.3), if `map`-driven widgets
  ever want it.
