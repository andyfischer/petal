# Reuse the native-call argument buffer

Status: **proposed**, 2026-09-15. Measured but not started.

## The observation

In a symbolized profile of a tight `slice()` loop (5M iterations,
`cargo build --profile profiling` + `sample`), the malloc/free traffic visible
under the native-call path is **argument-vector growth**, not string or list
data: `RawVec::grow_one` and `RawVec::finish_grow` appear alongside
`_xzm_xzone_malloc` / `_xzm_free`, under `call_native_fn` / `do_builtin_call`
(`rust/src/backend/bytecode/vm/native.rs`).

Every builtin call pays this, so it is spread across every script rather than
concentrated in one app. The spreadsheet's formula commit issues ~441k native
calls in a single frame, which is what made it visible.

## The change

Hold a reusable buffer on the VM (or the stack) for a native call's arguments
and clear it per call instead of allocating a fresh `Vec`. A small-vector
inline array is the other option, but a single reused buffer is simpler and
enough — native calls do not nest deeply, and where they do (an intrinsic
calling back into the VM) the nesting depth is bounded and can take a second
buffer or a stack of them.

Check the same for the `chain` / provenance vector built beside the arguments
on the same path.

## How to check it

`PETAL_OPT_STATS=1` does not cover this, so measure directly: a builtin-heavy
microbenchmark (a `slice` or `len` loop), plus

```bash
./ts/bin/bench-opts.ts
cd petal-ui && cargo run --release --example bench_panel -- \
  ../examples/productivity/spreadsheet/app.ptl 600 \
  --scenario ../examples/productivity/spreadsheet/bench/edit.json --no-gate
```

Take the minimum of several runs. The corpus goldens and the differential
oracles must not change — this is a pure allocation change.
