# `vec3`: why it lives on the heap

`vec3(x, y, z)` is a native 3D vector parallel to `vec2`: the same operators
(`+ - * /`, unary `-`, `==`), fields `.x .y .z`, and the shared vector builtins
(`mag`, `distance`, `normalize`, `dot`, `limit`, `lerp`) plus `cross`. The
script-facing contract is in [Builtins.md](../Builtins.md#vectors-2d-and-3d)
and [language-guide.md](../language-guide.md#vectors). This note records the
one design decision that is not visible from there: the representation.

## The constraint

`Value` is `Copy` and 24 bytes — a tag plus a 16-byte payload — and
`rust/src/value.rs` asserts it stays that way: every register, list slot,
record entry and closure capture is a `Value`, so widening it costs memory
bandwidth everywhere, for every program, whether it uses vectors or not.
`Vec2(f64, f64)` fills the 16 bytes exactly. Three `f64`s (24 bytes) do not
fit.

## Options considered

| Option | Precision | Cost per op | Other |
|---|---|---|---|
| Widen `Value` to 32 bytes | f64 | none for vec3 | +33% on *every* value, every program. Rejected. |
| Inline `Vec3(f32, f32, f32)` (12 bytes) | f32 | none | `vec3(0.1, 0, 0).x == 0.1` is **false**; printing shows `0.10000000149011612`; positions accumulated over a long run drift ~1e-7 relative. Petal numbers are f64 everywhere else, and the C bridge API (`pb_vm_set_vec3(double, double, double)`) is f64 too. |
| Reuse the `F64Array` slab (`Vec<f64>` per vector) | f64 | a `malloc` per result | Cheap to write, but every `a + b` would heap-allocate a `Vec`. |
| **Dedicated `Slab<[f64; 3]>` + `Value::Vec3(Vec3Id)`** | f64 | a free-list pop, no `malloc` | Chosen. |

## The chosen representation

- `Heap` has a `vec3s: Slab<[f64; 3]>` next to the other slabs, and
  `Value::Vec3(Vec3Id)` carries a generational id into it (8 bytes, so
  `Value` stays 24). The payload sits in the slot itself, so after warm-up an
  allocation is a free-list pop.
- **Immutable.** Like every heap object except cells, a vec3 is never written
  after allocation; every operator and builtin allocates a fresh one. So the id
  is a value in every observable sense: sharing it is safe, `limit(v, big)`
  can hand back `v` itself, and a fork (`Heap::fork`) or snapshot
  (`inherit_generations`) needs nothing special.
- **GC.** A vec3 is a leaf: `mark_referenced` marks the slot, `sweep` frees it,
  and each allocation charges `24 + SLOT_TRACE_COST` to the collector budget
  like any other object. `AllocKind::Vec3` counts them in the alloc stats.
- **Equality / hashing** go by components: `values_equal`, `memo::content_equal`
  (bitwise, like `Vec2`), `hash_value` and `run_deps::hash_content` all read
  through the heap. Two separately built `vec3(1, 2, 3)`s are `==`.
- **Truthiness** is the one visible difference from `vec2`: `Value::is_truthy`
  has no heap, so it cannot see the components, and a `vec3` is always truthy
  (like a list or record), whereas a zero `vec2` is falsy.
- **JSON / state.** `value_to_json` writes `{"type": "vec3", "x", "y", "z"}`
  (the same tagged shape `vec2` already used), and `json_to_value` now reads
  that exact shape — the tag plus exactly the component fields, all numbers —
  back as a vector, for both `vec2` and `vec3`. That is what makes
  `json_parse(json_stringify(v))` and `get_state_json` / `set_state_from_json`
  round-trip. An object with any extra field stays a record.
- **Types.** `Type::Vec3` (`vec3` in annotations). The checker's builtin
  table types `vec3`/`cross` as `vec3`, and `normalize`/`limit`/`lerp` follow
  their argument's kind.

## Cost

Measured on a 2M-iteration `p = p + vec3(0.5, 0.25, 1) * 0.001` loop (release
build, noisy shared machine): the heap vec3 ran in the same range as the inline
vec2 equivalent (0.5–0.8 s for both), and roughly 6–8× faster than the
`{x, y, z}` record + `add3`/`scale3` helper functions that scripts wrote before
the type existed (~4.3 s). Interpreter dispatch, not the slab allocation,
dominates an interpreted binop at this scale.

## For embedders

`Heap::vec3_value(x, y, z) -> Value` allocates one, `Heap::get_vec3(id) ->
[f64; 3]` reads one. A native receives `Value::Vec3(id)` and reads it through
`state.heap()`.
