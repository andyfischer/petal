# Call-path keyed `state`

How `state` slots are identified at runtime, and the design decisions behind
it. The user-facing description is the "State" section of the
[Language Guide](../language-guide.md#state); the runtime overview is
[Architecture.md](Architecture.md#keying).

## 1. The rule

A `state` declaration allocates a slot *per use*, the way a React component
instance gets its own hook slots. **Each call path gets its own slot**: a
helper with a `state` in it, called from three places, holds three
independent values. Code that wants one shared value declares a top-level
`state var` and reads/writes it with `get`/`set`.

## 2. Semantics

### 2.1 The call path

Every runtime frame chain from the program root down to a `state` declaration
defines a **path**: an ordered list of parts, one per dynamic step.

- **Call part** — pushed when a function/lambda/method is called; identifies
  the callsite (§3.1).
- **Index part** — pushed per loop iteration (`for`/range/`while`), at *every*
  level of the live frame stack, not just the declaring function's.
- The declaration itself contributes its **declaration id** (§3.1).

A slot is `(decl_id, path)`. Two executions reach the same slot iff they
arrive at the same declaration through the same chain of callsites and loop
iterations:

- Multiple callsites → independent slots.
- Recursion → one slot per depth, like nested React components.
- A widget function called inside a `for` gets per-iteration slots
  automatically (React's positional list keying), so reordering the list
  moves the values.
- Same-named `state` in two functions cannot collide: distinct decl ids by
  construction.

### 2.2 `state(key)` is absolute

`state(expr) name` keys the slot by `(decl_id, hash(key value))` and ignores
the call path. It is the escape hatch for "same entity ⇒ same slot, no matter
who asks" — React's `key=` prop, but absolute. Examples in the tree:

- `garden/examples/panels/plant.ptl` — lineage keying: the same leaf is
  reached one recursion level deeper each growth step; a path component would
  reset it every frame.
- `petal-fantasy-nes/prelude/nes.ptl` (`btn_repeat`) — two widgets asking
  about one button share one repeat phase by design.
- `nes.ptl`'s `_font_rows` — the build varies by argument, so an absolute key
  caches per ink across all callers.

### 2.3 Top-level `state` / `state var`

Module-scope declarations run on the root path (empty), so a top-level
declaration is exactly one slot.

### 2.4 The sharing idiom

Intentionally shared, cross-function state is a top-level cell:

```petal
state var theme = default_theme()   // top level: one cell, one path

fn ui_theme()      get theme end
fn theme_set(t)    set theme = t end
```

`get` is required for a bare cell read inside a function, which keeps the
sharing visible at the read site. `set` goes through the cell and emits no
`StateWrite`.

The same idiom covers the *cache* shape: `fn table() state var t = build() …
end` reads as a memo, but its initializer runs once per path, so a caller
inside a loop rebuilds it every iteration and never hits. Hoist it to a
top-level cell. (A cell whose builder reads a value the file computes at run
time cannot be hoisted above that value; the compiler reports "cannot be
hoisted: its body reads a value the file computes at run time".)

### 2.5 Host entry points

`Env::call_function` runs with no caller frame, so a host-invoked function
gets a root path of one call part derived from the function's qualified name
(`hash("host " ‖ name)`). Repeated host calls of one function share slots with
each other, matching embedder expectations for event handlers, but not with
in-program calls of the same function. For sharing, use a top-level
`state var`.

## 3. Design

### 3.1 Stable identity

The hot-reload contract is "the name hash survives the edit". Nothing
positional survives a reload: `TermId`/`BlockId`/`ClosureId` are dense indexes
rebuilt per compile, source spans shift on any edit above them, and CST nodes
have no persistent ids. So both identity components are derived from names
and structure. Both live in `rust/src/compiler/state_ids.rs`.

**Declaration id** (`Compiler::state_key_for`; a `u64` in the `StateKey`
newtype):

- Top level: `hash(name)` for the entry file, `hash("module::name")` for
  modules.
- In a function: `hash(module ‖ enclosing-fn-name-chain ‖ var name ‖ shadow
  ordinal)`. The fn-name chain handles lexical nesting; lambdas contribute
  their binding name when bound (`let f = fn(x) -> …`) and a per-function
  lambda ordinal otherwise. The shadow ordinal disambiguates re-declarations
  of one name in one function.

**Callsite id** (`Compiler::call_site_for`): `hash(canonical callee text ‖
ordinal among identically-spelled callees within the enclosing function)`,
qualified by the same lexical prefix. "Canonical callee text" is the callee
expression with trivia stripped (`f`, `obj.method`, `m::f`). Stability:

- Edits anywhere else in the file: path unchanged.
- Renaming the callee, or adding/removing an *earlier* call to the same callee
  in the same function: that callsite's id changes and its subtree of state
  drops on reload. This is the same class of event as renaming a state
  variable, already the documented contract
  ([program-modification.md](../program-modification.md)).

A **dynamic callee** (`f(x)` where `f` is a parameter) is keyed by the *text*
`f`, so two different closures passed to one callsite share the callsite
part; their state diverges only below, via their own decl ids. Hashing
closure identity instead would be reload-unstable.

### 3.2 Runtime key shape

```rust
enum PathPart { Call(u64), Index(usize), Key(u64) }
struct RuntimeStateKey { base: StateKey /* decl id */, path: SmallVec<[PathPart; 4]> }
```

(`rust/src/stack.rs`.) Explicit-key slots are `{base, path: [Key(h)]}`
(absolute, §2.2); top-level slots have an empty path.

Recursion makes paths grow with depth; `SmallVec<[_; 4]>` covers typical UI
trees without allocating. A rolling per-frame hash would bound the per-call
copy; it is designed but not built (§4).

### 3.3 VM

- `VmFrame` carries the **whole path**, not a parent pointer: `frame_from_pool`
  extends the caller's parts into the pooled vector, so a warm pool copies
  without allocating per call.
- Loop instructions push/bump/pop `Index` parts into the frame's path
  unconditionally.
- `Vm::state_key` is: explicit key ⇒ `[Key(h)]`; else the frame's own path,
  minus `path_pop` innermost parts. There is no walk of the frame stack; the
  composition happened incrementally at push time.
- **Intrinsic closures get the intrinsic's callsite, not a per-element index.**
  `map`/`filter`/`reduce`/`sort`/`forEach` thread the `BuiltinCall` term's
  `call_site` through to the closure, so `map(xs, widget)` gives `widget`'s
  `state` one slot per `map` callsite, shared across elements. `Index` parts
  come from `for`/`while` only.

### 3.4 Compiler / IR / bytecode

- `Term::call_site: Option<u64>` holds the callsite hash. It is a `Term` field
  because `Program.terms` is already the `TermId`-indexed table on the hot
  call path, and the field rides the IR serialization and `ir_equiv`
  comparison. `Compiler::emit_call_term` is the single construction site, so
  no call reaches the runtime without a path part.
- `Term::path_pop: u32` — the count of loop bodies between a `state`
  declaration and a later access to it, which the VM drops from the live
  path. Without it a top-level accumulator breaks: `state xs = []` with
  `xs = append(xs, i)` inside a `for` would write `{xs,[Index(i)]}` while the
  `StateInit` and every reader address `{xs,[]}`. It is always well-defined,
  because assigning to a captured binding is a compile error, so a
  declaration and its writes are always in one function.
- `state_inits` collisions are a compile error rather than a silent
  overwrite: decl ids are unique by construction, so a duplicate means one
  declaration would be silently unreachable.
- `ir_equiv` compares `call_site` and `path_pop`. Extracting a helper or
  moving a call is a *semantic* difference, and `ir-equal` reporting it is
  correct ([refactor-verification.md](refactor-verification.md)).
- **Hand-written IR degrades rather than fails.** A call term with no
  `call_site` contributes id 0, so every such call shares one part: one slot
  per declaration. See [ir-as-target.md](ir-as-target.md#state-keying).

### 3.5 Hot reload

`transfer_stack_state` (`rust/src/transfer_state.rs`) matches on `base` only
and treats the rest of the key as opaque.

- Decl survives the edit ⇒ entries retained. Paths that no longer occur are
  swept by the untouched-key GC after the next full run.
- Call-structure edits (§3.1) silently orphan the old path and init a fresh
  slot. Same failure mode as a rename.

### 3.6 Host surfaces

- `get_state_json` renders top-level slots as bare (module-qualified) names.
  Pathed slots render through one renderer for every part shape: the parts
  root-to-leaf, `/`-separated, variable name last (`counter/count`,
  `[3]/row/hovered`, `k1234…/leaf`). A pathed name always contains a `/`,
  which no bare name can.
- `Call` parts render as the callee's spelling recovered from the term graph.
  These labels are display-only: the `#n` suffix is assigned per identical
  callee spelling in *term* order across the program, not per enclosing
  function as the compiler numbers ordinals. The slot is keyed by the hash,
  never by the string.
- `set_state_from_json` / `set_state_map_from_json` are top-level-only; pathed
  entries are not addressable.
- `Env::get_state`/`set_state` synthesize empty-path keys, which is correct
  for top-level state, which is all hosts touch.

### 3.7 Optimizer (escape.rs)

State-rooted copy elision applies only where the `StateInit`'s path is
statically empty (top-level state outside every loop), because that is the
only case where a base key names one runtime slot. A `StateWrite` nested
deeper in loops than its declaration is fine, since `path_pop` drops exactly
those `Index` parts, which keeps the top-level accumulator idiom eligible.

## 4. Measured cost

Release binaries, before vs after call-path keying on identical sources: deep
recursion (depth 300, 6M calls) 3.62 s → 5.06 s (1.40×, the O(depth) path
copy per call); `fib(27)` within noise; a 3M-iteration top-level state loop
~6%; a shallow call-heavy widget tree with no state ~5%. The rolling-hash
mitigation (§3.2) is not built.

## 5. Open

- **Structural path repair in `transfer_state`**: remapping old paths onto new
  ones when a single callsite ordinal shifted. Worth revisiting if the
  accepted loss (deleting the first of two `f()` calls hands the survivor the
  first one's state) causes pain in the Garden live-editing workflow.
- **Per-element keying for intrinsic closures** (§3.3), if `map`-driven
  widgets ever want it.
