# Moses debrief: what is still open

`games/moses` in Cheesecake (*The Power Broker*, 27,000 lines in 33 modules)
was the largest Petal program when this debrief was written, on 11 October
2026. Most of what it asked for has shipped:

- trivial builtins as VM intrinsics;
- lints that run by default;
- `petal test` and `expect`;
- `const`;
- usable enums;
- petal-ui layers, hit testing and retained drawing;
- `f(...args)`;
- most of the stdlib gaps;
- the bridge's save/load state;
- the profiler;
- `petal --version`;
- `memo(key)`.

The full debrief, with the measurements, the final-state table and its
commits, was trimmed on 2026-10-23. Read it with
`git show b0dfccc7:docs/tasks/moses-debrief.md`. Source comments cite its
sections 2.1 and 2.2, which are there. Cheesecake's half of the debrief is
`~/cheesecake/docs/moses-debrief.md`.

## Held back by the owner (language design)

Each of these needs a proposal (`propose-language-change`) before work starts.

- **Checked record shapes (item 7).** `type Ctx = {date: Date, ...}` and
  option shapes with defaults, checked by `petal check` only:
  - an unknown field read;
  - an unknown key in a literal;
  - a missing required field.

  Moses has 966 `??` and 324 `field()` calls because nobody knows which
  fields are present. A misspelt option key silently takes the default.
- **Int-keyed collections and bit operators (item 9).** `Map` now takes
  non-string keys. Still missing: a `Set`, and `&`/`|`/`<<` (`&` does not
  lex, so bits are tested with `(mask / POW2[b]) % 2`).
- **Growable numeric arrays and tables of records (item 10).** Wanted:
  - `f64_array`/`i32_array` with in-place `push`/`extend`, passed to host
    natives as a pointer and a length;
  - `set T[i].f = v` that writes in place, or a columnar table;
  - `push xs, v` from a helper.

  In a career run GC is 12% of the time.
- **Module state without `get`/`set` (item 13).** Moses has 354
  module-level `state var` cells and 1,013 `get`s. Two suggestions:
  - read such a cell without `get` inside functions;
  - `reset` a module's state to its initializers. Moses lists each module's
    cells by hand in `reset()`, and one cell was missed.
- **Tuples and destructuring.** Positional lists carry magic indices
  (`f[7]`, `sd[2 + m * 2]`).

## Smaller, not started

- **Names (3.7):**
  - a module file named like a builtin or bundled package (`map`, `ui`)
    (compare module-system-followup issue 1);
  - `--set cam.DIST=…` instead of prefixing every `config let` by hand;
  - `config_overridden(NAME)`;
  - range/choice/doc metadata on `config let`.
- **VM:**
  - reuse of swept slots, and a nursery;
  - a two-target `Branch` and register coalescing. `LoadConst`, `Move` and
    `JumpIfPending` are still 35% of the opcode mix.
- **Appends that are still quadratic:** onto a list in a record field, onto
  a list in another list, and through a function that returns the list.
- **`petal test`:** catching a runtime error in a test, setup and teardown,
  and the call chain for a failure inside a helper.
- **Lints:** `prefer-match` for statement bodies.
- **Stdlib:** a priority queue, polyline helpers, a multi-generator `for`,
  list `repeat`, and a format spec in interpolation (`{x:.1f}`).
- **Bridge and check:**
  - lint findings are not exposed to a host;
  - `petal check` does not look inside a spread;
  - `petal.toml` cannot set a minimum Petal version.
