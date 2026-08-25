# Plan: call-path keyed `state` (React-`useState` semantics)

Status: **IMPLEMENTED** — 2026-08-25. Phases 0-4 landed; Phase 5 (ecosystem
re-vendor) is the only piece outstanding.
Date: 2026-08-24 (planned), 2026-08-25 (shipped)

The sections below are preserved as written, as the *intent*. Where the
implementation diverged from them, [§10](#10-what-actually-shipped) is
authoritative — read it before trusting a detail here. The user-facing
description of the shipped semantics lives in docs/language-guide.md and
docs/dev/Architecture.md ("State" → "Keying").

## 1. Motivation

The original intent of `state` (docs/dev/goals.md Goal 2 literally promises it:
"Inline `state` (React-`useState`-like, but a language primitive)") is that a
declaration site allocates state *per use*, the way a React component instance
gets its own hook slots. What shipped is a C-`static`-local: the slot key is a
hash of the variable name alone (`Compiler::state_key_for`,
rust/src/compiler/mod.rs:763), so **every callsite of a function shares one
slot** — and, worse, two functions that happen to declare the same state name
share one slot silently (the known name-collision bug; `state_inits` overwrite
at rust/src/compiler/stmt.rs:300, mis-resolution via `find_state_init`'s `Copy`
path at stmt.rs:750-756).

This plan changes the semantics: **each call path gets its own slot.** A helper
with a `state` in it, called from three places, holds three independent values.
Code that *wants* one shared value declares a top-level `state var` and
reads/writes it with `get`/`set` — which is already the language's sanctioned
shape for cross-function mutation.

Evidence the current model fights users:

- `sample-apps/petal-fps/LANGUAGE_IDEAS.md` §2 — "`state` inside functions
  doesn't work as you'd hope"; asks for exactly this.
- `examples/console/particles.ptl:5-10` — comments claim per-iteration keying
  that does not exist; all four "independent" particles share one slot today
  (verified by running it).
- `examples/console/reactive_ui.ptl` advertises "a React-like component model"
  its `button()` widget cannot deliver: a second instance would share the
  first's click count.
- `petal-ui/prelude/ui.ptl:473-475` — the `_theme_slot` accessor pattern exists
  *only* to launder the one-slot rule ("one function so there is exactly one
  `state` declaration").

## 2. New semantics (spec)

### 2.1 The call path

Every runtime frame chain from the program root down to a `state` declaration
defines a **path**: an ordered list of parts, one per dynamic step:

- **Call part** — pushed when a function/lambda/method is called; identifies
  the callsite (see §3.1 for the stable identity).
- **Index part** — pushed per loop iteration (`for`/range/`while`), at *every*
  level of the live frame stack, not just the declaring function's.
- The state declaration itself contributes its **declaration id** (§3.1).

A slot is `(decl_id, path)`. Two executions reach the same slot iff they
arrive via the same declaration through the same chain of callsites and loop
iterations. Consequences:

- Multiple callsites → independent slots (the headline change).
- Recursion → one slot per depth (each recursive call adds a path part), like
  nested React components.
- A widget function called inside a `for` gets per-iteration slots
  automatically (React's positional list keying). This finally makes
  `particles.ptl` behave as its comments claim.
- Same-named `state` in two functions can no longer collide — distinct decl
  ids by construction.

### 2.2 `state(key)` stays **absolute**

Today an explicit key *replaces* the loop-index vector entirely
(rust/src/backend/bytecode/vm/frame.rs:183-190). We keep that rule and extend
it: `state(expr) name` keys the slot by `(decl_id, hash(key value))` and
**ignores the call path**. It is the escape hatch for "same entity ⇒ same
slot, no matter who asks" — React's `key=` prop, but absolute.

This is load-bearing for existing code and must not become path-relative:

- `garden/examples/panels/plant.ptl:61-64,114-120` — lineage keying: the same
  leaf is reached one recursion level deeper each growth step; a path
  component would reset it every frame.
- `petal-fantasy-nes/prelude/nes.ptl:906` (`btn_repeat`) — two widgets asking
  about one button share one repeat phase *by documented design*.

With this rule, all 15 keyed declarations in the ecosystem keep working
unchanged.

### 2.3 Top-level `state` / `state var`: unchanged

Module-scope declarations run on the root path (empty), so their behavior —
and, if we keep decl-id derivation compatible (§3.1), their **persisted
values across the version upgrade** — is untouched. All 855 top-level
declarations in the ecosystem are unaffected.

### 2.4 The migration idiom

Intentionally shared, cross-function state becomes a top-level cell:

```petal
state var theme = default_theme()   // top level: one cell, one path

fn ui_theme()      get theme end
fn theme_set(t)    set theme = t end
```

This replaces the `_theme_slot(write, v)` accessor pattern everywhere it
appears. Note `get` is required for a bare cell read inside a function
(rust/tests/get_keyword.rs:92), which keeps the sharing visible at the read
site — a feature, not a cost.

### 2.5 Host entry points

`Env::call_function` runs with no caller frame. Rule: a host-invoked function
gets a root path of one call part derived from the function's qualified name.
Repeated host calls of the same function therefore share slots with each
other (matching embedder expectations for event handlers) but *not* with
in-program calls of the same function. Documented, with the workaround being —
as always — a top-level `state var`.

## 3. Design

### 3.1 Stable identity (the hard problem)

The entire hot-reload contract today is "the name hash survives the edit"
(rust/src/transfer_state.rs:36). Nothing positional survives a reload:
`TermId`/`BlockId`/`ClosureId` are dense indexes rebuilt per compile
(rust/src/provenance.rs:180-190 mandates discarding stale TermIds), source
spans shift on any edit above them, CST nodes have no persistent ids, and
`StmtKind::State.id` (ast.rs:378) is a parse-order counter — exactly the
fragility `hash_state_name` was written to avoid. So both new identity
components must be **name/structure-derived**:

**Declaration id** (replaces `StateKey`'s name hash, still a `u64` in the same
`StateKey` newtype):

- Top level: `hash(name)` for the entry file, `hash("module::name")` for
  modules — **byte-identical to today's keys**, so existing programs' hot
  state survives the upgrade.
- In a function: `hash(module ‖ enclosing-fn-name-chain ‖ var name ‖ shadow
  ordinal)`. The fn-name chain handles lexical nesting; lambdas contribute
  their binding name when bound (`let f = x -> ...`) and a per-function lambda
  ordinal otherwise. The shadow ordinal disambiguates re-declarations of one
  name in one function (rare; today they silently merge).

This alone — before any path work — fixes the name-collision bug.

**Callsite part**: `hash(canonical callee text ‖ ordinal among callsites with
identical callee text within the enclosing function)`. "Canonical callee
text" is the callee expression's source text with trivia stripped (`f`,
`obj.method`, `m::f`). Stability profile:

- Edits anywhere else in the file: path unchanged. ✅
- Renaming the callee, or adding/removing an *earlier* call to the same
  callee in the same function: that callsite's part changes and its subtree
  of state drops on reload. This is the same class of event as "renaming a
  state variable drops it", which is already the documented contract
  (docs/program-modification.md:176-177). Accepted.

Rejected alternatives: TermId / span (reload-fragile, above); explicit
user-visible site ids in source (invasive; `state(key)` already covers the
cases that need manual control).

### 3.2 Runtime key shape

```rust
enum PathPart { Call(u64), Index(usize), Key(u64) }   // replaces LoopKeyPart
struct RuntimeStateKey { base: StateKey /* decl id */, path: SmallVec<[PathPart; 4]> }
```

`RuntimeStateKey.loop_indices` (rust/src/stack.rs:25-28) is *already* a vector
of path parts — this generalizes it rather than replacing it. Explicit-key
slots are `{base, path: [Key(h)]}` (absolute, §2.2). Top-level slots keep an
empty path. `Stack.state`, `touched_state_keys`, sweep, and `gc_roots` are
shape-compatible.

Recursion makes paths grow with depth; `SmallVec<[_; 4]>` covers typical UI
trees without allocation. If profiling shows deep-path cost, a follow-up can
move to an incremental rolling hash per frame with a debug-only parts vector —
not in v1.

### 3.3 VM

- `VmFrame` gains `path_prefix: SmallVec<[PathPart; …]>` (or a shared parent
  pointer): computed once at call time as parent's prefix + `Call(site)` —
  frames already carry `call_site: Option<TermId>`
  (vm/frame.rs:32-34); the lowered call carries the precomputed site hash
  (§3.4). `recycle()` clears it like `loop_idx` today.
- Loop instructions (`ForEachInit`/`RangeInit`/`WhileInit`/`*Next`/
  `LoopBumpIdx`/`LoopPop`, vm/dispatch.rs:74-179) keep pushing/bumping Index
  parts — but into the frame's path rather than a separate `loop_idx`, and
  **unconditionally**: the `idx_ctx` gate (lower.rs:593,626 — already
  hard-coded `true`) and the `in_loop` static flag disappear.
- `Vm::state_key(base, in_loop, explicit)` (vm/frame.rs:173-201) simplifies
  to: explicit ⇒ `[Key(h)]`; else the current frame's full path. The
  walk-all-frames loop-index concatenation is gone — the composition happened
  incrementally at push time.

### 3.4 Compiler / IR / bytecode

- `state_key_for` (compiler/mod.rs:757-768) → decl-id derivation (§3.1); the
  module-qualification split folds into it.
- `compile_state_decl` (stmt.rs:276-323): drop `in_loop` (stmt.rs:299);
  `state_inits` stays `HashMap<StateKey, TermId>` but collisions become
  compile errors instead of silent overwrites (assert on insert).
- `StateWrite` emission (`rebind_name` stmt.rs:640-733, `find_state_init`
  stmt.rs:736-758) and the phi `Copy` tagging (phi.rs:236-280) are unchanged
  in shape — they resolve *decl ids*, which are still static. The `in_loop`
  copy-through (stmt.rs:673) is deleted.
- Each call term gets a `call_site: Option<u64>` (the §3.1 hash), computed at
  compile time, serialized on the IR like `state_key` is today, and threaded
  into the lowered call instruction (or a `TermId → u64` side table on
  `Program`, whichever keeps the ISA churn smaller — decide at implementation
  time; the side table avoids touching `Inst::Call` encodings).
- `Inst::StateInit`/`StateWrite` (isa.rs:355-382) lose `in_loop`;
  `emit_state_init` (lower.rs:839-889) otherwise unchanged, including the
  lazy-init/`Pending` no-commit rule (dispatch.rs:443-450) which is
  key-scheme-independent.
- `ir_validate`'s init-coverage invariant (ir_validate.rs:446-466) still holds
  at the decl-id level. `ir_equiv` keeps comparing `state_key` and starts
  comparing `call_site`; note the sensitivity change in docs (§6): extracting
  a helper or moving a call is now a *semantic* difference, and `ir-equal`
  reporting it is correct, not noise.

### 3.5 Hot reload

`transfer_stack_state` (transfer_state.rs:31-56) already matches on `base`
only and treats the rest of the key as opaque. That remains exactly right:
path parts are the new opaque tail. Behavior:

- Decl survives the edit ⇒ entries retained. Paths that no longer occur are
  swept by the existing untouched-key GC after the next full run
  (stack.rs:124-140) — no new mechanism needed.
- Call-structure edits (§3.1) silently orphan the old path and init a fresh
  slot. Same failure mode as a rename today; document it.
- Future work (explicitly out of scope for v1): structural path repair in
  `transfer_state` — remapping old paths to new when a single callsite
  ordinal shifted.

### 3.6 Host surfaces

- `get_state_json` (env/state_json.rs:11-58): top-level slots keep rendering
  as bare (module-qualified) names — **no change for every existing embedder
  consumer**, since all ecosystem host-inspected state is top-level. Pathed
  slots render as a slash-joined path: `main/counter#1/count`, loop parts as
  `[3]`, explicit keys as `k<hash>` (extending the existing `name[0,1]` /
  `name[k<hash>]` scheme).
- `set_state_from_json` / `set_state_map_from_json` (env/state_json.rs:60-115)
  stay top-level-only — that limitation is already documented
  (state_json.rs:84-98); pathed entries join loop-indexed entries in the
  not-addressable class.
- `Env::get_state`/`set_state` (env/mod.rs:620-684) already synthesize
  empty-path keys — correct as-is for top-level, which is all hosts touch.
- Debug protocol `state`/`set_state` (docs/dev/debug-protocol.md:43-44), SDL
  protocol, web-canvas `get_state_json`/`set_state_json`, and web-canvas
  props↔state name sync (top-level `state` vars by design) all keep working;
  they only see new key strings for in-function state, which nothing
  addresses by name today.
- `petal-ui-run` traces key state by name (docs/dev/headless-ui-run.md:54-55):
  every trace with in-function state changes byte shape ⇒ **all
  `test/ui-golden/index.json` hashes rotate** and must be regenerated via
  `ts/bin/verify.ts` after eyeballing the diffs.

### 3.7 Optimizer (escape.rs)

`multi_slot_keys` (escape.rs:240-251) currently exempts `in_loop`/explicit
keys from state-rooted copy elision because the runtime key mixes in live
context. Under path keying that reasoning covers **every in-function state**.
v1 rule: state-rooted elision applies only to declarations whose path is
statically empty — i.e. top-level state. That is where the accumulator
patterns live post-migration anyway, and it keeps `copy_elision.rs:405` /
`backend/bytecode/tests.rs:772,930` green with adjusted fixtures. A later
"path is statically fixed at this site" analysis can win back in-function
cases if profiles demand it.

## 4. Rollout phases

Each phase lands green on its own; semantics flip only at Phase 2.

**Phase 0 — decl ids (bug fix, semantics-preserving for correct programs).**
New `StateKey` derivation (§3.1) with top-level keys byte-compatible.
Same-name state in different functions stops colliding; `state_inits` insert
asserts uniqueness. Update the two tests that reach into the hash shape
(rust/tests/modules.rs:301,312). Ship independently.

**Phase 1 — migration lint + repo migration (forward-compatible).**
Add lint `state-shared-callsites`: an in-function `state` whose enclosing
function has multiple callsites (or any in-loop callsite). Then migrate all
in-repo reliers to top-level `state var` — these rewrites behave identically
under *old and new* semantics, so they land before the flip:

- `petal-ui/prelude/ui.ptl:477` `_theme_slot` → top-level cell (highest blast
  radius: theming for every petal-ui app).
- `petal-fantasy-nes/prelude/nes.ptl:128,492,695,723,946,1120` +
  `nes_sound.ptl:420` + `carts/petal_quest/game.ptl:147,153,159,165,171,177,
  292` — all 15 accessor-pattern slots.
- `examples/console/state.ptl`, `mutable_cells.ptl:77`, `state_machine.ptl`,
  `reactive_ui.ptl` (`accumulator`/`todo_app` are called inside `for` loops
  and would become per-iteration), `particles.ptl` — rewrite as teaching
  examples of the *new* model (per-callsite counters, per-iteration widgets,
  and one top-level shared cell), updating `expects` files.
- `~/biz/petal-lang.org/frontend/public/petal/snippets/state.ptl` (live on
  the site; `out/` copy regenerates on build).

**Phase 2 — the flip.** §3.2–3.5 + §3.7: PathPart, frame path, loop-part
unification, `in_loop`/`idx_ctx` removal, callsite hashes on call terms,
transfer/GC/validate/equiv updates, escape restriction. Update the ~470
inline Rust-test snippets and the ts/ suites (heaviest:
`petal-ui/tests/prelude.rs` 130, `backend/bytecode/tests.rs` 99,
`state_lifecycle.rs`, `transfer_state.rs` in-file tests). New tests, §7.

**Phase 3 — tooling surfaces.** state-JSON path rendering, CLI dump docs
(`kN`/`key=0x…` unchanged in shape, plus `call_site`), `ir-equal` docs and
comparison, regenerate `test/ui-golden/index.json`, diagram/SDL protocol doc
notes.

**Phase 4 — docs rewrite** (§6).

**Phase 5 — ecosystem.** Rebuild + re-vendor the WASM build for
petal-lang.org and hotlaps (see `~/biz/petal-lang.org/docs/how-to-update-
petal.md`) — their scripts are top-level-only and need no source changes;
garden GPP apps, both experiment apps, and ~/tools are unaffected (verified:
zero in-function `state`). petal-query is pure Rust host code; zero impact.

**Verification throughout:** differential runs with `--seed`/`PETAL_SEED` +
`--error-format bare` before/after each phase; `rust/tests/script_cases.rs`
expects files; `petal ir-equal` for the Phase 1 script rewrites where
equivalence is expected; `verify.ts` golden regeneration at Phase 3.

## 5. Migration inventory (from the full-ecosystem survey)

- **855 top-level declarations: unaffected** (all garden GPP apps, hotlaps,
  ~/tools, cube-browser, todo-app, games/productivity/dashboards/SDL/
  web-canvas examples, side-scroller, `rust/prelude/std.ptl` has none).
- **16 in-function declarations across 4 files must migrate** (accessor
  pattern; Phase 1 list above).
- **15 keyed `state(expr)` declarations: safe** given the absolute-key rule
  (§2.2) — plant.ptl (11), nes.ptl `btn_repeat`/`_menu_sel`/`_font_rows`.
  `_menu_sel` (nes.ptl:1039) is *also* an accessor slot; keeping `state(id)`
  absolute preserves it without migration.
- **~27 single-callsite in-function declarations: no behavior change**, except
  the three console examples called inside loops (migrated in Phase 1) and
  `ui.ptl:50` `elapsed()`, whose semantics *improve* (per-callsite timers —
  the doc comment at ui.ptl:44-48 currently documents the shared-timer
  wart and its workaround; rewrite it).
- **Silently fixed:** `particles.ptl`, `reactive_ui.ptl`'s widget model.

## 6. Docs to update

Rewrites (semantics stated): docs/language-guide.md:1424-1485 (State,
state var, state(key)) plus 129-165 module-state/capture notes;
docs/module-system.md:241-259 + 297; docs/dev/Architecture.md:400-455;
docs/program-modification.md:157-178, 239, 299-300; docs/ffi.md:159-197;
docs/syntax/overview.md:215-235; docs/dev/goals.md Goal 2 (finally true —
update status table); docs/dev/ir-as-target.md:171,235-237,333-335 (contract:
`call_site` operand, invariant restated over decl ids); docs/CLI.md:296-317
(`ir-equal` sensitivity: call moves are semantic now), 412, 626, 778-780,
858-906; docs/dev/debug-protocol.md; docs/dev/headless-ui-run.md:46-64;
docs/dev/refactor-verification.md:88-92; docs/dev/var-next-steps.md:123-127;
docs/dev/debugging-visibility.md:80; docs/dev/pending-values-plan.md:159-175;
docs/config-files.md:244; integrations/petal-desktop-sdl/docs/
game-dev-guide.md:23-34; integrations/petal-web-canvas/README.md:88-104;
garden/docs/writing-gpp-apps.md:266,383-411; README.md:47.

Close out: sample-apps/petal-fps/LANGUAGE_IDEAS.md §2 and §6 (both resolved
by this change — §6's reorder fragility becomes the documented Index-part
behavior with `state(key)` as the stable alternative).

Code doc-comments: compiler/mod.rs:755-762 (`state_key_for`),
escape.rs:92-110 module docs, capture_lag.rs:16-45 (rule unchanged — it is
scoped to module-level `state`, which keeps today's meaning — but its prose
mentions "the persisted slot" and should name the new model),
state_json.rs:84-98.

## 7. Testing plan

New coverage (ts/test + rust/tests):

- Per-callsite isolation: two callsites, independent counters; the old
  shared-accessor shape as a *negative* test.
- Same-name state in two functions (the collision bug) — Phase 0.
- Recursion: one slot per depth; state survives across runs at each depth.
- Caller-loop keying: widget called in `for` gets per-iteration slots; loop
  reorder drops positional slots (documented), `state(key)` inside the same
  shape survives reorder.
- `state(key)` absoluteness: two different callsites, same key ⇒ same slot
  (pins the plant.ptl/btn_repeat contract).
- Hot reload: unrelated edit preserves pathed state; adding an earlier
  same-callee call drops that subtree; top-level state upgrade-compat
  (old-format key survives into new build — snapshot/restore round-trip).
- `Env::call_function` pathing (§2.5).
- GC: path not taken this run is swept; explicit-key slot visited is kept
  (extends state_lifecycle.rs:112,139).
- Pending-init no-commit per path (extends env/tests.rs:2066-2145).
- `state var` per-path cells: cell persists, `set` works, no `StateWrite`.

Updated: state_lifecycle.rs (:38,:72 in-loop base-slot accumulation tests
now assert per-iteration), modules.rs keying tests, transfer_state.rs in-file
tests, copy_elision fixtures (top-level accumulators), ir-state/loop-state/
state-lazy-init/bug-state-in-if ts suites, ui-golden hashes.

## 8. Simplification opportunities (call-outs)

1. **Name-collision bug: fixed by construction** (Phase 0); delete the
   defensive prose around it.
2. **Module qualification of keys collapses** into decl-id derivation —
   `state_key_for`'s two-armed match, its doc'd move-a-decl caveat, and the
   hash-shape tests (modules.rs:301,312) all simplify.
3. **`in_loop` disappears end-to-end**: `Term.in_loop` (program.rs:344), the
   stmt.rs:299 set + stmt.rs:673 copy-through, `Inst` fields (isa.rs:355-382),
   `idx_ctx` (always-true, lower.rs:593,626), and the `Vm::state_key`
   frame-walk (frame.rs:191-197). A loop iteration is just a path part.
4. **The `loop_depth` anomaly dies with it**: today a lambda lexically inside
   a loop is `in_loop=true` while a fn *called from* a loop is false
   (compiler/function.rs:224 never resets `loop_depth`) — a semantic
   inconsistency that path keying makes unrepresentable.
5. **`LoopKeyPart` → `PathPart`** unifies loop/explicit/call keying into one
   concept; `state_json`'s `name[…]` rendering becomes one path renderer.
6. **Explicit-key "replaces loop indices" special case** becomes the defined
   absolute-key rule instead of an undocumented quirk.
7. **`StmtKind::State.id`** (ast.rs:378) is parsed in both pipelines but
   consumed by nothing — either repurpose it as the shadow-ordinal source or
   delete it from both parse.rs:673-675 and cst_project.rs:60-66.
8. **escape.rs `multi_slot_keys`** reduces from "any in_loop or explicit key
   anywhere on the base" to "path statically non-empty".
9. **The `_theme_slot`/`_cell` accessor idiom is deleted from the preludes**
   (~16 wrapper fns across ui.ptl/nes.ptl/nes_sound.ptl/game.ptl), replaced
   by plain top-level cells — less code *and* the pattern stops being taught
   by example.

## 9. Risks & open questions

- **Reload fragility of callsite ordinals** — the accepted-loss class grows
  (call-structure edits near a callsite drop its subtree's state). Mitigation
  is future path repair in `transfer_state` (§3.5). Watch for pain in the
  Garden live-editing workflow.
- **Path cost & growth** — per-op key construction is now O(path) hashing on
  a hot path (StateInit/Read/Write per frame); deep recursion grows keys and
  `touched_state_keys`. Measure with docs/dev/performance.md tooling; the
  rolling-hash fallback is designed but deferred.
- **Dynamic callees** — callee-text hashing keys `f(x)` where `f` is a
  parameter by the *text* `f`, so two different closures passed to one
  callsite share the callsite part (their state diverges only below, via
  their own decl ids). Acceptable? Alternative (hashing the closure identity)
  is reload-unstable. Current answer: yes, accept; revisit with evidence.
- **`while` + no-iteration-context cases** — `WhileInit` pushes
  unconditionally today; confirm no fixture depends on while-loop state
  accumulating across iterations (survey found none in .ptl, but inline Rust
  fixtures need the Phase 2 sweep).
- **`ir-equal` sensitivity** — refactors that move calls now legitimately
  change semantics; verify tooling that assumed "extract function is
  IR-equal" needs its expectations revisited (docs/dev/
  refactor-verification.md).
- **Do we want sugar for the shared case later?** (`state shared x`, or a
  `module state` form) — out of scope; the top-level `state var` idiom is the
  answer for v1, and the lint points at it.

---

## 10. What actually shipped

Landed over 2026-08-24/25 on `state-callsite-keying`:

| Phase | Commit(s) | What |
|---|---|---|
| 0 — decl ids | `54688db` | `state_key_for` derives the declaration id from the full name path; same-name collisions gone; top-level keys byte-identical |
| 1 — lint + migration | `4043b8d`, `b059275`, `ac27e4a` | `state-shared-callsites` warning (temporary; deleted after the flip — see below); console examples rewritten as teaching examples; the `_theme_slot`/`_cell` accessor idiom deleted from `ui.ptl`/`nes.ptl`/`nes_sound.ptl`/`game.ptl` |
| 2 — the flip | `66fd42f`, `d77ef70`, `e65e195` | `PathPart`, per-frame path, `in_loop`/`idx_ctx` removal, `call_site` on call terms, escape restriction, the test sweep |
| 3 — tooling | `3e348e2` | one path renderer in `get_state_json`, `ir-equal` sensitivity docs, dump docs, one golden re-baselined |
| 4 — docs | this commit and its siblings | §6's rewrite list |

The semantics in §2 shipped as specified: per-callsite slots, per-iteration
slots, one slot per recursion depth, absolute `state(key)`, unchanged
top-level state, and the §2.5 host-entry rule (`hash("host " ‖ name)`).

### Deviations from the plan

1. **`Term::path_pop` was added — the plan has no such field.** §3.4 said only
   to delete the `in_loop` copy-through, and §7 predicted the two
   `state_lifecycle.rs` in-loop accumulation tests would "now assert
   per-iteration". Implemented literally, that does not produce per-iteration
   semantics; it produces a *broken* slot. `state xs = []` at top level with
   `xs = append(xs, i)` inside a `for` writes `{xs,[Index(i)]}` while the
   `StateInit` and every reader address `{xs,[]}`, so the persisted slot stays
   `[]` and the accumulation is lost. (Verified by hand-zeroing `path_pop` in
   a `show-ir --json` document: the run leaves `xs: []` plus four orphan
   `[0]/xs` … `[3]/xs` slots.) `path_pop` is the static count of loop bodies
   between a declaration and an access — always well-defined, because
   assigning to a captured binding is a compile error, so the two are always
   in one function. Both `state_lifecycle` tests therefore still assert
   base-slot accumulation.
2. **`call_site` is a `Term` field, not a side table or an `Inst` operand.**
   §3.4 left the choice open. `Program.terms` already *is* the `TermId`-indexed
   table, so lookup is an array index on the hot call path, and the field rides
   the IR serialization and `ir_equiv` comparison that `state_key` already used.
3. **`VmFrame` carries the whole path, not a parent pointer.** §3.3 offered
   either. `recycle()` clears the vector but keeps its buffer and
   `frame_from_pool` extends the caller's parts into it, so a warm pool copies
   without allocating — which took the shallow-call cost from ~15% to ~5%.
4. **`StmtKind::State.id` was deleted, not repurposed** (§8.7 offered both). It
   is a global parse-order counter; the shadow ordinal has to be per-function
   and per-name, so the compiler derives its own in `state_key_for`.
5. **escape.rs needed no fixture adjustments.** §3.7 predicted "adjusted
   fixtures" for `copy_elision.rs` / `backend/bytecode/tests.rs`; the v1 rule
   ("state-rooted elision only where the `StateInit`'s path is statically
   empty") left all four `inplace_fires_*` fixtures green unchanged.
6. **The state-JSON rendering changed shape more than §3.6 implies, and rotated
   exactly one golden — not all of them.** Loop and explicit-key parts became
   ordinary path steps (`[0]/[1]/xs`, `k1234…/leaf`) rather than keeping the
   old `name[0,1]` / `name[k…]` suffix, which is what §8.5's "one renderer"
   costs. §3.6 predicted "**all** `test/ui-golden/index.json` hashes rotate";
   in fact Phase 2 rotated none and Phase 3 rotated one
   (`garden/examples/panels/plant.ptl`, the `state(key)` lineage app), because
   almost no traced app has in-function `state` at all. That diff was
   machine-checked before re-baselining: `state` was the only field that moved
   in any of 60 frames, and rewriting each old key `name[parts]` to
   `parts/name` reproduced the new trace exactly.
7. **Intrinsic closures get the intrinsic's callsite, not a per-element index.**
   `map`/`filter`/`reduce`/`sort`/`forEach` thread the `BuiltinCall` term's
   `call_site` through to the closure, so `map(xs, widget)` gives `widget`'s
   `state` one slot per `map` callsite, shared across elements. §2.1 defines
   `Index` parts as coming from `for`/`while` only and is silent on intrinsics;
   per-element keying there would be a follow-up.
8. **`ir_equiv` compares `path_pop` too**, not just `call_site` (§3.4).
9. **Callsite labels in host dumps are display-only and numbered globally.**
   §3.6 asked for `main/counter#1/count`; the `#n` in a rendered label is
   assigned per identical callee spelling in *term* order across the whole
   program, not per enclosing function as the compiler numbers ordinals. The
   slot is keyed by the hash, never by the string, so this costs a reader
   accuracy and nothing else.
10. **Hand-written IR and legacy documents degrade rather than fail.** A call
    term with no `call_site` contributes id 0, so every such call shares one
    part — exactly the pre-flip one-slot-per-declaration behavior. A stale
    `in_loop` field is ignored (unknown fields deserialize away).

### Measured cost (§9's open question)

Release binaries, pre-flip vs post-flip on identical sources: deep recursion
(depth 300, 6M calls) 3.62 s → 5.06 s (**1.40×**, the O(depth) path copy per
call); `fib(27)` within noise; a 3M-iteration top-level state loop ~6%; a
shallow call-heavy widget tree with no state ~5%. The rolling-hash mitigation
stays designed-but-deferred, as §3.2 allowed.

### Resolved after the flip

- **Phase 5 — ecosystem: done 2026-08-25.** The WASM build was rebuilt and
  re-vendored for `~/biz/petal-lang.org` and `~/biz/hotlaps` (both stamped
  `petalCommit: bad26bc`, up from `ffdc68c`), and `~/worlds-fair` — a consumer
  §5's survey missed entirely — re-vendored its wholesale copy of the source
  (`vendor/petal/`, 90 files, +14915/-2074).

  §5 was wrong to call the ecosystem top-level-only. Two scripts needed real
  source changes: `~/biz/petal-lang.org`'s live `state.ptl` snippet (whose
  `counter() // 2` comment the flip falsified) and `~/worlds-fair`'s
  `ui/ptl/host/garden.ptl`, whose `wf_fixture()` shared three in-function
  `state` values across two callsites. That one was a genuine behavior break,
  and subtler than predicted: two callsites alone stay in step, because both
  run once per frame and see the same key edges. They diverge only when one
  caller is skipped — `screens/hud.ptl` returns early while the model is
  pending — after which the two pickers never re-agree. Migrated to module-level
  cells, with a regression test.

  The re-vendors also surfaced breakage unrelated to `state`, which nothing had
  been checking: three syntax/semantic drifts in the site's hand-authored Petal
  (trailing commas in enum variants, the removed `fn(x) { … }` lambda form, and
  `push` no longer mutating) left two docs-site tour blocks failing to parse
  outright.

- **The `state-shared-callsites` lint was deleted** (user decision,
  2026-08-25). It was a Phase 1 *migration* warning, and the flip made its
  premise false: it announced that "`state` is moving to per-call-path keying"
  and fired on an in-function `state` with several callsites — which is now the
  intended per-callsite-counter idiom and behaves correctly. Gone with it:
  `rust/src/typecheck/state_callsites.rs`, its registration in
  `compiler/mod.rs`, `rust/tests/state_shared_callsites.rs`, and its row in
  `docs/CLI.md`'s compile-time-lints table.

- **Code doc-comments named in §6** are clean. `escape.rs` no longer mentions
  `in_loop` anywhere (the Phase 2 commits took those comments with the field),
  and `transfer_state.rs`'s test-module comment now says "pathed keys" instead
  of "loop-indexed keys".

### Still outstanding

- **The installed `garden` binary is stale.** `~/.cargo/bin/garden` dates from
  2026-08-04 and predates the `get` keyword, so it rejects any script using a
  top-level `state var` cell — i.e. exactly the migration idiom this change
  tells people to adopt. A `garden-dev.ts --headless` loop against a migrated
  script fails with `Expected 'then', got …` until Garden is rebuilt from
  `garden/` (`./garden/install-local.sh`). The vendored-Petal test paths are
  unaffected.
- **Structural path repair in `transfer_state`** (§3.5, explicitly out of scope
  for v1): remapping old paths onto new ones when a single callsite ordinal
  shifted. Worth revisiting if the accepted-loss class (deleting the first of
  two `f()` calls hands the survivor the first one's state) causes pain in the
  Garden live-editing workflow.
