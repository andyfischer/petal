# What the docs pass found and did not fix

Status: **open items**, 2026-10-13. Found while reorganising `docs/` and
writing the new feature and internals pages. The docs were corrected to say
what the code does; the items here are in code, in files outside `docs/`, or
were left because they could not be verified. Nothing here has been fixed.

## 1. Probable bugs

Each was reproduced unless marked otherwise. The docs describe the behaviour
as it is.

### Language server

1. **A diagnostic from an imported module lands in the importing file.** When
   the compile succeeds, a warning or error found in an imported module is
   published on the importing document at the module's line and column, with
   no file name. Reproduced: an error on line 9 of `lib.ptl` shown at line 8
   of a 2-line `main.ptl`. The failed-compile path handles this;
   `petal_diagnostic_to_lsp` in `core/src/lsp/document.rs` does not check
   `span.file`.
2. **Positions are code points, not UTF-16.** `positionEncoding` is not
   negotiated, so columns are off by one for each astral character earlier on
   the line. Reproduced with `"😀"` before an identifier.
3. **A script that relies on a host's implicit prelude gets false "unknown
   function" diagnostics**, because the server compiles each file alone.
   `~/worlds-fair/ui/ptl/host/garden.ptl` gets 31 and `screens/missions.ptl`
   gets 78.

See [internals/lsp.md](../internals/lsp.md), "Known gaps".

### petal-sdl timeline

4. **`replay` after `set_state` discards the written value.**
   `integrations/petal-desktop-sdl/docs/agent-protocol.md` (`### replay`) says
   to call `replay` by hand after `set_state` on a rewound game. Replay always
   restores from a recorded fork, so `set_state x=100` then `replay` left
   `x=22`. Either the doc or the behaviour is wrong.
5. **`state` while frozen reads the live execution, not the cursor frame.** It
   returned `x=10` with the cursor at frame 2. Neither protocol doc says so.
6. **Windowed `--agent` mode does not rewind `time()`.** It binds wall-clock
   time, so only the frame number goes back. The plain windowed loop rewinds
   its simulated time. The fix would be in `run_agent`.
7. **Not tested:** what happens to sound requests emitted during an unfrozen
   `replay`. Replay skips `Host::end_frame`, so they may sit in the buffer
   until the next live frame.

See [features/time-travel.md](../features/time-travel.md).

### Differentiation

8. **Builtins that have no derivative rule drop the derivative silently.**
   `exp`, `log`, `pow`, `atan2`, `hypot`, `clamp`, `fract`, `lerp`,
   `map_range`, `smoothstep` and `dot` return a plain float with no warning.
   `vec2(dual, …)` silently stores the value.
9. **`sign(dual)` is an error while `abs(dual)` works.**

See [features/differentiation.md](../features/differentiation.md).

### Handles

10. **Not verified: `is_valid` under the memo.** It is registered `PURE`, so a
    recorded scope that only calls `is_valid(h)` could replay a stale `true`
    after the host destroys the object. One test did not show it, but the
    scope may not have been recorded.
11. **Not verified: handle methods and the frame gate.** A handle method marks
    the enclosing memo scope effectful (`memo_note_effect`) but does not call
    `run_deps.note_effect()`. Whether a run whose only host interaction is
    handle methods can be skipped by the gate was not tested.

See [game-engine-features/handles.md](../game-engine-features/handles.md).

### Other

12. **`config state`, `config fn` and `config pub let` give a poor error.**
    Only `var` and `const` have dedicated messages; the rest fall through to
    ``undefined variable `config` ``.
13. **`tools/calc-to-ir.ts` does not run under plain `node`.** It uses a
    TypeScript parameter property, which strip-only mode rejects, and it is
    not executable. `npx tsx` and `node --experimental-transform-types` work.

## 2. Stale text outside `docs/`

### `petal help`

- `petal help run` does not list `--runs <n>`, which exists
  (`core/src/cli/args.rs`) and works.
- `petal help run` says the default policy is `fast-memo`. The code uses
  `fast-memo-static` (`core/src/cli/handlers.rs`).
- `petal help show-static` says nothing depends on the classification yet and
  that "a later release" will keep static values. The cache has shipped.
