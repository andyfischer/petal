# Retained scene output

How a host that re-declares its whole world every frame (a game scene, a
panel) can pay for what changed rather than for the size of the world. This
is Petal's side of Cheesecake's retained optimization plan
(`docs/retained-optimization-plan.md` in the Cheesecake repository), whose
layers are named L1 to L8 there; it is also the "segmented output" item of
P0 in [reactive-rendering-plan.md](reactive-rendering-plan.md). The
prototype that measures it is `core/examples/scene_frame`.

## L3: segmented output from the VM (as built, B1)

**The problem.** With memoization on, a helper whose inputs did not change
is replayed instead of run ([memo-scopes.md](memo-scopes.md)), but its
record kept the values it emitted as a flat copy, and a replay appended them
all to the output buffer again. A host draining the buffer walked every
value: on the static 24,000-item world of `scene_frame` the script took
21 µs and the best host that walks values (`SegHost`, which reuses last
frame's result per value by position) about 360 µs.

**The mechanism** (`core/src/output_segments.rs`, VM half in
`core/src/backend/bytecode/vm/memo.rs`). A host opts a buffer in with
`Env::set_output_segmented(sym, true)` (C: `pb_vm_set_segmented`). For such
a buffer:

- When a memo scope is **recorded**, the values it emitted into the buffer
  are moved out of the buffer (`split_off` at the scope's start, so nothing
  is copied) into a `SegmentBody`, together with the marks its child scopes
  left, and the buffer keeps one `SegMark` (position + `Arc<SegmentBody>`)
  in their place. A parent's body therefore holds its own values and its
  children's bodies by reference, never their values.
- When a scope is **replayed**, the VM pushes one mark for its body and
  appends nothing. (This is also why the script time drops: a replay no
  longer copies the scope's values.)
- The host **drains** a `SegmentedFrame` (`Env::take_output_segments`, C:
  `pb_vm_drain_segments`): the frame's fresh values and segment references
  in order; the definition of every segment it has not been given yet,
  children before parents; and the serials no record or buffer refers to any
  more. The channel holds every body it defined until it lists the serial
  as dropped (the first drain after nothing else holds the body), so the
  values a host was given stay live until it is told to forget them.
- A **cutoff** (a child re-executed during its parent's validation that
  produced the same output) keeps the previous record's bodies, so the host
  is never handed an equal segment under a new serial.

Identity: a segment's **serial** is process-unique (an atomic counter), so a
host cache keyed by serial is never stale, across stacks and reloads. Its
**id** is a SipHash of the call path that ran the scope, stable across
frames and runs of the same program: a new serial with a known id is that
scope's new contents. That is the hook for R5 (an unchanged segment is an
unchanged GPU range; a changed one with the same id can reuse its range) and
for replication (R6).

What stays as before: a buffer is not segmented while emit tracing is on (a
body does not carry origins); scopes that get no record (effectful, folded as
too small, past the table's bounds, cold call sites) leave their values in
the buffer as fresh items; `take_output_buffer` on a segmented buffer
returns it expanded, so a host that has not been taught segments keeps
working; turning segmentation off expands what is undrained, and records
made while it was on replay flat. Bodies are GC roots through the memo
records, the undrained marks and the channel's delivered set; a host may keep
the `Value`s of a definition until its serial is reported dropped
(`kept_definitions_stay_live_until_dropped` checks this with the
record replaced and the collector run before that drain).

**The host contract**, in full: keep a map serial → what you made of the
segment; for each definition build its entry (its child references are
already defined); walk the items, splicing a kept segment wherever one is
referenced; after the walk, forget the dropped serials. `SegmentCache` in
`output_segments.rs` is the reference implementation (it expands back to the
flat buffer), and `VmSegHost` in `scene_frame` the realistic one: a frame is
a list of chunks (a fresh draw, or a kept segment), and bodies are reference
counted by the live segments that declare them, so reaping walks segments
that came or went, not bodies.

**The bridge.** `pb_vm_set_segmented`, `pb_vm_drain_segments` (filling a
`pb_segments`: `pb_seg_entry` items, `pb_segment` definitions with `scope`
and `serial`, and the dropped serials) and `pb_vm_reset_segments` (a host
that lost its cache; the next drain defines everything again), documented in
`petal_bridge.h` ("Segmented output buffers") and in
[embedding-c.md](../usage/embedding-c.md#segmented-buffers-a-static-world-costs-per-scope-not-per-value);
`petal::Vm::set_segmented` / `drain_segments` / `reset_segments` in
`petal.hpp`. Values decode into one view arena per drain, valid until the
next run, as for `pb_vm_drain`.

**Measured** (`scene_frame`, release, Apple M4, load average 2.5 to 7, median
µs per frame; runs agreed within a few µs):

| World | Mode | Script | Host | Total |
|---|---|---:|---:|---:|
| 20,000 meshes, 4,000 bodies, nothing moving (150 frames) | emit + memo (flat, decode everything) | 21 | 3,243 | 3,264 |
| | retained segments (`SegHost`, walks every value) | 22 | 360 | 380 |
| | **VM segments** (`VmSegHost`) | **4** | **4** | **8** |
| 2,000 meshes, 400 bodies, 300 ships (300 frames) | retained segments | 190 | 72 | 262 |
| | **VM segments** | 189 | **38** | **228** |
| 20,000 meshes, 4,000 bodies, 300 ships (150 frames) | retained segments | 212 | 398 | 610 |
| | **VM segments** | 194 | **43** | **238** |
| 2,000 meshes, 400 bodies, 3,000 orbiting ships (`SHIPS=orbit`, 60 frames) | retained segments | 7 | 88 | 96 |
| | **VM segments** | 4 | **21** | **25** |

On the static world the host now handles 2 references a frame instead of
23,840 values; what is left with moving ships is their fresh declarations
(about 0.12 µs each through the planned decode) and, for orbits, sampling
3,000 orbits natively. Every mode's world is hashed every frame and compared
(the orbit variant compares the two hosts that sample orbits among
themselves, since the others do not move declared motion). That check once
failed on the default world for the VM-segments host only: the hosts' field
diff (`same_value`) compared a kept value's contents through its id after
the collector had freed it and reused the slot for this frame's value, so a
moved ship looked unchanged. Ids are generational, so it now compares
contents only while the kept value is live (a fresh value is not rooted
after the drain; a definition's are, until dropped).

Reproduce:

```sh
cd core
CARGO_TARGET_DIR=... cargo run --release --example scene_frame -- 150 20000 4000 0
MODES="emit + memo + retained,emit + memo + VM" cargo run --release --example scene_frame
```

The `skipped/fr` column counts, for the VM-segments mode, the segment
references spliced per frame.

**Checked by** `core/tests/output_segments.rs` (each script under memo off
with a flat buffer, memo on flat, and memo on segmented drained both through
a `SegmentCache` and flat, equal frame for frame through collections,
re-recordings, effectful scopes, cutoffs and a reload, and also runs that
fail inside a scope, a scope emitting into a segmented and a flat buffer, and
recursion past the memo depth bound; plus the identity, drop, liveness and
reset rules) and
`segmented_draw_buffers_reproduce_unmemoized_frames_across_the_corpus` in
`core-libs/petal-ui/tests/memo.rs` (every panel app, its draw buffer
segmented, against the unmemoized run). Breaking the position rebase in the
collapse fails both; dropping the cutoff's body reuse fails
`a_cutoff_keeps_the_serial_the_host_has`. The bridge has
`segmented_buffer_hands_a_replayed_scope_over_as_one_entry` in
`integrations/petal-c-bridge/tests/bridge_tests.cpp`.

**Left for later:**

- Cheesecake's script host does not drain segments yet: that is the engine
  side of L3 (keep decoded draws and body slots per serial, splice, reap per
  segment), on top of L1 (scene natives as emitters) and L2.
- petal-ui and the canvas hosts still drain their draw buffer flat (see
  [reactive-rendering-plan.md](reactive-rendering-plan.md), P0).
- A scope id is a 64-bit hash and may in principle collide; content is
  always looked up by serial, so a collision could only mislead a host that
  reuses resources by id.
- A body is kept per buffer per record; a scope that emits into several
  segmented buffers has one body (and one serial) per buffer.
- `pb_vm_restart` leaves the old stack (and its memo records) alive, so its
  segments are not reported dropped until the VM is destroyed. A reload that
  recompiles drops them.

## L4: shape plans at the bridge (as built, B2)

**The problem.** A C host reads an options record (`body(id, {shape: "box",
pos: p})`) the way Cheesecake's natives do: it checks every field against an
allow-list (`check_options`, O(fields × allowed) string compares), then
finds each option it supports with a linear scan by name
(`petal::Value::get`, about 30 reads per `body` and 40 per `draw_mesh`, most
of them for absent options). Underneath, the bridge's decode copied every
record's keys into the view arena (and found each key's length again by
scanning for its NUL), copied each heap string twice, and a native that
returns a state record built a fresh key list (one `Arc<str>` per key, a new
shape) on every call.

**What changed** (all in `integrations/petal-c-bridge`; no core change):

- **Shape ids in views.** A decoded `PB_MAP` carries its record shape id in
  `integer` (never 0; `petal::Value::shape()` in C++). Every record a literal
  makes shares the literal's shape (`core/src/record.rs`), so the id says
  "same keys, same order" without looking at them.
- **Option plans** (`src/plan.rs`, `pb_plan_*`, `petal::Plan`). A plan lists
  the options a native understands. The first time it binds a record of a
  shape, it maps each option to the field holding it and notes the first
  field that is not an option; after that, binding is a one-entry cache check
  (or a hash lookup by shape id) and a pointer per present option. The host
  reads values by position (`o[MASS]`), and an unknown field is reported
  with its key, so the host keeps its own error text. Keys are only ever
  appended under a shape id (`Shape::push` keeps the id for a uniquely held
  shape), so an entry is checked against the record's field count and a
  grown shape is planned again.
- **Snapshots** (`pb_snapshot_*`, `petal::Snapshot`). A keyed declaration
  keeps one; `update` compares the bound options with the ones held, exactly
  (scalars inline by bits, strings by bytes, records and lists element by
  element; a closure or pending value never compares equal) and returns the
  changed options as a bit set. An option that stays absent costs one
  compare. The comparison is on contents, not heap ids, so it does not
  depend on values staying live between frames (the stale-id hazard of
  L3's `same_value`). A handle compares by slot, serial and class index
  (`PB_HANDLE` carries the class index in `number`), since slots and
  serials are per class: the first object of two classes are both (0, 1).
- **One key copy per shape.** The view arena keeps a key cache (shape id →
  NUL-terminated key text) that survives `reset`, so the per-call argument
  arena of a native copies a shape's keys once in its life and every record
  of the shape points at them. Heap strings and class names are copied
  straight into the arena once.
- **Host-built records share a shape** (`src/builder.rs`, a per-thread
  `ShapeCache` keyed by the key list). A native's state record, a binding or
  a call argument with the same keys gets the same shape: its cost is its
  value vector, and a script's `st.pos` hits the VM's inline cache. The cache
  holds the shape, so a script that adds a field copies it with a fresh id.

**Measured.** `examples/scene_frame.cpp` (CMake target
`petal_bridge_scene_frame`) runs `core/examples/scene_frame/scene.ptl`
through the bridge: 2,000 static city meshes, 400 static crate bodies and 300
moving ships, memo on, the scene natives either as host callbacks applied in
the call (Cheesecake today; `body` returns a state record) or as emitters
drained after the run (L1). The mock host reads every option it supports
(40 for `body`, 20 for `draw_mesh`) and keys bodies by string; every mode's
world (bodies with every option, draws with every float) is hashed every
frame and is identical across modes. Release build, Apple M4, load average
6 to 16 (other builds running), median µs per frame over 300 frames; the
"before" rows are the same program against the bridge at `960b5ad3`, where
only the name-lookup reads exist.

| Mode | Script | Drain (decode) | Host | Total |
|---|---:|---:|---:|---:|
| callbacks, by name, **before** | 2,010 | — | 4 | 2,014 |
| callbacks, by name | 1,786 | — | 4 | 1,791 |
| callbacks, plan | **1,453** | — | 4 | **1,457** |
| callbacks, plan + snapshot | 1,494 | — | 4 | 1,498 |
| emitters + memo, by name, **before** | 217 | 467 | 879 | 1,100 |
| emitters + memo, by name | 216 | 351 | 755 | 972 |
| emitters + memo, plan | 218 | 356 | **522** | **740** |
| emitters + memo, plan + snapshot | 216 | 351 | 517 | 734 |

(Host includes the drain. In callback modes the reads happen inside the
script's calls and count as script time.)

- The bridge changes alone (key cache, one string copy, shared return
  shapes) take the drain from 467 to 351 µs and the callback frame from
  about 2,010 to 1,790 µs. Of that, the shared return shapes are roughly
  50 to 150 µs (switching the builder's cache off, in noisy runs).
- Plans take the host's reads (host minus drain) from about 405 to 165 µs:
  2.4× on reads, and the frame from 1,100 to 740 µs (emitters) or from
  2,010 to 1,450 µs (callbacks). The Rust mock had predicted host 530 → 425
  µs for this step; the measured gain is larger because the mock read 9
  options per body rather than every option the native supports.
- The snapshot applies 300 options a frame instead of 12,000 (the ships'
  `pos`), at about the cost of comparing the parsed descriptions: in this
  mock applying an option is an assignment, so it breaks even. It pays where
  applying is costly (rebuilding a collider or a material), which is where
  Cheesecake's field-by-field body diff is today.
- On the static 24,000-item world (`… 150 20000 4000 0`) the reads go from
  about 2,600 to 1,200 µs, but the drain's decode of 24,000 values (about
  6 ms) dominates: that is L3's to remove, by draining segments.

Reproduce (from the repository root; a build directory off the internal
disk is fine). Configure with `CMAKE_BUILD_TYPE=Release`: without it the
Rust library is still optimized but the C++ host is not, and its reads come
out several times slower:

```sh
cmake -S integrations/petal-c-bridge -B <build> -G Ninja -DCMAKE_BUILD_TYPE=Release -DPETAL_CARGO_TARGET_DIR=<target>
cmake --build <build> --target petal_bridge_scene_frame
<build>/petal_bridge_scene_frame [frames] [city] [crates] [ships]
MODES=emit.lookup,emit.plan <build>/petal_bridge_scene_frame
```

**Checked by** `plan::tests` in the bridge's Rust unit tests (binding by
position across one literal's records, the first unknown field, a grown
shape planned again, a snapshot's changed bits through a spread, a nested
record, an option going absent and a closure; key text shared per shape and
kept across a reset; host-built records sharing a shape, a repeated key, and
a script append not touching the cached shape), and in `bridge_tests.cpp`
`a_plan_reads_options_by_position_and_names_an_unknown_field`,
`a_snapshot_reports_which_options_changed` and
`host_built_return_records_share_a_shape`; `c_smoke.c` binds and diffs a
hand-made view from C.

**Left for later:**

- A record built by a spread that adds a field (`{...base, pos: p}` where
  `base` has no `pos`) gets a fresh shape id on every evaluation, because
  the shared shape is copied before it is extended (`RecordMap::insert`). A
  plan then checks it every call (correct, no saving), and the VM's own
  `GetField` inline caches miss on it too. A transition cache in
  `core/src/record.rs` ((shape id, added key) → child shape, as V8's hidden
  class transitions) would give such records a stable shape.
- The bridge still decodes every value of a drained record, including the
  ones no plan reads; with plans, decoding is most of what is left of the
  host's per-item cost. A plan-driven (lazy) decode is possible but changes
  the view API; L3 segments already remove the decode of replayed scopes.
- Cheesecake does not use plans yet: its `check_options` and `o["…"]` reads
  in `engine/script/src` become one `petal::Plan` per native, and its body
  entry's field diff can become a `petal::Snapshot`, after it vendors this
  bridge (`third_party/petal/VENDORED.md`).
- A plan's cache is per plan, not per (native, VM); a host that shares one
  plan across threads must lock it.

## L5: dependency classes and effect kinds (as built, B3)

**The problem.** Memoization decides everything at run time. It records
every scope it opens and finds out on the next frame whether the record was
worth it; a helper that reads the clock records every frame and validates
never, until the cold-site guard notices (and a site called once a frame is
let through every frame). A static call such as `draw_city(CITY)` is checked
every frame: its arguments compared, its captures compared, its children
walked. And a host native had one way to say it does something: `effect`,
which makes every helper above it unreplayable, whether it is a draw, a
keyed declaration, a one-shot impulse or a raycast.

**The analysis** (`core/src/static_classify/dep_classes.rs`). It runs at the
end of the static-value classifier (`analyze`, so it is computed once per
program and kept with it) and reuses what that pass resolved: which user
function or native each call can reach, captures, phis, cells and blocks.
Its result is `StaticAnalysis::dep_classes()`.

- A **`Deps` bit set per term**: the native rows' input classes (`POINTER`,
  `KEYBOARD`, `CLOCK`, `VIEWPORT`, `HOST_DATA`, `RESOURCES`, `RNG`,
  `BINDINGS`, and new: `PHYSICS`, `NET`; `INPUT` names pointer and keyboard
  together), the program's own sources (`STATE`, `VAR`, `MODULE_VAR` for a
  top-level plain `var`, `ENUM` for dynamic enum data, `UNKNOWN` for a call
  it cannot resolve, `ARG` for a parameter), and what a call does
  (`EFFECT`, which also covers a random draw and a read nothing validates;
  `COMMAND`; `EMIT`; `DECLARE`; `TICKS`). `CONST` is the empty set. A term's
  mask is its own bits, its value inputs' masks, the `CellMay` edges of a
  `var` (every write reaches every read), phi sources, captures at every
  place the closure is made, the control terms it runs under, and for a
  call the summary of every function it can reach: `trace_dependents` run
  forward from every source at once and carried across calls. A function's
  `may` set is its body's union without `ARG`. The program is iterated to
  a fixed point (the lattice is finite and only grows). A summary reaches
  its callers one round later, so a call chain costs a round per level; the
  walk runs until nothing changes (an earlier cap of 64 rounds left the top
  of an 80-deep chain looking constant, replayed stale), and the guard of
  4,096 rounds, if ever hit, marks everything `UNKNOWN`.
- An **every-call set per function**, a must-analysis: the terms of its body
  block in order, up to the first control term that can return from inside
  (a loop body may not run, a branch may not be taken). An `EFFECT` carries
  through a function called on every path (an effect makes every enclosing
  scope effectful, so no cutoff saves the caller). `TICKS`, `MODULE_VAR` and
  the input classes count only directly: a callee's failing record can still
  be re-executed to the same output and keep its caller valid.
- **Witnesses**: the first term that gave each bit, with the callee it came
  through and, for a native, why its row gives it. They are the lint's
  reasons.

**The native vocabulary** (`NativeEffects`, `core/src/native_fn.rs`;
[native-effects.md](native-effects.md#the-four-kinds-of-engine-native)). The
plan's four kinds of engine native are row flags with their replay rules:

| Kind | Row | Replay rule |
|---|---|---|
| declare (`body`, `point_light`) | `DECLARES` (`PB_FX_DECLARE` on an emitter) | as an emit: spliced back, or one segment; the host reaps what no visited scope repeats |
| emit (`draw_mesh`) | `EMITS` | spliced back, or one segment |
| command (`apply_impulse`, `sfx`) | `COMMAND` (`PB_FX_COMMAND`) | recorded with its arguments (`Dep::Command`) and **re-issued** on replay. Not `pure`, so never re-executed speculatively; inside a re-execution it is recorded and not issued, so the parent's replay or run issues it exactly once. Arguments a record cannot hold, or a non-nil result, make the scope effectful |
| query (`raycast`, `body_state`) | `query(PHYSICS)` (`PB_FX_READS_PHYSICS`) | `Dep::ClassRead`: valid while that class's revision holds. `Env::note_class_changed` / `pb_vm_note_class_changed` bumps it; each revisioned class has its own counter, so a physics step does not invalidate a `HOST_DATA` read. The frame gate treats a changed class as host data changed |

plus `ticks` (`PB_FX_TICKS`) for a read whose answer changes every run
(`time`, `frame_count` in petal-ui; not a fixed `dt`). The bridge gained
`pb_vm_register_emitter_fx`, `pb_vm_note_host_data_changed` (the host-data
bump C hosts had no way to make; Cheesecake's `body_state` is `Effect`
because of it), `pb_vm_note_class_changed` and `pb_vm_dep_classes_report`;
`Env::set_native_effects` replaces a registered row. The effect audit
accepts a command's effect and a revisioned read's host-read counter.

**What the runtime does with it** (`backend/bytecode/memo_hints.rs` writes
`BytecodeFn::memo_hint` and `Inst::Call::static_args` after lowering;
`push_closure_frame` and `memo_try` read them; details in
[memo-scopes.md](memo-scopes.md#the-mechanism), "Compile-time hints"):

- **Effect on every call**: never opens a scope (it would be given up at its
  close; its effect lands in the enclosing scope either way).
- **Ticks on every call**: opens no scope unless a recording scope encloses
  the call, which keeps the cutoff a parent can get through it (a `blink()`
  that reads the clock but answers the same bool most frames). This is the
  compile-time form of the cold-site guard for such scopes.
- **Static arguments** (top-level static data, or in a function a literal of
  literals and static captures): the replay check does not compare them.
  When the callee is also **constant** (its `may` set is only output, and
  it captures only static values, its one closure being made once per run:
  in top-level code outside any loop) the record
  holds for the life of the program: captures and dependencies are not
  checked either (when the record's own entries are only children and
  observations), so `draw_city(CITY)` is retained once and spliced every
  frame.
- `Env::set_memo_hints(stack, false)` ignores all three. A memoized run now
  lowers against the analysis even with the static-value cache off.

**The lint.** `never-replays` (`petal lint --rules-include never-replays`,
opt-in, report only; `core/src/lint/never_replays.rs`) and
`petal show-static --deps` name every function whose own record can never
validate, and why, down to the native: an effect on every call (through the
chain of calls on every path), a ticking read, a top-level `var` (a new cell
every run). A `PHYSICS` or `NET` query on every call is reported as *rarely*
replayed. A host gets the same with its natives:
`Env::dep_classes_report` / `Env::replay_findings`,
`pb_vm_dep_classes_report`.

```
never replays: bob: reads `now` on every call (class CLOCK), whose answer changes every run; its calls are not recorded unless a caller is
rarely replays: hit_test: reads `ray` on every call (class PHYSICS): it replays only on runs where the host did not revise PHYSICS
never replays: noisy: calls `shout` on every call, which never replays: shout calls `print` on every call (an effect a replay cannot reproduce)
```

**Measured** (`core/examples/dep_classes_frame.rs`, release, Apple M4, 300
frames, median µs per frame; the two runs' output is compared every frame).
A static city of 2,000 blocks drawn through `city → district → block`, 300
ships that read a ticking clock directly, and a HUD that reads the clock for
a spinner and draws 300 unchanging rows:

| Run | hints ignored | hints | |
|---|---:|---:|---|
| static-value cache on (the `fast` policy) | 219 | **175** | the HUD is not recorded, so its rows become scopes of their own and replay (hits 299 → 89,999 over the run); records 923 → 321 |
| static-value cache off (as under observation) | 1,156 | **1,078** | `CITY` is rebuilt every run; its call still replays by the static-argument rule, where comparing it by content exceeds the comparison budget |

The ships cost the same either way: the cold-site guard already stopped
recording them after three frames, which is what the hint does from the
first. Without a scope that would have been recorded and thrown away (the
HUD), or a static argument rebuilt every run, the hints change little: with
the cache on, a static argument is the same object every run and compares
in one step anyway.

**Checked by** `core/tests/dep_classes.rs`: a game-shaped script (a constant
city, ticking ships, declarations, commands, a `PHYSICS` query, a pure child
whose command depends on the pointer, a literal argument, a printing helper)
run frame by frame with memo off, memo on with hints and memo on without,
which must agree on every draw, declaration, printed line and issued command;
plus the classes and findings of the script, the every-call rules through
call chains, branches and early returns, a module `var` against a `state
var`, a query's revision against general host data, and the gate on a class
change. Turning off the deferral of commands in a re-execution fails two of
them (a command issued twice). The verification of B3 added three cases that
had replayed stale: a call chain deeper than the walk's old round cap, and a
lambda made in a top-level loop whose closure a later call picks by input
(both as a constant callee and as a capture passed as a static argument). The bridge has
`commands_reissue_queries_follow_their_class_and_declares_replay` in
`bridge_tests.cpp`; `lint_state_const.rs` pins the opt-in rules.

**Left for later:**

- **Guards for top-level code by class** (use 2 of P2, and the plan's
  "top-level games gain"): the root frame is still not a scope. The per-term
  masks are there to guard a block (`if frame_dirty & mask == 0 then reuse`);
  the guard and the dirty set per run are not built.
- **Hoisting the constant class** (use 1 of P2) beyond what the static-value
  cache keeps, and per-`state` bits (the mask has one `STATE` bit; the plan's
  "each `state`" would give a bit per declaration).
- **Static colliders' own revision**, so a `raycast` against level geometry
  only stays valid across physics steps.
- **The kinds in the type checker**: they are runtime rows; nothing checks
  that a `command` really returns nil or that a `declare` is keyed.
- **Cheesecake adopting it**: register the scene declarations as
  `PB_FX_DECLARE` emitters, `apply_impulse`/`sfx` as `PB_FX_COMMAND`,
  `raycast`/`body_state` as `PB_FX_READS_PHYSICS` with
  `pb_vm_note_class_changed` after each physics step, `time()` with
  `PB_FX_TICKS`, and run `pb_vm_dep_classes_report` over the games: a
  Cheesecake step, after it vendors this Petal.
- The static-argument rule inside functions accepts only literals and
  static captures; `FnPlan` (`static_cache/in_fn.rs`) knows more invariant
  terms and could be shared.

## L6: binding-time split of option records (as built, B4)

**The problem.** In `body(1000 + i, {shape: "box", size: 2.0, mass: 5.0,
kinematic: true, color: #ff8800, roughness: 0.4, metallic: 0.2, pos: p})`
seven of the eight fields are the same on every call, but nothing said so:
each record reached the host as eight values to decode (a string copied each
time), and a keyed declaration compared all eight with last frame's to find
that only `pos` moved. L4's plans and snapshots made both cheaper per field;
the compiler knows which fields cannot change and can take them out.

**The mechanism** (`core/src/record_template.rs`).

- **At lowering** (with the static-value cache, after it): a plain record
  literal (no class, no repeated key) with at least as many static fields as
  holes gets a `TemplatePlan` on its `Inst::AllocMap`: its holes and a site
  index. A field is static when its value is a constant, a top-level value
  the cache may keep (`StaticPlan::roots`), or a term in a function body that
  is invariant across calls (`FnPlan::invariant`, the expression-level
  classifier behind `static_in_functions`; `FnPlan` is now computed once in
  `static_cache::apply` and shared). Where observations are preserved (no
  `FnPlan`) only constants and top-level values count. `show-bytecode`
  prints `template N holes [...]` after the map.
- **At run time** each stack keeps, per site (beside its static slots, so
  stamped by the lowering and emptied with them, and rooted for the
  collector), the static fields of the first record the literal built and a
  `record::Template`: `id`, `holes`, `len`. The record is built exactly as
  before, from the same registers; if its static fields are the site's, it
  carries the template (`RecordMap::template`, an `Arc` beside the values).
  A constant field is not compared (it is the same by construction; a
  patched literal bumps the constant revision, which empties the sites); a
  classifier-static field is compared, the same heap object first and then
  by contents (`static_cache::identical`). One that differs starts the site
  over under a new id; a site that starts over more than 8 times, or holds
  a value with no contents to name (a closure, a cell, a handle, a pending
  value), stops tagging. The kept values are made immutable, as the cache's
  are, so what an id names cannot change under it.
- **The id** is a SipHash (fixed keys) of the keys, the hole positions and
  the static values' exact contents (kind and bits, so `1` is not `1.0`;
  strings by bytes; lists, records with keys and class, enums, dicts).
  The same literal with the same static values has the same id on every
  frame, after a reload that does not change them (including a recompile),
  in a fresh `Env` and in another process of the same build; editing one of
  them gives a new id. (A symbol is hashed by its id in the VM's table,
  stable within a process. The hash is std's `DefaultHasher`, which a Rust
  release may change, so ids are not to be kept across Petal builds.)
- **Invisible to scripts.** Equality, printing, field reads, spreads and
  copies ignore the tag, and every `&mut` method of `RecordMap` drops it,
  so a record that was written to (`set b.size = 3.0`, or `b.pos`) or
  rebuilt by a spread is untagged. `Env::set_record_templates(false)` (or
  `PETAL_RECORD_TEMPLATES=0`) builds every record untagged, the oracle.

**The bridge** (`integrations/petal-c-bridge`). A decoded `PB_MAP` carries
`pb_value::tmpl`, a `pb_template` (id, field count, hole positions, an
`is_hole` byte per field); `Value::record_template()` /
`template_id()` in C++. The view arena decodes a template's static fields
once into an arena of its own and copies those nodes into each record's
children (their text and nested children stay in the template's arena), so
only the holes are decoded. A VM's drains share one template cache
(`view::TemplateCache`), trimmed only when the views are cleared, so a
template is decoded once per VM rather than once per drain; a native's
argument arena keeps its own. `pb_snapshot_update_record(snap, record,
values, n, changed)` (`Snapshot::update(record, options)`) compares only the
options bound to holes when the record has the template the snapshot last
held (it finds the field an option is bound to by its position in the
record's children). `pb_vm_set_record_templates` switches tagging. The
`pb_value` struct grew by one pointer (80 → 88 bytes): a host must rebuild
against the new header (`pb_abi_check` catches a mismatch).

**Measured.** Release, Apple M4, load average 4 to 10 (other builds
running), median µs per frame over 300 frames of the default world (2,000
city meshes, 400 crates, 300 ships), worlds identical across every mode on
every frame.

Through the bridge (`examples/scene_frame.cpp`; `.untagged` is the same
host with tagging off in the VM; host includes the drain):

| Mode | Script | Drain | Host | Total |
|---|---:|---:|---:|---:|
| emit + plan, untagged | 222 to 237 | 387 to 404 | 552 to 576 | 775 to 819 |
| emit + plan | 224 to 231 | 320 to 335 | 487 to 505 | 716 to 736 |
| emit + snapshot (diff), untagged | 230 to 233 | 401 to 405 | 576 | 808 to 810 |
| **emit + templates** (`tmpl`) | 224 to 226 | 325 to 332 | **474 to 479** | **698 to 707** |

- Decoding: the ships' bodies and meshes and the crates are templated
  (the city's draws read a loop variable in every field, so they are not);
  the drain falls from about 400 to about 330 µs with no host change.
- Reading: `tmpl` (the snapshot compares only `pos`; a draw's static
  options are read once per template) takes the host from about 500 to 477.
  Callback modes (native arguments decoded per call) were too noisy under
  this load to separate (`callback.diff` 1,550 to 1,620, `callback.tmpl`
  1,470 to 1,620).

In the Rust mock (`core/examples/scene_frame`, VM segments), a host that
reads templates (`emit + memo + VM segments + templates`) goes from 41 to
37 µs: it reads values off the heap, so it never paid for decoding, and
only the field compares go. Script time with tagging on and off
(`TEMPLATES=off`): 192 to 198 µs against 197 to 198, the same; an earlier
version that compared every static field, constants included, cost about
5 µs (600 tagged records a frame).

Reproduce:

```sh
cd core
CARGO_TARGET_DIR=... cargo run --release --example scene_frame
TEMPLATES=off MODES="emit + memo + VM segments" cargo run --release --example scene_frame
cmake -S integrations/petal-c-bridge -B <build> -G Ninja -DCMAKE_BUILD_TYPE=Release -DPETAL_CARGO_TARGET_DIR=<target>
cmake --build <build> --target petal_bridge_scene_frame
MODES=emit.plan.untagged,emit.plan,emit.diff.untagged,emit.tmpl <build>/petal_bridge_scene_frame
```

**Checked by** `core/tests/record_templates.rs`: each script run frame by
frame with tagging on, off, with no static-value cache, with the cache
checking itself, and with memoization, which must print and emit the same;
and every pair of records with one id must agree on everything but their
holes. It covers which literals are tagged and their holes, writes and
spreads dropping the tag, ids across reloads (text moved, another literal
changed, a patched `config let`) and a fresh `Env`, `1` against `1.0` and
nested values, a closure in a static field, and tags through memo replays.
`record_template::tests` holds a site to its rules (a static field that
changes starts it over under another id and back again; a site that keeps
changing gives up; kept values become immutable; off tags nothing). The
static-value fuzzers (`static_fuzz`, `static_fn_fuzz`) run with tagging on.
The bridge has `plan::tests::a_templated_record_decodes_as_an_untagged_one_from_one_decode_of_its_template`
(the views equal, the static text shared), `a_snapshot_compares_only_the_holes_of_the_template_it_holds`,
and `a_templated_record_carries_its_template_and_a_snapshot_compares_its_holes`
in `bridge_tests.cpp` (on against off through a VM).

**Left for later:**

- **The record is still allocated whole** each evaluation: the VM copies
  the static values from registers into a fresh vector. Sharing the
  template's values copy-on-write (or a record that holds its template and
  only its holes) would stop the allocation the plan mentions; it touches
  every reader of `RecordMap`.
- **Calls do not carry `(template, holes)` separately**: the host receives
  the record and its tag. For an emitter that is the same information; a
  native ABI that passed only holes would need the host to hold the
  template's decoded values first.
- **Spreads and writes drop the tag.** `{...base, pos: p}` with a static
  `base`, or a write to a hole, could keep or derive a template.
- Class instances and literals with fewer static fields than holes are not
  templated. Inside a top-level loop body only constants and top-level
  values count as static (`FnPlan` covers function bodies).
- **Cheesecake adopting it**: once it vendors this bridge, its body entry
  can use `Snapshot::update(opts, o)` and its draw path can keep what it
  parsed per `template_id()`. The `pb_value` size change needs its rebuild.
- The id is 64 bits; a collision could mislead a host that caches by id.

## L8: implicit keys and keyed loops (as built, B5)

**The problem.** A declaration needs a key the host reconciles it by, and a
script had to make one up: `body("crate{i}", {...})` formats a string per
call, the host hashes it, and two helpers that happen to format the same
string collide. The runtime already has the right identity for "this
declaration, at this place in the program": the call path that keys `state`
and memo scopes, `[Call(site) | Index(i) | Key(h)]`
([state-call-paths.md](state-call-paths.md)).

**The key** (`frame::call_key_of`, `core/src/backend/bytecode/vm/frame.rs`).
A native call's implicit key is `scope_id(path ++ [Call(site)])`: the live
frame stack's full path (the same walk `state` and memo use) with the
native's own call site appended, hashed by the SipHash with fixed keys that
names memo scopes and segments. So it is the id a memo scope would have if
the native were a user function called there, and it inherits everything
the path guarantees:

- the same on every frame and every run while the call path is; call-site
  ids are derived from callee text and ordinals, so an edit elsewhere and a
  reload keep it (adding an earlier call of the same callee in the same
  function moves it, exactly as for a `state` slot);
- one per loop iteration (`for` and `while`), one per recursion depth, one
  per call site of a helper;
- under `for e in ents key e.id` it follows the entity, because the loop's
  part is `Key(hash(e.id))`, not the index;
- unique within a run wherever a `state` slot is.

A call made in a callback of a higher-order builtin (`map`, `forEach`,
`filter`, … while `Stack::callback` is set, nested calls included) has
**no** key: every element runs on the same path, which is why `state` is an
error there. A call with no call term (a probe re-evaluated by memo
validation) has none either.

**The API.** `PetalCxt::call_key() -> Option<u64>`, computed on request from
the borrowed frame stack (`CallKeySource::Frames`), so a native that never
asks pays nothing; no effect-row flag is involved. The bridge exposes it
twice (`integrations/petal-c-bridge`): `pb_call_key(call, &key)` for a
callback native (`petal::Call::key()`), and `PB_FX_KEYED` on
`pb_vm_register_emitter_fx`, which makes the emitter push `tag(key,
args...)` with the key as an int (nil where there is none). `PB_FX_KEYED` is
bridge-only and does not reach the effect row. A host that already gets an
explicit id keeps using it; the key is there for calls without one.

**Memoization.** A record is keyed by its scope's path, so a replay at that
path hands back emitted values whose keys are the ones a run would compute;
segments carry them the same way. Two natives are re-called outside their
call path, and both are handled:

- a **command** re-issued by a replay runs on the parent's path with no call
  term. `Dep::Command` now holds the key the call read and the replay sets
  `MemoTable::reissue_key` around the re-issue, so the native gets
  `CallKeySource::Fixed`. A command deferred by a re-execution (it is not
  called then) stores the key computed at the child's own path.
- a **probe** is re-evaluated by validation with no call term, so a probe
  whose answer read its key cannot be checked: the scope is given up as
  effectful (`memo_note_native`, the probe branch).

**Keyed loops: scopes follow their entity.** This needed no new mechanism:
memo records, segment ids and now declaration keys are all keyed by the path,
and a keyed loop puts `Key(h)` where the index was. When the entities of
`for e in ents key e.id do ent(e) end` change order, every `ent` scope is at
the same path as last frame with the same argument and replays; a segmented
host is handed no new segment. In a positional loop the same reorder makes
every moved entity's scope miss (its position now holds another argument)
and re-record, and its declarations change keys.

**Measured** (`core/examples/implicit_keys_frame.rs`, release, Apple M4,
200 frames, median µs per frame; host = drain plus a hash-map upsert per
declaration):

MEASUREMENTS

**Checked by** `core/tests/implicit_keys.rs`. Each script runs frame by frame
with memoization off, on with a flat buffer and on with the buffer segmented
(drained through a `SegmentCache`), and all three must declare the same
values under the same keys and issue the same commands with the same keys:
call sites, loop iterations, recursion, `while`, top level, callbacks of
`forEach`/`map` (none) and calls nested in them; a keyed loop whose entities
reorder (keys and segments follow the entity; positional keys stay with the
position; every `ent` replays); commands re-issued by replays and deferred by
re-executions; a probe that reads its key (never replayed); and reloads that
keep the call path (same keys) or add an earlier call of the same callee
(that one key moves). Mutations each test was seen to catch: re-issuing a
command without its recorded key, deferring one without computing its key,
and letting a probe that read its key be recorded. The bridge has
`implicit_keys_reach_keyed_emitters_and_callbacks` in `bridge_tests.cpp`
(keyed emitter payloads, a callback's `Call::key()`, a command's key across
a replay).

**Left for later:**

- **Persistent collections and incremental keyed loops (P3 of
  [reactive-rendering-plan.md](reactive-rendering-plan.md)).** Not built: a
  keyed loop over `n` unchanged entities still visits all `n` and validates
  one memo record each (the second table above). Making it O(changes) needs
  persistent lists and maps that diff by structural sharing (RRB vectors,
  HAMTs) and a keyed `for` compiled to a map over that diff: bodies run
  only for inserted or changed keys, removed keys drop their segments and
  state, moves reorder segments. That is weeks of work in the value layer
  (every list and map reader), estimated at 6 to 8 in the reactive plan.
  An intermediate step that needs no new collection: a keyed loop whose
  body is one call of a constant-class helper could compare its list with
  last frame's by identity (`Value` ids are generational) and splice the
  whole loop's segment when it is the same object.
- **Keys in a higher-order builtin's callback.** None is given; `map` and
  `forEach` could push an `Index` part per element (they visit elements in
  order), but `filter` and `sort` call back a data-dependent number of
  times, which is why per-element keying was rejected for `state`
  ([state-call-paths.md](state-call-paths.md) §5).
- **A probe that reads its key** gives its scope up. Recording the key in
  `Dep::Probe` (as for commands) and re-evaluating with it would keep such
  scopes, if a query keyed by its caller (`body_state()` without an id)
  turns out to be wanted.
- **No duplicate check.** Unlike a keyed loop's duplicate keys, two calls
  with one implicit key in one run are not detected by the VM (they need a
  host that calls one Petal function twice per run); a host that reconciles
  by key sees the second as a duplicate.
- **Cheesecake adopting it** (a Cheesecake step, after vendoring this
  Petal): register `body`, `point_light` and the other declarations with
  `PB_FX_KEYED` (or read `Call::key()` in the callback form) and key
  declarations without an id by it, so a game can write
  `body({shape: "box", ...})`; ids stay for what the script must name
  (`body_state(id)`, joints).
- The key is 64 bits; a collision would make a host merge two declarations.
