# Native effect rows and the audits

Every native is registered with a row that says what it does besides
computing from its arguments. The reactive layers believe the row and never
look at what a call actually did, so a row that says too little makes a
memoized scope or a skipped frame stale without a word. This page is how the
row is defined, who reads it, and the three tools that check rows against
behaviour. What an embedder should declare, with the Cheesecake case, is in
[usage/ffi.md](../usage/ffi.md#declaring-native-effects-honestly).

## The row

`NativeEffects` in `core/src/native_fn.rs`, passed to every registration
(`Env::register_native(name, f, effects)`, `register_native_boxed`, the core
table's `register`), so there is no undeclared native:

| Field | Meaning |
|---|---|
| `reads: InputClasses` | the external inputs it reads, one bit per class that changes on its own: `POINTER`, `KEYBOARD`, `CLOCK`, `VIEWPORT`, `HOST_DATA`, `RESOURCES`, `RNG`, `BINDINGS` |
| `probe` | the result is a pure function of the arguments and `reads`, and calling it again at validation is harmless (`mouse_x`, `hovered`, `time`). This is what lets a memoized scope stay valid when an input moved but the answer did not |
| `emits` | pushes into an output buffer the host drains after the run, the one kind of output a replay reproduces |
| `effect` | does something a replay cannot reproduce: prints, reseeds noise, creates or resolves a resource, advances a counter, reaches through a handle, writes host state |
| `pending: NativeClass` | a `Pending` argument is absorbed (`Strict`, the default), makes the call a no-op (`Effectful`, for emitters), or reaches the native (`AllowPending`, the pending inspectors) |
| `constant` | a constant function: the same arguments give the same result for the life of the program, reading no dynamic input of any kind. Asked through `is_const()`, which is false for a row that also reads, emits or has an effect |
| `contained` | with `effect`: the effect stays inside the execution (the print buffer, the noise seed, a counter, an output buffer, the resource table, the stack's method and enum tables), which a fork owns a copy of. A speculative run (direct manipulation v2) calls a contained native and drops what it did with the fork; it **suppresses** an effect that is not contained (answers `nil`). Off by default, so an embedder's effect is escaping until it says otherwise. Every core `effect` row is contained, and `core/tests/dm_v2_spec.rs` fails when a new one is not |
| `no_feedback` | direct manipulation may not move this native's result through its arguments: a literal whose every path to a dragged value goes through such a native's arguments is not offered as an edit (`through_rng`, docs/dev/direct-manipulation-v2-plan.md §2.5.1). The core sets it on `random`, `random_int`, `choose`, `noise`, `hash_int`, `hash01` and `hash_str`; any `reads(RNG)` row is treated the same way. It is the native-side twin of the `rigid` hint. Read by parameter discovery (`core/src/dm_v2/params.rs`); nothing at run time looks at it |

The constructors are `PURE`, `CONST`, `EMITS`, `EFFECT`, `probe(classes)`,
`reads(classes)`, and the builders `with_emits`, `with_effect`,
`with_pending`, `contained`, `no_feedback`. A row is the union over every path through the native: one
that consults the resource table only when handed a `Pending` still declares
`RESOURCES`. Over-declaring costs a missed replay; under-declaring is the bug.

From C the row is a bit set of `PB_FX_*` flags (`petal_bridge.h`), `PB_FX_PURE`
being `0`.

## Who reads it

| Layer | Reads | Where |
|---|---|---|
| Memoized scopes | the whole row, at every native call inside a recording scope: `effect` (or a `Pending` result) makes the scope effectful; `HOST_DATA` and `RESOURCES` reads become dependencies; other reads are probed if `probe`, else effectful | `memo_note_native`, `core/src/backend/bytecode/vm/memo.rs` |
| Static values | `is_const()`, through `NativeFnTable::constness`: a call is static only if its native is constant (`map`, `filter`, `reduce` and `sort` are constant when their callbacks are) | `core/src/static_classify/` |
| Pending values | `pending`, at the native-call boundary, only when an argument is pending | `core/src/backend/bytecode/vm/native.rs` |
| Speculative runs | `effect` and `contained` (suppress an escaping effect), `emits` (record the raw arguments), `reads(HOST_DATA)` (replay the baseline's answer), `reads(RNG)` (replay the draw by place) | `call_native_spec`, `core/src/backend/bytecode/vm/native.rs`; docs/dev/direct-manipulation-v2-plan.md §2.1.3 |
| Parameter discovery | `no_feedback` and `reads(RNG)` (the arguments are a feedback barrier) | `core/src/dm_v2/params.rs`; plan §2.2 |

The frame gate does not read rows. It reads the **activity counters**, which
the instrumented `PetalCxt` methods move by construction: `binding`,
`push_output`, `emit`, `print`, `next_counter`, `resources_mut` and the
others listed in `tools/native-effect-audit.ts`. `note_host_read()` and
`note_effect()` are the manual form for a native that reaches host state any
other way. The counters are also what the audits below hold the rows against.

## Checking rows

### The runtime effect audit

`core/src/effect_audit.rs`. With it on, the VM snapshots the activity
counters around every native call and accumulates, per native, what it was
seen doing. The report lists two kinds of finding:

- **under-declared**: a native did something its row does not cover. A bug;
  the row must grow.
- **over-declared**: a declared facet no audited call exercised. Not a bug,
  since a row is a union over paths the run may not have taken, but worth a
  look when the row was a guess.

One observed facet is not held against the row: an effect on a call that
returned a `Pending`, because the memo already treats that call as effectful.

```
$ petal run --effect-audit -e 'print(random() < 2)'
true
effect audit: 2 natives called, 0 under-declared, 0 over-declared
```

| Surface | |
|---|---|
| `petal run --effect-audit` | the builtins a script calls |
| `petal-ui-run --effect-audit` | the builtins and the `petal-ui` natives; exit code 3 for a clean run with an under-declared native |
| `Env::set_effect_audit(true)`, `Env::effect_audit_report()` | an embedder's own natives; `report.has_under_declared()` for a test |
| `core-libs/petal-ui/tests/effect_audit.rs` | every panel app in the corpus under a monkey scenario; fails on any under-declaration, prints over-declared facets with `--nocapture` |

Off, it costs one branch per native call. It sees only what moves a counter:
a native that reads a thread-local or the filesystem without calling
`note_host_read()` is as invisible to it as to the gate.

### The static scan: `tools/native-effect-audit.ts`

A grep-level pass over the Rust sources of `core`, `petal-ui`, `petal-query`,
`garden` and `integrations`. It finds each registration, matches the
function's body by braces, and classes the native as **instrumented** (its
body calls a counter-moving method), **manual** (`note_host_read` /
`note_effect`) or **SILENT** (neither). A SILENT body is right for a pure
function and suspect for anything else, so the script prints the SILENT
bodies that reach a thread-local, the filesystem, the clock, the environment,
a process, a random generator or the network:

```
$ ./tools/native-effect-audit.ts
repo          total  silent  instr  manual
core          148    133     14     1
petal-ui      25     16      3      6
garden        33     13      14     6
integrations  12     5       4      3

SILENT natives that reach outside their arguments: 1
  integrations queue_on_host           thread_local      integrations/petal-desktop-sdl/src/sound.rs
```

(16 October 2026. `queue_on_host` is a test fixture in `sound.rs` that
counts host calls in a thread-local on purpose.) `--all` lists every native,
`--json` gives the same as data. Its use beside the runtime audit is
coverage: it sees natives that no corpus app calls.

### Natives outside the repo: `tools/oracle-external.ts`

The frame-gate and memo differential oracle for worlds-fair's UI, which is not
in this repository and so not in the cargo corpus. It bundles each screen the
way `wf-ui-core` does, feeds it the real fixture models (`wf-ui
--print-fixtures`), and runs a monkey scenario under `baseline` and under
`fast-memo`, `replay` and `fast`, comparing the frames. It then runs each
fragment under `replay --effect-audit` and fails on an under-declared native.

```bash
cd core-libs/petal-ui && cargo build --release
cd ~/worlds-fair/ui && cargo build --release -p wf-ui-garden
./tools/oracle-external.ts [--wf <dir>] [--frames N] [--seed N] [--runner <petal-ui-run>] [--wf-bin <wf-ui>]
```

It exits non-zero when frames differ, when a fragment ran vacuously (fewer
than 10 draw commands a frame, which means its fixtures did not apply), or
when the audit found a native doing more than it declared.

### Constant rows: the static audit

A wrong `CONST` row keeps a stale value until the source changes, so it has
its own check. `petal run --static-audit` and `petal-ui-run --static-audit`
watch every dynamic input while a top-level term the classifier calls static
is evaluated, and name the native that touched one (`core/src/static_audit.rs`;
[CLI](../CLI.md#checking-the-answer---static-audit)). The set of constant
builtins is pinned by `const_builtins_are_exactly` in
`core/src/builtins/mod.rs`, and the rows of the host I/O builtins by the test
beside it.

## When adding a native

1. Start from `PURE` and add what the body does. A read of host state the host
   reports changes of is `reads(HOST_DATA)` with `note_host_read()` in the
   body; a write to host state is `EFFECT` with `note_effect()`. An effect
   that only touches the execution (a counter, an output buffer, `print`) is
   `EFFECT.contained()`, so direct manipulation's speculative runs still call
   it.
2. `CONST` only for a function of its arguments that reads nothing the host
   can load, swap or seed. When unsure, leave it dynamic.
3. Run the effect audit over a script that calls it from inside a helper
   function: a call at the top level is never memoized, so it hides an
   under-declared row.