- `petal help run` (`core/src/cli/help.rs`, `--effect-audit`) points at
  `docs/tasks/declarative-effect-refactoring.md`, which was deleted. The
  content is in [usage/ffi.md](../usage/ffi.md#catching-it-with-the-effect-audit).

### Comments that cite files that no longer exist

- `core/src/effect_audit.rs:12` and `core-libs/petal-ui/tests/effect_audit.rs:4`:
  `docs/tasks/declarative-effect-refactoring.md`.
- `core/src/handle.rs` and `core/tests/handles.rs`:
  `docs/dev/unreal-ffi-proposal.md`. The same string is in `docs/stdlib.json`,
  which is generated from the source.
- `core/src/program.rs:621`: `docs/ir-as-target.md` (now `docs/dev/`).
- `test/vitest/error-positions.test.ts`: `docs/dev/bytecode-future-ideas.md`.
- `tools/test-examples.ts` header: `./bin/test-examples.ts`.

### Comments that disagree with the code

- `core/src/run_deps.rs`, on `hash_content` (about line 439): says a handle is
  not fingerprintable. The code hashes it.
- `integrations/petal-c-bridge/include/petal_bridge.h:96`: the `pb_error.phase`
  comment omits `"link"`, which the bridge returns.

### Timeline: "after the cursor" is off by one

`integrations/petal-desktop-sdl/docs/agent-protocol.md` (`replay`), the
petal-sdl `README.md`, the header of `timeline.rs`, and the `replay` row in
[dev/debug-protocol.md](../dev/debug-protocol.md) say replay re-simulates the
frames after the cursor. It restores the fork of the frame before the cursor
and re-runs the cursor frame too.

### Other files

- `test/verify-plans/lint-fix.json` lists the corpus root `docs/examples`,
  which does not exist.
- `CHANGELOG.md` was not touched by the reorg, so its entries name the old
  paths. One is a link that no longer resolves:
  `docs/hot-reload.md#static-values-across-a-reload` (about line 230).
- `examples/AUTHORING.md:311` links
  `../garden/docs/petal-graphical-panels.md#supported-draw-surface`, an
  anchor that does not exist.

## 3. Left in `docs/`, not verified

Measured numbers were kept unless they contradicted the task records.

- [features/hot-reload.md](../features/hot-reload.md): the Moses
  `set_config_value` figures (4.08 / 4.61 ms; the nearest recorded are 4.04 /
  4.66), most cells of the `reload_follow` tables, and the 403-file corpus
  sweep.
- [dev/performance.md](../dev/performance.md): no figure was re-measured.
  About line 620 it names a test `measure_constants`; the real name is
  `measure_constants_inside_functions_on_the_corpus`.
- [usage/embedding-c.md](../usage/embedding-c.md): "Performance notes" (0.4 ms
  per frame, about 0.1 s per load, from Cheesecake) and the "Limitations"
  entry on runtime error positions. Its value table gives only `integer()` =
  slot for a handle; the serial (`count`) and class name (`str()`) are missing.
- [game-engine-features/petal-to-glsl.md](../game-engine-features/petal-to-glsl.md):
  the instruction counts and the caching table.
- [usage/text-and-fonts.md](../usage/text-and-fonts.md): the Garden row and
  the SDL "synthetic bold/oblique" claim.
- [language-guide.md](../language-guide.md): recursion through a callback
  limited to "a few hundred levels"; the `match` jump table turned off by
  `--no-opt`; class instances and stale labels across a reload.
- [features/direct-manipulation.md](../features/direct-manipulation.md): the
  batch example output cannot be reproduced with plain `petal run`, and the
  worked example marks `radius` as shared although one call reads it.
- [writing-petal-guide.md](../writing-petal-guide.md): uses the Garden-only
  `panel_store_get` as an example in a newcomer guide.
- [dev/testing.md](../dev/testing.md): the file and test counts.
- The plan documents under `dev/` were checked for paths, identifiers and
  links only, not read claim by claim.

## 4. Content still in the wrong file

Each is unique detail that sits outside the file that owns its topic.

| Where it is | What | Owner it belongs in |
|---|---|---|
| `usage/embedding-guide.md`, "Shipping a Petal library" | `Env::add_module_path` is best-effort; bad manifests are collected in `env.package_errors()` | `module-system.md#embedding` |
| `module-system.md`, "Embedding" | `Env::project_of`, `project_dependencies`, `resolve_import`, `packages()`, the `ModuleResolver` trait, `petal_ui::provide_interface` | `usage/ffi.md` |
| `usage/embedding-c.md`, "Modules and packages" | `override_file` semantics; the Rust `Env::override_file_source` is described nowhere | `module-system.md#embedding` |
| `usage/ffi.md`, "Hot reload" | The `ReloadReport` field list | `features/hot-reload.md` |
| `features/direct-manipulation.md`, "Tracing live code" | The `--trace-emits --json` field list | `CLI.md` |
| `dev/debugging-visibility.md` §1a, §1b | Cell-frontier JSON fields; `--observe` naming rules | `CLI.md` |
| `dev/testing.md`, `dev/refactor-verification.md` §1 | `--seed` and `--error-format bare` | `CLI.md` |
| `CLI.md`, "Traversing the IR" | JS helpers for reading `show-ir --json` | `dev/ir-as-target.md` |
| `writing-petal-guide.md` §2, §4 | Hoisting of constant `let`s; the shared-data recipe; flat parallel lists for hot loops | `language-guide.md`, `syntax/var.md` |
| `features/program-modification.md` | Fork and speculative execution API detail beyond what `features/time-travel.md` took | `usage/ffi.md` |

## 5. Pages that do not exist yet

- `features/pending-values.md`: pending values from the script side. Only
  [dev/pending-values-plan.md](../dev/pending-values-plan.md) exists, and
  `Builtins.md` has signatures alone.
- `features/provenance.md`: the dataflow tools as a feature (`petal graph`,
  `petal explain`), now covered only in `CLI.md`.
- `syntax/declaration-order.md`: hoisting rules are spread over four sections
  of the language guide.
- `usage/embedding-web.md`: the wasm hosts' JS-facing `PetalRuntime` API.
- `game-engine-features/frame-contract.md`: the per-frame contract an engine
  follows, now inside `usage/embedding-c.md`.
- A dev page for native effect rows and the audit tools
  (`tools/native-effect-audit.ts`, `tools/oracle-external.ts`); the design
  record was deleted.
- A script-side page for host I/O (`symbol`, `push_output`, `binding`); only
  the host side is explained.
