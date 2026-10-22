# `petal check`: catch `memo` initializers that are not pure

Status, 2026-10-22: **open, not started.** A follow-up from the soundness
review of [`memo`](../features/memo.md).

## The problem

`memo(key) name = init` gives one entry per key value, whatever call path
asks. The design argument for that absolute key is that sharing a *pure*
result cannot be observed: every caller with an equal key would have
computed the same value anyway. Nothing checks that premise. An initializer
that reads anything besides the key and constants silently hands one
caller's answer to every other caller:

```petal ignore
fn scaled(k, scale)
  memo(k) v = k * scale     // the first caller's `scale` wins for every caller
  v
end
print(scaled(2, 10), scaled(2, 100))   // 20 20
```

Three other kinds of impure initializer break the premise in other ways:

- **Effectful natives:** `memo(id) tint = random(0.0, 1.0)`. Memo entries are
  dropped by a reload that changes the program, and by a run that does not
  reach them. Each recompute gives a new value, so the "constant per id" it
  appears to promise changes without warning.
- **Mutable reads:** `memo(k) v = get table[k]`. A later `set` on the cell
  is never seen.
- **Time and input:** `time()`, `mouse_x()`, `frame_count()`. The cached
  value is stuck at its first frame.

The same blind spot affects the `state-key-deprecated` auto-fix
(`core/src/lint/state_key.rs`). It rewrites any never-written `state(k)` to
`memo(k)`, including `state(id) tint = random(...)` in a script like
`plant.ptl`, which changes behaviour.

## The rule

Warn, in `petal check` and wherever compile warnings surface, when a memo
initializer depends on anything other than:

- the free variables of its key expression (the key is the cache's input, so
  reading what it reads is sound; a value *derived* from those variables is
  fine too);
- static data and static-pure functions as the
  [static-value classifier](../../core/src/static_classify/mod.rs) defines
  them (`const`, top-level static `let`s, `CONST` natives);
- calls to user functions that are pure under the same rules, with the
  arguments checked.

Report each offending read with its reason, the same way the classifier
gives a reason chain:

```text
warning: this `memo` reads `scale`, which its key `k` does not include, so
every caller with an equal key gets the first caller's value. Add it to the
key (`memo([k, scale])`), or use plain `state` if it is per caller. [memo-impure]
```

Separate messages for each kind of offence:

- **A local or parameter outside the key:** suggest adding it to the key.
- **An effectful or non-`CONST` native** (`random`, `time`, input, IO): say
  that the value is recomputed on reload and on a run that does not reach it.
  Suggest `state`, or a `state var` Map when the value must be kept per id.
- **A `get` of a cell or a read of `state`:** suggest putting the value in
  the key, or not using `memo`.
- **A call the analysis cannot resolve** (a dynamic callee or a parameter):
  report it as unknown, at a lower severity. Otherwise a correct `glsl.ptl`
  style cache, which calls through `f`, is noisy.

Suppress with `// petal-lint-ignore memo-impure`.

## Implementation notes

- **Reuse, don't rebuild.** The static classifier already runs a dataflow
  pass over the term graph with a data / function / dynamic lattice, the
  `CONST` constness of natives, and static-purity of user functions. The
  memo check is the same question with one extra "data" source: the key
  block's free variables. Look at whether the classifier can take a seed set
  of terms assumed to be data, rather than writing a second analysis.
- Native effect rows (`NativeEffects`, `--effect-audit`;
  `core/src/native_fn.rs`, `core/src/effect_audit.rs`) say which natives are
  pure enough. `random` and `time` must come out impure.
- **Closures in the initializer:** a lambda built inside the initializer and
  called there is fine. One returned as the cached value is already limited
  by the runtime's "no `var` cell in a memo value" check
  (`Vm::check_memo_value`).
- **Wire the same check into the `state-key-deprecated` fix.** Make the
  rewrite to `memo` only when the initializer passes. Otherwise report
  without a fix and suggest `state var` + Map. Update the doc comment in
  `core/src/lint/state_key.rs` and the inline-state.md deprecation note,
  which currently tell authors to watch for this themselves.
- **Run the check on the in-tree memos** and make each one clean or
  explicitly ignored. They are `nes.ptl`'s `_font_rows`, `glsl.ptl`'s
  translation cache, `examples/console/state.ptl`,
  `docs/features/function-introspection.md` and `petal-fps/LANGUAGE_IDEAS.md`.
  `glsl.ptl` reads `f` and `info` outside its key, and they are tied to
  `info.code_hash`, which is in the key. This is the case most likely to
  need either the ignore comment or a smarter rule: a value derived from a
  key component.

## Tests

- Each offence kind above warns once at the offending read, and the message
  names the binding.
- Clean cases do not warn:
  - a pure helper of the key;
  - a `const` table lookup;
  - a static-pure user function;
  - a record built from the key.
- Suppression works.
- The `state-key-deprecated` fix leaves `state(id) tint = random(...)` alone
  and still rewrites `state(text) w = measure(text)` (if `measure` is `CONST`).
- A sweep over `examples/`, `garden/` and the ecosystem trees listed in
  CLAUDE.local.md reports no false positives that have no clear fix.

## Docs

[memo.md](../features/memo.md): add a section on what an initializer may
read. Also update docs/CLI.md (lint and warning list) and the CHANGELOG.
