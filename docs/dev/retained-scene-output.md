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
  more. The channel remembers what it defined by weak reference, so "dropped"
  is exactly "this body died".
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
records and the undrained marks; a host may keep the `Value`s of a
definition until its serial is reported dropped.

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
themselves, since the others do not move declared motion).

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
re-recordings, effectful scopes, cutoffs and a reload; plus the identity,
drop and reset rules) and
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
