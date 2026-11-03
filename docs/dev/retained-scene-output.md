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
  L3's `same_value`).
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
disk is fine):

```sh
cmake -S integrations/petal-c-bridge -B <build> -G Ninja -DPETAL_CARGO_TARGET_DIR=<target>
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
