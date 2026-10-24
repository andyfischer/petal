# Static values: what is left

Status, 2026-10-23: **every planned stage has shipped**: A1 (enum read path),
A2 (frozen heap), A3 (cheaper `state` hit), S0 (classifier, `show-static`,
`--static-audit`), S1 (slot cache, `const`, `state-as-constant` lint), S2
(following the diff across reloads), the store per value, constants inside
function bodies, and follow-ups 11 (`unkept_part`) and 12 (`stale_caches`).
The design is in [the proposal](../proposals/static-values.md) and the
current behaviour in [internals/static-values.md](../internals/static-values.md).

The full work record (gate tables, measurements, reviews, and the S0/S1/S2/A1
sections that source comments cite) was trimmed on 2026-10-23. Read it with
`git show b0dfccc7:docs/tasks/static-value-optimization.md`.

## Open, ranked

1. **Per-input-class slots.** Cache values that depend only on the viewport
   or the theme until that input changes (reactive-rendering-plan P2). This
   is where UI panels would gain: the top-level cache saves them only about
   1% of instructions.
2. **Recompiling pays for full classification.** It costs 14 of 187 ms on
   neon and 47 of 488 ms on Moses. An incremental version needs the
   classifier's reads recorded as an explicit dependency graph. A further
   5 ms (neon) and 13 ms (Moses) of the cache's lowering overhead has not
   been investigated.
3. **Ecosystem re-vendors.** Until they re-vendor, worlds-fair (`wf-ui-ffi`,
   plus `fix-imports`) and the petal-lang.org WASM build (and
   `snippets/match.ptl`) get neither `const` nor the cache. The five
   `git.ptl` panels in `~/.garden` need commas.
4. **Function-body slots** are not kept through a patched or recompiling
   reload, and not kept under observation. They have no per-value cut and no
   `--static-audit` coverage. After every edit Moses rebuilds each of its
   4,381 such constants once.
5. **Fold literal `Enum.variant.key` reads.** About 0.5–0.7% on Moses, and it
   would let the game drop its `T`/`F`/`USE` int tables.
6. **Debug views.** The `explain` per-term trace turns the cache off. A
   recompile under observation re-runs every kept segment once to rewrite its
   logs.
7. **Not yet checked under Cheesecake:** the follow-up 12 stale-cache
   warning. Moses' map-grid half-reload is the case it was built for.
8. **Speculative, no profile asks for these:** keeping slow intermediates
   that feed several kept values; a frozen generation that can release
   objects; `state-as-constant` reaching `state` tables built from host
   natives (the lint is opt-in and has no host); and proofs, beyond tests, of
   the two equality arguments (a kept value leaving its function, and
   in-place edits under observation).
