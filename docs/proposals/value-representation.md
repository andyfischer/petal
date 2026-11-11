# Proposal: a 16-byte `Value` and leaner heap objects

Status: proposed, 2026-11-04

## Summary

`crate::value::Value` is a 24-byte `Copy` enum (`core/src/value.rs:81`).
Only three variants need more than 8 payload bytes: `Vec2(f64, f64)`,
`Dual { value, derivative }` and `EnumVariant { tag: StringId, data: ListId }`.
Moving `Vec2` and `Dual` into heap slabs (as `Vec3` already is) and making the
enum tag a `u32` index makes `Value` **16 bytes**. That is a third off every
list element, record field, `map` entry, register, closure capture, trace
event and memo record.

Measured, though, the width of `Value` is **not** what drives the VM's memory
use. Per-object overhead around it is: a short string costs 150–210 bytes, a
two-element list about 100, a four-field record about 175, and only 8 bytes of
each of those comes from `Value` being 24 rather than 16. The largest blow-up
found is algorithmic: a common accumulator pattern silently copies the whole
list on every iteration.

**Recommendation, in priority order:**

1. **Fix now (done, 2026-11-04: private cells in `backend/bytecode/cells.rs`):** `set xs = append(xs, f(x))` is O(n²) in time and garbage
   whenever `f` is a non-trivial builtin or a user call. 200k appends of
   `str(i)` took **142 s**; the same loop with the argument bound by `let`
   first takes 0.2 s. This is a separate change; see
   [the in-place window](#1-the-in-place-window-is-too-narrow).
2. **Adopt (done, 2026-11-04: `Text` and a content-keyed intern table in `heap.rs`):** slim string storage. Stop duplicating each string's text in the
   intern table, and store short strings inline. That is roughly −60% per
   short string.
3. **Adopt (done, 2026-11-05):** the 16-byte `Value` (box `Vec2`/`Dual`, `u32` enum tag).
4. **Adopt (small):** shrink slab slot metadata from 12 to 4 bytes per object.
5. **Reject:** NaN-boxing or pointer-tagging to 8 bytes. It needs a language
   change to `int` and a 64→48-bit id squeeze, and it gives up exhaustive
   `match` over ~3,500 `Value::` sites, all for a saving that items 2–4 mostly
   capture.

No item is visible to Petal programs except the two edge cases noted under
[Design](#design) (truthiness of a zero `vec2`/dual, and Rust-side `==`),
which the implementation must preserve.

## Motivation

### Where the bytes go today

Sizes from a probe compiled into `core` (`size_of`, x86-64):

| Type | Bytes | Notes |
|---|---|---|
| `Value` | 24 | 1-byte tag + 16-byte payload, align 8 |
| `RawId` (every heap id) | 8 | `u32` index + `u32` generation |
| `Slot<Vec<Value>>` (a list) | 40 | 24 `Vec` header + 12 metadata, padded |
| `Slot<StrObj>` (a string) | 48 | 24 `String` + 8 `chars` + 12 metadata |
| `Slot<MapObj>` (a record) | 72 | `RecordMap` 40 + class + metadata |
| `Slot<[f64; 3]>` (a vec3) | 40 | |
| `Slot<CellSlot>` (a `var`) | 96 | |
| `(Value, Value)` (a `map` entry) | 48 | |
| `TraceEvent` | 152 | `SmallVec<[Value; 4]>` + result |

Slot metadata (`core/src/heap.rs:259-285`) is `generation: u32`,
`high_water: u32` and four `bool`s. `high_water` differs from `generation`
only after `Slab::inherit_generations`, and `permanent` already has a bit
index on the slab.

### Measured per-object cost

Release build, peak RSS minus the 12.5 MB of `print(1)`. Each probe appends N
values to a `var` list in a loop, with the value bound by `let` first so that
the append runs in place. Probes are in the session scratchpad, not checked in.

| Probe | N | RSS over baseline | Bytes / element | Of which `Value` width |
|---|---|---|---|---|
| `Int` | 1,000,000 | 18.5 MB | ~19–24 | 24 (all of it) |
| `vec2(i, 1)` (inline) | 1,000,000 | 18.6 MB | ~24 | 24 |
| `vec3(i, 1, 2)` (heap) | 1,000,000 | 57 MB | ~57 | 24 |
| `[i, i + 1]` | 200,000 | 19.6 MB | ~98 | 72 |
| `{x, y, vx, vy}` | 200,000 | 35 MB | ~175 | 120 |
| `Shape.Circle(i)` | 200,000 | 13.7 MB | ~68 | 48 |
| `str(i)` (≤ 7 chars) | 200,000 | 29.5 MB | ~148 | 24 |
| `str(i)` (≤ 7 chars) | 1,000,000 | 213 MB | ~213 | 24 |

The string row is where memory goes. A seven-character string costs:

- 24 B for the `Value` in the list,
- a 48 B slot,
- a ~32 B `malloc` for the text,
- the intern table entry: `HashMap<String, StringId>` (`heap.rs:737`), which
  is a second `String` (24 B header + another ~32 B `malloc` of the same
  text) plus the id and hash-table slack. `insert_interned` (`heap.rs:1250`)
  `clone()`s the text into the table. **Every** string up to
  `INTERN_MAX_LEN` (512 bytes) is interned, not just literals.

The growth from 148 to 213 B/string between 200k and 1M is the intern
table's doubling resize.

### The quadratic accumulator

```petal
var xs = []
for i in range(0, 200000) do
  set xs = append(xs, str(i))
end
```

This takes **142 s and 136 MB** in a release build. `petal run --profile` at
N = 20,000 shows `append` at 35 µs/call (ints: 87 ns) and 1,632 collections.
`petal show-bytecode` shows the reason:

```
     6  r171 = cell_read r163
     7  r173 = builtin "str" [r170]
     8  r174 = builtin "append" [r171, r173]    // copying append
     9  r175 = cell_write r163 <- r174
```

Writing `let s = str(i)` first gives `cell_take` / `builtin_in_place "append"`
/ `cell_put`, at 0.02 s for N = 20,000. This is documented as a known gap in
`core/src/backend/bytecode/cells.rs:100-104` ("Bind the argument first"). The
same happens for `sqrt(i)` and any user function call. The pattern is the
first thing a JS/Python programmer writes, and each copy is a full list of
garbage, which is what drives the collections.

### How often the boxed variants occur

- `vec2(` appears 41 times in `.ptl` files in the repo, in 7 files
  (`core/packages/vec2/vec2.ptl`, `core-runtime/glsl/src/glsl.ptl`,
  `examples/games/solar-system/app.ptl`, `examples/console/vec2_demo.ptl`,
  tests). On the Rust side `Value::Vec2` appears 70 times in 19 files,
  including `core-libs/petal-ui/src/draw.rs` and `integrations/petal-c-bridge`.
- Autodiff (`deriv`, `dual(`) appeared in two files in `examples/`
  (`console/differentiation.ptl` and the `d2-dial` sensitivity option; an
  earlier count of 31 was stale). `Value::Dual` appeared 59 times in 24
  files. (Dual numbers were removed on 2026-11-11,
  docs/tasks/differentiation-rewrite.md.)
- Neither is handled in the VM's fast loop (`vm/fast.rs`) beyond one test.

## Design

### 16-byte `Value`

```rust
pub enum Value {
    Nil, Bool(bool), Int(i64), Float(f64),
    String(StringId), List(ListId), F64Array(F64ArrayId), Map(MapId),
    Dict(DictId), Closure(ClosureId), OverloadSet(OverloadSetId),
    NativeFunction(NativeFnId), Element(ElementId), Cell(CellId),
    Vec3(Vec3Id), Symbol(SymbolId), Handle(HandleVal), Pending(PendingId),
    Vec2(Vec2Id),                         // was Vec2(f64, f64)
    Dual(DualId),                         // was Dual { value, derivative }
    EnumVariant { tag: EnumTag, data: ListId },  // tag was StringId (8 B)
}
const _: () = assert!(std::mem::size_of::<Value>() == 16);
```

I checked the layout with `rustc`: a `u8` discriminant plus a 12-byte,
4-aligned payload (`EnumTag(u32)` + `ListId`, or `HandleVal`) packs into 16,
and `Option<Value>` stays 16.

- **`Vec2Id` / `DualId`** are `Slab<[f64; 2]>` ids, exactly as
  [vec3.md](../dev/vec3.md) describes for `Vec3`: immutable, a free-list pop
  per allocation, leaves for the GC.
- **`EnumTag(u32)`** indexes a heap-side tag table (`Vec<StringId>` plus a
  reverse map) whose strings are marked permanent. Tags come from program
  text and from `json_to_value`, so the table is bounded by distinct variant
  names. Payload-free variants still allocate nothing, because they use
  `empty_payload`.

**Behaviour that must not change:**

- **Truthiness.** A zero `vec2` and a dual whose value is 0 are falsy today
  (`value.rs:88-91`), and `is_truthy` has no heap. Keep `is_truthy` for the
  heap-free cases. Have the VM's `JumpIfFalse`/`JumpIfTrue` (`vm/fast.rs:227-239`)
  and the 20 other callers go through a `truthy(&Heap)` fallback for
  `Vec2`/`Dual`. The fast path pays nothing for scalars. Do **not** copy
  vec3's "always truthy" rule: that would be a visible change.
- **Equality.** `Value` derives `PartialEq`, so Rust-side `a == b` on two
  `Vec2`s compares components today and would compare ids afterwards. Script
  `==` already goes through `values_equal` (`value.rs:897`), which reads
  through the heap. Rust callers need an audit: `memo::content_equal`,
  `static_cache.rs:1274`, `hash_value`, `run_deps::hash_content` and the trace
  filters. This is the main correctness risk. A temporary
  `#[deprecated] impl PartialEq` shim, or removing the derive, will surface
  every site at compile time.
- **Embedding.** The C bridge (`integrations/petal-c-bridge/src/{vm,view,builder}.rs`)
  and `petal-ui/src/draw.rs` read `Value::Vec2(x, y)`. They switch to
  `heap.get_vec2(id)`, the way they already handle `Vec3`. The C ABI
  (`pb_vm_set_vec2(double, double)`) does not change.

### Strings

1. **The intern table stops owning text.** Make it a
   `hashbrown::HashTable<StringId>` hashed by content and compared through
   the slab (`strings.get(id).text == probe`). This removes one `String` and
   one `malloc` per string. The sweep already removes entries
   (`heap.rs:2532-2534`).
2. **Inline short text.** `StrObj { text: String, chars: usize }` becomes a
   small-string type that holds up to ~22 bytes inline and is the same 24
   bytes otherwise (`compact_str`, or a hand-rolled enum), plus
   `chars: u32`. Most runtime strings (`str(i)`, keys, labels, tags) never
   `malloc`.
3. Optionally, intern only below a smaller length (say 64 bytes). Interning
   is an optimisation, not a contract: `values_equal` compares content when
   ids differ (`heap.rs:661-663`).

Estimated short-string cost after (1)+(2) with a 16-byte `Value`: 16 + ~40
slot + ~12 table = **~70 B, down from ~150–210**. This is an estimate, not a
measurement.

### Slot metadata

`generation: u32` stays in the slot. `alive`, `gc_mark` and `immutable` move
to per-slab bitsets beside the existing `permanent` bits. `high_water` moves
to a side map that is populated only by `inherit_generations`. That saves 8 B
per heap object: a list slot goes from 40 to 32 B and a record slot from 72
to 64 B. The bitset layout also makes the sweep's "skip dead and permanent"
scan word-at-a-time.

## Before and after

These are runtime representations; Petal source does not change. The "after"
figures are projections from the layout arithmetic above, not measurements
of a prototype.

| Workload | Today | After 2+3+4 | Change |
|---|---|---|---|
| 1M-int list | ~24 MB | ~16 MB | −33% |
| 200k `{x, y, vx, vy}` records | ~35 MB | ~27 MB | −23% |
| 200k `[i, i+1]` pairs | ~19.6 MB | ~14 MB | −28% |
| 1M short strings | ~213 MB | ~70 MB | −67% |
| 1M `vec2`s in a list | ~24 MB | ~16 + 24 = 40 MB | **+67% (worse)** |
| 200k-event trace ring (`DEFAULT_CAPACITY`) | ~30 MB | ~21 MB | −30% |
| `append(xs, str(i))`, 200k | 142 s, 136 MB | 0.2 s, ~25 MB (item 1) | |

**The case where the old form is better:** a program holding large arrays of
`vec2` (particles, polylines) gets bigger, and every `vec2` operation
allocates. The vec3 measurement (`docs/dev/vec3.md`, "Cost") found
slab-allocated vec3 arithmetic in the same 0.5–0.8 s range as inline vec2 on
a 2M-iteration loop. I reproduced that on this machine with 1M appends:
vec2 0.22–0.33 s, vec3 0.33–0.39 s. The bulk-data answer is already
`F64Array`. Given 41 `vec2(` sites in the corpus against every program's
lists and records, I judge this trade worth making. It is the main point a
reviewer could reasonably reject.

**Interaction with hot reload, `state` and the frame gate:** none for the
layout itself. `state` is persisted via JSON (`{"type": "vec2", ...}`), which
does not change. The frame gate and memo fingerprint contents through the
heap already for `Vec3`. `Heap::fork` and `inherit_generations` gain two
slabs, as they did for vec3.

## Assessment

**Project goals.** This is neutral to positive. It does not touch the
language surface. Lower memory helps live coding, where Garden keeps forked
heaps for speculative execution (`Heap::fork` deep-clones every slab,
`heap.rs:1110`), so per-object savings are multiplied there.

**Ease of writing.** No effect from items 2–5. Item 1 removes a performance
cliff that users currently have to learn ("bind the argument first"), and
that the lint advice in `core/src/env/mod.rs:1112` exists to explain.

**Bug-freedom.** Risks:

- the Rust-side `PartialEq` change for `Vec2`/`Dual`, mitigated by
  compile-time surfacing;
- truthiness, mitigated by keeping the zero-vector-is-falsy tests at
  `value.rs:961+`;
- stale `Vec2Id`s under the weak-holder rule. This is the same discipline as
  every other heap id; the trace buffer's `display_weak` must learn the new
  kinds.

The differential oracle, golden corpus, small-scope check and the
`values_equal` symmetry property test (`value.rs:1100+`) all still apply
unchanged and are the regression net.

**Performance.**

- Fewer bytes moved for every `Move`, which is ~20% of instructions on
  `spreadsheet.ptl` (`performance.md`, "Where the remaining headroom is").
- Fewer bytes for every register-file resize, list clone and memo record.
- The cost lands on `vec2` and dual operations: one slab allocation each,
  plus GC charge.
- Benchmarks: `tools/bench-opts.ts` (`test/benchmarks/particles.ptl`,
  `spreadsheet.ptl`, `life.ptl`), `bench_panel` on the panel corpus, and a
  new vec2/dual micro-benchmark under `test/benchmarks/micro/`. Item 1
  should get a regression test in the style of
  `test/vitest/recursion-memory.test.ts`.

**Live coding, introspection, program manipulation.** Provenance, `explain`
and direct manipulation work on terms and on values read through the heap, so
they are unaffected. Autodiff (`Dual`) is affected only in where the pair
lives. `dm_v2`'s Jacobians are built from `as_f64`/`derivative`, which become
heap reads.

**Maintainability.** About 130 `Vec2`/`Dual` match sites in roughly 40 files
move from destructuring to `heap.get_vec2(id)`, plus about 60 `EnumVariant`
sites whose `tag` becomes `heap.enum_tag_str(tag)`. It is mechanical but
broad. It removes a special case: `Vec2` and `Vec3` now use the same
representation. The new invariant, enforced by the size assertion, is that
`Value` stays 16 bytes and nothing inline exceeds 12 payload bytes beside the
tag.

**Domains.**

- Games and creative coding: worse for `vec2`-heavy data, better for
  everything else. The C bridge API is unchanged.
- Productivity, UI and dashboards (lists, records, strings): clearly better.
- Embedding hosts: the Rust embedders that destructure `Value::Vec2`
  (`petal-ui`, the C bridge) need small edits.

**Compatibility.** Nothing breaks for Petal programs or the C ABI. Rust
embedders matching `Value::Vec2(x, y)` or `EnumVariant { tag, .. }` get a
compile error that points at the fix.

**Prior art.**

- Lua 5.x's `TValue` is exactly this design: a 16-byte value-plus-tag, with
  every non-scalar on the heap.
- CPython boxes everything behind a pointer and caches small ints.
- V8 uses 31-bit Smis plus 32-bit compressed pointers.
- LuaJIT, SpiderMonkey and JavaScriptCore NaN-box into 8 bytes. That only
  works because their numbers are all doubles (or 32-bit ints) and their
  references are raw 47/48-bit pointers. Petal has full `i64` ints and
  64-bit generational ids, so neither precondition holds.

## Alternatives considered

- **Do nothing.** 24 bytes is defensible, and `vec3.md` already rejected
  going to 32. But the per-object overheads (strings, slot metadata) and the
  quadratic append are worth fixing regardless, and they are independent of
  the `Value` change.
- **8-byte NaN-boxed or tagged `Value`.** Rejected:
  - `int` would have to become 48- or 63-bit with boxed overflow. That is a
    language change, and it touches `i64::MIN`/`MAX` edge cases that the
    Kani proofs pin (`docs/dev/formal-verification.md`).
  - Ids would have to shrink to fit the 48-bit payload, for example a 24-bit
    index plus a 24-bit generation, which caps live objects at 16M per kind.
  - Every `match` on `Value` would become unsafe bit manipulation.
  - It would save 8 more bytes per slot, while the measured cost is
    dominated by the objects behind the slots.
- **`Vec2` as inline `(f32, f32)`.** Rejected for the same reasons as `vec3.md`
  gives against f32 `Vec3`: `vec2(0.1, 0).x == 0.1` would be false.
- **Inline small strings inside `Value` itself** (a 16-byte value has room
  for up to 14 bytes of text). It is attractive (no slot at all), but every
  `Value::String(id)` site (and its use as a map key, intern id and GC
  object) would gain a second string form. Revisit after item 2 if strings
  still dominate.
- **Smaller wins inside the 24-byte layout.** Items 1, 2 and 4 are
  independent of item 3 and can ship first. That is the smallest version
  that delivers most of the benefit.

## Migration and compatibility

No Petal source changes. Rust embedders: mechanical edits at
`Value::Vec2`/`Dual`/`EnumVariant` destructuring sites (compile errors guide
them). JSON and `state` formats are unchanged. `docs/dev/Architecture.md`
("Value") and `docs/dev/vec3.md` ("The constraint") must be updated to say
16 bytes.

## Implementation sketch

Each step is a separate commit with its own measurement:

1. **The in-place window** (`backend/bytecode/cells.rs`). A cell that no
   closure captures cannot be reached by any callee (the containment
   invariant, `docs/syntax/var.md`). So for such cells, any call may sit in
   the window, as long as nothing inside the window reads the cell. Errors
   are safe: on an owned cell, `cell_take` returns the same id and nothing
   has been mutated until the in-place builtin runs. Tests: the existing
   `cell_fuzz.rs` differential fuzzer, plus a new time/memory-ceiling test.
2. **Strings:** a content-keyed `HashTable<StringId>` intern table, then
   inline short text and `chars: u32`. Measure with the `str(i)` probe and
   `bench_panel`.
3. **Slot metadata to bitsets** (`Slab`). Measure sweep time with
   `tools/bench-gc-pause.ts`.
4. **`Value` to 16 bytes:**
   1. Add the `vec2s`/`duals` slabs and the enum tag table.
   2. Remove `derive(PartialEq)` temporarily to find every Rust `==`.
   3. Port the match sites.
   4. Flip the size assertion to 16.
   5. Run the full suite, the differential oracle, the small-scope check,
      and `tools/bench-opts.ts` before and after.

## Open questions

- Is the `vec2` regression acceptable? The project owner should weigh this
  against any `vec2`-heavy host workloads outside this repo (Cheesecake's
  games are cited in `performance.md` and are not in the corpus).
- `Heap::fork` deep-clones every list and string. For direct manipulation's
  speculative runs, structural sharing (`Arc` payloads, or copy-on-write
  slab pages) may matter more than anything here. It is unmeasured; it
  deserves its own measurement with `cargo run --release --example
  rerun_timing` / the DM v2 study apps.
- The 12.5 MB RSS of `print(1)` was not broken down (binary, stdlib
  lowering, static caches).
