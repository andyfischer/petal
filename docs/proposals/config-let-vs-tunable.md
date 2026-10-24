# Proposal: replace `config let` with `tunable`?

Status: accepted, implemented 24 October 2026 (design E, with the
conservative defaults for open questions 3 and 4; open question 2 was
reversed the same day and the DM weights merged into one knob tier; see
[Implementation notes](#implementation-notes-24-october-2026) for what was
built and where it deviates from the sketch). This answers the question "Can
we get rid of the `config` keyword and just use `tunable` instead? This would
simplify the language surface, and tunable can also customize min/max/step."

## Summary

The question asks whether `config let x = v` can be removed and
`let x = tunable(v)` written in its place. `tunable(value, min?, max?,
step?)` is the identity builtin from Direct Manipulation v2.

**Recommendation: reject the removal and adopt a smaller version.** Keep
`config let` as the declaration-level marker for "this binding is a setting
that hosts list, set and override", for any kind of data. Keep `tunable` as
the expression-level, numbers-only DM hint. Make the two compose. That means
documenting `config let X = tunable(v, min, max, step)` as the way to give a
knob a range, and teaching the host-facing readers to see through that form:
`static_bindings`, the C bridge, `set_config_value`, and the DM preference
for config records.

That delivers the part of the request that has real value: a slider knows its
min, max and step. It also fixes two existing bugs found while researching
this. The language surface does not grow, and none of the 1,500 existing
uses breaks.

Three findings drove this:

1. **`config let` is not only a DM marker.** It is the static, unshadowable
   flag that hosts build features on. These include Cheesecake's Tuning
   screen, its post-effects panel, its CLI `--set NAME=VALUE` and run
   profiles, the C bridge's `"config": true`, `set_config_value` across zero,
   and the reload reports. `tunable` is invisible to all of them, which was
   measured (see [Motivation](#motivation)).
2. **28% of today's knobs are not numbers.** Of the 1,500 `config let` lines,
   414 hold a bool, string, colour, record, list, `vec3(…)` or `nil`. For
   those, `tunable` is a lint warning today and marks nothing, and min, max
   and step mean nothing.
3. **Removal does not simplify; it moves the special case.** To match what
   `config` does, `tunable` would need several things:
   - to accept non-numeric data;
   - a name-based special case in the compiler's sign fold, in
     `static_bindings`, in `source_diff` and in `set_config_value` paths;
   - protection against being shadowed.

   That amounts to `config` spelled as a call, with weaker properties. Most of
   the cost lands on a vendored project with 1,326 uses.

The three perspective reviewers (goals and writing; bugs, reload and
introspection; maintenance and migration) each reached this conclusion
independently. The strongest argument against it is under
[Assessment](#the-case-for-removal-strongest-counter-argument).

## Motivation

The request is right that there is overlap. A top-level number can be marked
in three ways today: `config let x = 4`, `let x = tunable(4)`, and
`config let x = tunable(4)`. DM v2 gives them different weights, 0.5 and 0.25
(`core/src/dm_v2/rank.rs:24-27`), and users cannot see that difference. It
also has a `Prefer::ConfigStrict` mode that fires only on `config`
(`core/src/dm_v2/api.rs:564`). Removing one marker would cut that overlap. To
see what removal would cost, here is everything `config` does today.

### Everything `config let` does today

| Place | What it does | Code |
|---|---|---|
| Lexer / parser | Contextual modifier, not reserved (`let config = …` still works). It has six special errors: `config var`, `config const`, `config state`, `config fn`, `config pub`, and the `pub` variants. | `lexer.rs:247`, `parse.rs:13-17, 604-736, 1282` |
| AST / CST | `StmtKind::Let.is_config`. The `config` ident is a child of `LetStmt`. | `ast.rs:587`, `cst_project.rs:302` |
| Compiler | Sets `Term.is_config` on the binding's root value term. **Sign fold:** a negated number literal inside the config data compiles to one `Constant`. | `compiler/stmt.rs:43-78` |
| Hot reload | `ConstructKind::ConfigLet`. A sign flip in config data is a value patch; in a plain `let` it recompiles. | `source_diff.rs:201, 640`, test `:1628` |
| Live set | `Env::set_config_value` / `pb_vm_set_config` work on any top-level `let` of data, but only config data can cross zero. | `env/reload.rs:588` |
| Static readers | `StaticBinding.is_config` in `static_bindings`. In the C bridge, `"config": true` appears in `pb_source_bindings_json` and on each changed value in the reload report. | `static_value.rs:611, 688-750`, `petal-c-bridge/src/source.rs:194`, `vm.rs:1946-1962` |
| Introspection | `show-static` (`kind: "config let"`), `show-ast`, `fn_ast`, `show-ir --json`, and `static_classify::DeclKind::ConfigLet`. `ir_equiv` compares the flag. | `static_classify/mod.rs:228, 1303`, `ir_equiv.rs:518` |
| Refactors | `apply-change convert-to-var` refuses a config binding. | `apply_change/convert_to_var.rs:82` |
| DM v1 | Once any config let exists, every other named binding is pinned. | `direct_manipulation.rs:178-489` |
| DM v2 | Weight 0.5, config-only subsets, `Prefer::ConfigStrict`, `--config-strict`, the selector `config`. | `dm_v2/params.rs:1135`, `rank.rs:24`, `api.rs:119, 564`, `cli/dm_v2.rs:148, 275` |
| Garden | `:DragAllow config`, `CandState::Config`. Line targeting skips a leading `config`. | `garden-app/src/app/dm_mode.rs:89`, `garden-app/src/dm_hints.rs:404` |
| MCP | The `allow`/`configStrict` parameters of the propose-edit tool. | `tools/petal-mcp.ts:246-273` |
| LSP / grammar | Keyword list; tree-sitter `config` token and highlight. | `lsp/document.rs:1425`, `editor-support/tree-sitter-petal/grammar.js`, `queries/highlights.scm:37` |
| Lint | `dm-hint-state-init` tells the user to "declare the start value with `config let`". | `lint/dm_hints.rs:91` |
| **Cheesecake (host)** | **The Tuning screen** lists every `b.config` binding of every source file and draws controls for numbers, colours and nested records. It drives `set_config` live and writes the file on release. **The effects panel** prefers a config record. **`--set NAME=VALUE` and `profiles.json`** use a textual scanner for `[pub] config let NAME =` that replaces the initializer. **`reloaded_values()`** documents config values. | `~/cheesecake/engine/editor/src/tuning_panel.cpp:91, 354`, `effects_panel.cpp:345-352`, `engine/script/src/config_overrides.cpp:65-100`, `natives_scene.cpp:328`, `engine/app/src/main.cpp:83, 95` |

Several things do **not** depend on the keyword:
- the type checker;
- goal-based editing and `.ptl`-as-config files (`goal_based_editing.rs`,
  `docs/usage/config-files.md`): they work on any binding;
- Garden's `init.ptl`, which has no `config let`;
- `static_follow.rs`;
- Kani proofs and the `--no-opt` oracle, neither of which reads `is_config`.

The keyword therefore has real semantics. It does not change program values,
but it does change:
- which bindings are listed, set and overridden from outside;
- how a sign edit reloads;
- how DM ranks.

Cheesecake already has the features the question asks about: CLI `--set`,
host-listed knobs, and a slider UI. All three key on `config`.

### What `tunable` is today

`core/src/builtins/hints.rs`, `core/src/dm_v2/hints.rs` and
`docs/Builtins.md` ("Direct-Manipulation Hints") define it:
- It is the identity function and has a `CONST` effect row.
- It marks the one **number** literal its argument resolves to.
- It can sit at a definition, at a use, or as a bare statement naming a
  binding.
- It is recognised only while its name resolves to the core's native. A user
  `fn tunable` or a host native of the same name turns it off.

The accepted hints proposal states that "`config let` stays the top-level
spelling" ([dm-v2-hints.md](dm-v2-hints.md#interaction-with-config-let)), and
so does plan §2.6.

### Measured: what a host sees when `tunable` replaces `config`

These results come from a probe that links core (`static_bindings` and
`set_config_value`), plus `petal show-ir`, `lint` and `propose-edit`.

| Source | `static_bindings` | `set_config_value` |
|---|---|---|
| `config let SPEED = 4.0` | `is_config=true`, `Float(4.0)` | `SPEED`=5.5: ok |
| `let RADIUS = tunable(12.0, 4, 40)` | `is_config=false`, `Call{function:"tunable", args:[12.0, 4, 40]}` | `RADIUS`=20: **NeedsReload**. `RADIUS[0]`=20: ok |
| `let NAMED = tunable(12.0, min: 4, max: 40)` | **`Err("a named argument")`**: not static at all, though `show-static` calls it static | `NAMED[0]`: **NotFound** |
| `config let NEG = -0.5` | `Float(-0.5)` | set to 0.5: ok |
| `let TNEG = tunable(-0.5)` | `Call{tunable, [-0.5]}` | `TNEG[0]`=0.5: **NeedsReload** (`Neg` over `Constant`, no fold) |
| `config let X = tunable(-0.5)` | (`is_config=true`) | sign flip is a **value patch**: the fold reaches call arguments (`source_diff.rs:90-95`) |
| `let FLAG = tunable(true)` | `Call{tunable, [Bool(true)]}` | n/a. Lint `dm-hint-not-literal`: "this argument is computed" (wrong wording) |
| `tunable(gap, 0, 40)` on its own line | `gap` is a plain `let`; the hint is invisible | DM ranks it `[config]`, score 0.29 |
| `let s: int = tunable(1.5)` | | no type warning. `config let t: int = 1.5` warns `type mismatch` |

### Two existing bugs found along the way

1. **Literals inside a `config let` record or list are not treated as
   config.** In `config let BOX = {x: 140, y: 90}`, the `140` ranks as a plain
   `let` under DM v2 Soft, with score 1.01 against 0.51 for a scalar
   `config let`. `propose-edit --allow config` on it fails: "nothing can move
   the goals … line 2: not among the allowed parameters". The cause is that
   `stmt.rs:78` puts `is_config` on the record constructor term only, and
   `params.rs:1123-1135` looks only at the literal's own named term. 113
   config lines are records or lists.
2. **`--config-strict` does nothing in a program that uses only `tunable`.**
   The trigger is "some term is `is_config`" (`api.rs:564`).

### Corpus

There are 1,500 `config let` lines in 197 files. All are at the top level
(none indented), and 1,101 are `pub config let`.

| Where | Lines | Files |
|---|---|---|
| `petal/examples` | 131 | 19: games (asteroids 11, boids 14, hopper 12, particle-sandbox 12, fractal-explorer 11, flappy 9, terrain 9, memory 7, pong, breakout, tower-defense 6 each, physics-playground, solar-system, tetris 5 each, 2048, minesweeper 4 each) and `direct-manipulation-v2/` (a01 2, l01b 2, l07b 1) |
| `petal/garden` | 14 | 4: `examples/panels/plant.ptl:8-17` (10), `drag-to-edit.ptl` (2), `dm-v2/01-helper-row.ptl`, `dm-v2/03-fractal-tree.ptl` (1 each, untracked) |
| `petal/core/tests/dm_v2/04_random.ptl` | 2 | 1 |
| core-libs, core-runtime, test/, integrations `.ptl`, `~/worlds-fair/ui/ptl`, `~/petal-creative`, `~/biz/petal-lang.org` `.ptl`, `~/.garden`, `~/tools` | 0 | 0 |
| `~/cheesecake/games` | 1,326 | 159: moses 38, solitude 28, moonbear 24, neon 12, skyrace 11, tides 10, gta 8, stride 6, … |
| `~/cheesecake` tests, engine, docs | 26 | 15 |

Cheesecake's `build/` and `temp/` copies are excluded. `sample-apps/` does not
exist.

**Initializer kinds:**

| Kind | Lines |
|---|---|
| float | 871 |
| int | 215 |
| bool | 123 |
| string | 86 |
| colour | 81 |
| record | 77 |
| list | 36 |
| `vec3(…)` | 7 |
| `nil` | 2 |
| computed (`level.SPAWN`) | 2 |

That is **1,086 numeric (72%) and 414 non-numeric (28%)**. In Cheesecake:
- 19 numeric initializers are negative;
- 748 lines carry a trailing comment, often column-aligned;
- 39 initializers span several lines.

`tunable(` appears in 6 lines of 4 `.ptl` files, and in none of Cheesecake.
The raw counts and the classifier script are in the session scratchpad
(`config-tunable/classify.py`).

Outside `.ptl` files:
- about 259 occurrences of `config let` in Rust, C++ and TS tests and sources
  in `~/petal`;
- 22 docs in `docs/`;
- 10 mirrored doc pages on petal-lang.org;
- about 185 `--set` uses and 3 `profiles.json` files in Cheesecake.

User feedback from the moses game (`docs/tasks/moses-debrief.md:59-63`) asks
for **more** declaration-level metadata: "range/choice/doc metadata on
`config let`", `--set cam.DIST=…` on imported modules, and
`config_overridden(NAME)`.

## Design

The designs evaluated (A to D) are under
[Alternatives considered](#alternatives-considered). This section specifies
the recommended one (E).

**1. The division of labour, as the guide states it.**
- `config let` declares a setting: a binding that hosts list, set live and
  override from outside, of any data type.
- `tunable(…)` marks a number that direct manipulation should prefer, with an
  optional range. It works anywhere a number is written.
- They compose: `config let SPEED = tunable(4.0, 0, 10)` is a setting with a
  range.
- `rigid(…)` forbids a number from moving.

**2. `static_bindings` sees through `tunable` on a config binding.** When a
`config let`'s whole initializer is `tunable(v, …)` and `v` is static:
- `value` is `v`, not `Call{tunable}`;
- a new field `range: Option<{min, max, step}>` carries the bounds;
- named arguments (`min:`, `max:`, `step:`) are accepted in this one position.

This is recognised by name **only under `config`**, where the declaration
already says "this is a knob", so a shadowed `tunable` cannot hide one. A
plain `let x = tunable(…)` stays a plain let to static readers, as it is
today.

**3. The C bridge.** `pb_source_bindings_json` adds
`"range": {"min":…, "max":…, "step":…}` beside `"config": true`, and
`"value"` is the unwrapped value. This is additive: an old host ignores the
field.

**4. `set_config_value`.** A path naming a config binding whose initializer
is `tunable(v, …)` addresses `v`, so `SPEED` means `SPEED[0]`. Bounds are
**not** enforced, which matches DM, where they are soft. A host can clamp to
`range`. Whether they should be enforced is an open question.

**5. Bug fixes.**
- The compiler marks every literal in a config binding's data positions as
  config. It reuses the walk of `config_folded_numbers`, so nested record and
  list literals get the config weight and match `--allow config`.
- `Prefer::ConfigStrict` also fires on a program with `Hint::Tunable` marks.

**6. Lint messages.**
- `dm-hint-not-literal` on a non-numeric literal says: `` `tunable` marks a
  number; `"title"` is a string. To expose a setting of any type, declare it
  with `config let` ``. It no longer says "this argument is computed".
- A new warning, `config-not-data`, covers `config let X = level.SPAWN`. It
  reads: `` a `config let` initializer is read by hosts without running the
  program; `level.SPAWN` needs evaluating, so hosts see no value ``. This is
  the one check `tunable` has that `config` lacks.

**7. DM weights. Open, not decided here.** One option is to merge "config" and
"tunable" into one 0.25 tier, so that marking something twice changes
nothing. See open question 2. (Later decided: merged, at 0.5.)

Nothing in the parser, AST, IR shape, bytecode or keyword list changes.

## Before and after

Every "before" was run or checked with `core/target/debug/petal`, built
2026-10-24. Cheesecake files need host interfaces, so the snippets were
extracted and run on their own. Two "after" columns appear where they differ:
**removal** (design A/B) and **recommended** (E). Neither runs today unless
marked.

### 1. The common case: numeric knobs

`examples/games/asteroids/app.ptl:50-60` runs as an extract and prints
`3 4.4 2.2`.

```petal
// before
config let start_lives   = 3
config let turn_rate     = 4.4      // rad/s
config let drag          = 0.55     // fraction of velocity lost per second
```
```petal
// after, removal (A/B) -- proposed
let start_lives   = tunable(3)
let turn_rate     = tunable(4.4)    // rad/s
let drag          = tunable(0.55)   // fraction of velocity lost per second
```
```petal
// after, recommended (E) -- unchanged, with ranges where wanted
config let start_lives   = 3
config let turn_rate     = tunable(4.4, 0, 10)    // rad/s
config let drag          = tunable(0.55, 0, 1)    // fraction of velocity lost per second
```

What changes:
- **Under removal,** each line gains a wrapper and an aligned comment column
  shifts, but the names stay. A newcomer reads `tunable(3)` as a function
  call and may expect it to do something at run time.
- **Under E,** a range is opt-in per knob, and the knob stays a declaration
  that hosts list. Today `config let x = tunable(…)` already runs, folds signs
  and patches live; only the host readers do not report the range.

### 2. Non-numeric settings: where `tunable` does not fit

`~/cheesecake/games/rivet/config.ptl:5-11` runs as an extract and prints
`title -1.0 false true`. The whole file passes `petal check`.

```petal
// before
pub config let START = "title"        // "title" | "select" | "play": where a run begins (tests, screenshots)
pub config let START_X = -1.0          // with START = "play": begin this many tiles into the stage
pub config let AUTOPLAY = false       // a simple bot holds the pad: runs right, jumps, shoots (tests, soak runs)
pub config let GOD = false            // take no damage (screenshots, the level tour)
```
```petal
// after, removal -- proposed; today lints dm-hint-not-literal on three of four lines
pub let START = tunable("title")
pub let START_X = tunable(-1.0)       // now recompiles when dragged across zero
pub let AUTOPLAY = tunable(false)
pub let GOD = tunable(false)
```

Here `config` means "override point" (`--set START=play`, profiles), not "drag
knob". The file's numeric tuning (`RUN`, `GRAV`, lines 13-34) is plain `let`.
`tunable` would have to be widened to every type, and then min, max and step
mean nothing for most of these. The metadata this author wants is a choice
list ("title" | "select" | "play"; moses-debrief), which `tunable` cannot
express. **The old form is the better one here, and E leaves it unchanged.**

### 3. A record knob

`~/cheesecake/games/rivet/game.ptl:32-40` runs as an extract and prints
`0.32 216`.

```petal ignore
// before
pub config let POST = {
  exposure: 1.2, bloom: 0.32, bloom_threshold: 1.0, vignette: 0.28, saturation: 1.12, contrast: 1.04,
  effects: [
    {effect: "chromatic_aberration", amount: 0.12},
    {effect: "scanlines", amount: 0.16, lines: 216, roll: 0.05},
```
```petal ignore
// after, removal -- proposed: wrap every field you want listed
pub let POST = {
  exposure: tunable(1.2), bloom: tunable(0.32, 0, 1), ...
```

Under removal, the binding-level flag that `effects_panel.cpp:352` and the
Tuning screen use is gone. Each of up to 20 literals needs its own wrapper,
`effect: "crt"` cannot be marked at all, and nothing tells a host that `POST`
as a whole is a setting. Under E the source is unchanged, and the fix to
nested `is_config` makes a drag prefer these literals. Today it does not.

### 4. Ranges and DM hints together

`examples/direct-manipulation-v2/l07b-fractal-tree-hints.ptl:16-18` passes
`petal check`.

```petal
// before
let trunk = rigid(80.0)
let shrink = tunable(0.7, 0.5, 0.8)
config let spread = 0.45
```
```petal
// after, recommended -- proposed (the source already runs; the change is what hosts see)
let trunk = rigid(80.0)
config let shrink = tunable(0.7, 0.5, 0.8)    // a listed setting with a slider range
config let spread = 0.45
```

This is the case the request is about. It is the strongest argument for
`tunable`, and E keeps it. `shrink` gets a range for DM today; under E a host
slider gets the same range. Under removal, `spread` becomes
`let spread = tunable(0.45)`, which reads fine.

### 5. Interaction with `state` and hot reload

`examples/games/asteroids/app.ptl:50, 80`.

```petal
// before
config let start_lives   = 3
state var lives     = start_lives
```
```petal
// after, removal -- proposed
let start_lives = tunable(3)
state var lives = start_lives
```

No difference in behaviour. Editing `start_lives` is a value patch in both,
and `lives` keeps its value across the reload. Two things change:
- `dm-hint-state-init` today says "declare the start value with `config let`",
  which under removal has nothing to point at.
- The migration edit itself (`config let X = 4.0` to `let X = tunable(4.0)`)
  is structural. Each migrated file recompiles once on a running game, with
  state kept (measured by the bug-freedom reviewer, whose reload equalled a
  fresh compile).

The difference shows on a sign flip. `tides/config.ptl`'s
`pub config let SEA_Y = -0.45`, dragged to `0.1`, is a value patch today. As
`let SEA_Y = tunable(-0.45)` it recompiles, and `set_config_value` refuses it.

### Migration

- **Under E,** nothing to migrate. Adding a range is a manual, per-knob
  choice. Hosts that want ranges read the new optional field.
- **Under removal,** a `petal lint --fix` modelled on `lint/export_to_pub.rs`
  would be mechanically correct only for non-negative numeric scalars: 921 of
  Cheesecake's 1,353 lines (61%). It also needs new padding logic, because
  `plan_pub_rewrite` handles a keyword that *shrinks* and 748 lines have
  aligned comments.
  - The 19 negative numbers keep their value but lose the sign fold and live
    setting across zero, unless the compiler learns `tunable` by name.
  - The 414 non-numeric lines have no target until `tunable` is widened.
  - Multi-line records change meaning: one flag on the whole tree becomes
    per-literal marks.
  - Cheesecake's C++ needs a manual migration before any re-vendor
    (see below).

## Assessment

Findings come first in each subsection. Opinion is marked.

### Project goals

**Findings:**
- `config` serves introspection and live coding: a static, host-visible
  declaration that hosts read without running anything.
- `tunable` serves goal-based manipulation: DM ranking and ranges.
- Both are identity at run time, so dataflow and effects are unaffected.
- Optional types: `config let t: int = 1.5` warns `type mismatch`;
  `let s: int = tunable(1.5)` does not, because the builtin's return type
  hides the literal.
- First-class state: the state-init lint's advice relies on `config let`.

**Opinion:** the DM hints are the part Petal should be distinctive at.
A declaration-level "exposed setting" marker is table stakes; Unity, Godot
and Unreal all have one. Removing it to save one contextual keyword works
against the introspection goal.

### Ease of writing

**Findings:**
- Today there are 2½ ways to mark a top-level number. Under E the guide
  gives one rule: "a setting is `config let`; a range or a preference is
  `tunable`".
- Under removal there is one spelling for numbers but none for the other 28%,
  until `tunable` takes any type.
- Misuse messages today: `tunable(true)`, `tunable("title")` and
  `tunable({x: 140})` all say "this argument is computed", which is wrong.
- The bare-statement form `tunable(gap, 0, 40)` marks a binding from a
  distance, so a reader of `let gap = 12` cannot see that it is a knob.

**Opinion:** someone coming from JS (dat.GUI, Leva) would guess a call that
takes any type. That argues for E's lint wording more than for widening
`tunable`.

### Bug-freedom

**Findings:**
- **Shadowing.** A user `fn tunable` silently demotes a knob: the DM score
  goes from 0.25 to 1.17, and only the `shadows-builtin` lint notices.
  `config` cannot be shadowed.
- **Name resolution.** `static_bindings` and `source_diff` run on source
  without natives. Under removal they would recognise `tunable` by name, and
  disagree with DM v2 whenever the name is shadowed.
- **Named arguments.** They make a binding non-static to hosts while
  `show-static` says it is static. A Tuning panel would drop such a knob
  silently.
- **New mistakes under E:** none. The unwrap is limited to the initializer
  position of a `config let`.
- **Fixed under E:** the nested-record gap and the `--config-strict` gap.

**Opinion:** removal trades a syntactic fact for a name-resolution fact in
five readers. That is the classic route to action at a distance.

### Performance

**Findings (debug build, `petal bench` and timing):**
- A top-level `let B = tunable(4.0)` read in a function costs the same as a
  `config let`: one instruction, 0.181 µs against 0.180 µs. It is a static
  value.
- In a body, `x * tunable(4.0)` costs about 45% more per call than a bare
  literal (0.307 µs against 0.212 µs). A 300k-iteration loop took 0.38 s
  against 0.22 s.
- Top-level knobs are therefore unaffected either way. E adds no run-time
  work. The use-site cost belongs to the hint and is not part of this
  question; the hints proposal already notes the `BuiltinCall` to `Move`
  lowering as a follow-up.

### Live coding and hot reload

**Findings:**
- Today `config let X = -4.0` to `4.0` is a value patch, and so is
  `config let X = tunable(-4.0)`. `let X = tunable(-4.0)` to `4.0`
  recompiles, and so does a bound crossing zero.
- Adding or removing `config`, or wrapping in `tunable`, recompiles with
  state kept. The reloaded program equals a fresh compile in every case
  tested.
- Reload reports carry `config: false` for a `tunable` edit, so hosts
  filtering on it (`natives_scene.cpp:328`) would stop seeing changes under
  removal.
- Under E, nothing changes for reload.

### Introspection, debugging and program manipulation

**Findings:**
- `explain` shows `tunable` as one more node and stops at the call span, not
  the literal: minor noise.
- `literal_edit` addresses a wrapped value as `X[0]`, so host paths would
  change shape under removal (`X` gives NeedsReload).
- DM v2 already treats config and tunable as one for labels and config-only
  subsets (`api.rs:687`). Only the weight differs.
- Goal-based editing and source preservation are unaffected by either
  design.

### Maintainability of the implementation

**Findings:**
- **Removal (A)** touches:
  - the parser (−130 lines and the 6 errors, +1 "removed" error);
  - AST, CST and the compiler (the fold has to move onto `tunable` arguments
    by name);
  - `source_diff` (re-key the config-data walk, lines 37-125 and 640-1163);
  - `static_value` (unwrap, named arguments);
  - `set_config_value` path routing;
  - `static_classify`, `ir_equiv`, `apply_change`, DM v1 and v2, the CLI,
    MCP, LSP, tree-sitter and the lint;
  - the hints contract, widened to non-numbers;
  - about 259 test sites, 22 docs and 10 mirrors.

  The maintenance reviewer estimates 1.5-2.5k changed lines.
- **Deprecation (B)** is all of that plus a dual-recognition window and the
  lint fix: the most total work.
- **E** is roughly 100-300 lines plus tests:
  - unwrap and range in `static_value`, plus the bridge JSON field;
  - path routing in `set_config_value`;
  - propagate `is_config` along data positions;
  - the ConfigStrict trigger;
  - two lint messages.

  It removes no special case and adds one ("a config initializer that is a
  `tunable` call is unwrapped"), confined to the readers.

### Domains

| Domain | Effect of removal | Effect of E |
|---|---|---|
| Graphics / creative coding | Slight help (ranges at the knob) | Same help, via `config let x = tunable(…)` |
| Games | **Gets in the way.** Cheesecake: Tuning screen, `--set`, profiles, effects panel; 1,326 uses, 28% non-numeric | Helps: ranges for the Tuning screen, and record knobs rank correctly |
| Productivity / UI apps | Neutral (no uses) | Neutral |
| Dashboards / data | Neutral (no uses) | Neutral |
| Web (`petal-web-canvas`, `petal-web-html`) | Neutral | Neutral |
| Editor scripting / `.ptl` config files | Neutral: goal-based editing does not use the keyword. Garden's IDE hint toggle already writes `tunable(…)` (`garden-app/src/dm_hints.rs`) | Neutral |
| Embedding hosts (Rust, C/C++) | **Gets in the way.** A stable boolean becomes `Call{tunable}` to decode, with `X[0]` paths | Additive `range` field |

### Compatibility and migration

**Findings:**
- **Removal breaks:**
  - all 1,500 `.ptl` sites;
  - Cheesecake's C++ silently. After a re-vendor that rejects `config`, the
    migrated games have no `config` text: `config_overrides.cpp` matches
    nothing, every `--set` and profile is a no-op, and the Tuning screen shows
    "No `config let` values".
- **Ordering.** Even under B, Cheesecake's scanner and panel must accept both
  forms *before* its vendored Petal warns. The scanner also replaces the
  whole initializer, so it would drop a `tunable` range.
- **Vendored copies:**
  - `~/cheesecake/third_party/petal` (`VENDORED.md`) carries all the
    breakage;
  - `~/worlds-fair/vendor/petal` (`node tools/vendor-petal.ts`) has no
    `.ptl` uses and only needs re-vendoring;
  - petal-lang.org needs `pnpm build:sdk` and its 10 mirrored doc pages
    updated.
- **E breaks nothing.** The bridge field is additive.
- **Future direction.** Removal does not strictly close one off, but
  moses-debrief's asks (choice and doc metadata, `--set` on modules,
  `config_overridden`) all want a declaration-level marker. E keeps that
  marker and adds range metadata now.

### Prior art

From the reviewers' knowledge, not verified against sources here:
- Unity: `[SerializeField]` exposes a field; `[Range]` is a separate widget
  attribute.
- Unreal: `UPROPERTY(EditAnywhere, meta=(ClampMin, UIMin))` separates
  exposure from range, and later also separated a soft UI range from a hard
  clamp.
- Godot 4: `@export` plus a growing family (`@export_range`, `_enum`,
  `_flags`, …). There is no range without export.
- Houdini and TouchDesigner: parameters are declared with a soft or strict
  range.
- Sketch-n-Sketch: per-literal ranges and freeze, numbers only and
  expression-level, which is the analogue of `tunable` and `rigid`.
- Processing Tweak Mode: every literal tweakable, with no control.
- Dear ImGui: call-site sliders, with no declaration and no persistence.

Every system with a host UI separates "exposed" from "range" and attaches the
range to the exposed declaration, which is E's shape. Two things went wrong
for them: soft and hard ranges were conflated (Unreal), and widget hints
proliferated (Godot).

### The case for removal (strongest counter-argument)

All three reviewers recorded the same argument, and it stands:
- Two marks that both mean "tunable" are real duplication.
- The 0.5 and 0.25 weight tiers and `ConfigStrict` are distinctions no user
  will understand.
- DM v2 already treats the two as one (`api.rs:687`), so users cannot predict
  which one a host honours.
- The migration is mostly mechanical. It lands almost entirely in repos the
  same author owns, so this is the cheapest moment it will ever be.
- E leaves the overlap in place permanently.

Godot shows that one marker carrying its range can work. That marker is still
a declaration, though, which is the opposite direction from removing `config`
in favour of an expression-level call. E answers the duplication by giving
each mark one job. Open question 2 can remove the weight distinction.

## Alternatives considered

- **A. Full removal.** Rejected. It loses non-numeric knobs (28%), the
  unshadowable static flag, the sign fold and live setting across zero, and
  stable host paths. It breaks 1,500 sites and Cheesecake's host features
  silently, and costs 1.5-2.5k lines.
- **B. Deprecate with a lint autofix** (`config let x = 30` to
  `let x = tunable(30)`). Rejected for the same end state. The fix is only
  correct for non-negative numeric scalars (61% of Cheesecake), needs
  host-side changes before it can ship, and recompiles each file once on a
  running game.
- **C. `config let x = v` as sugar for `let x = tunable(v)`.** Not adopted as
  stated, because the identity is false for non-numeric values. The narrow
  piece that holds is E's composition: `config let x = tunable(v, …)` already
  compiles, folds signs and patches live.
- **D. `tunable` canonical for numbers only, `config let` for the rest.**
  Rejected. It splits one concept by value type, so `rivet/config.ptl` would
  mix spellings line by line, and it keeps both mechanisms anyway.
- **Do nothing.** This leaves the two bugs and gives sliders no ranges. E is
  barely bigger.
- **Range syntax on the declaration** (`config let x = 40 range 10..80`).
  Considered in the hints proposal and rejected there, because it needs new
  syntax and does not reach function bodies or call sites. E gets the same
  result from an existing builtin.

## Implementation sketch

In this order, each step with its tests:

1. **Nested `is_config`.**
   - `compiler/stmt.rs`: mark every literal term in config data positions,
     reusing the `config_folded_numbers` walk.
   - Test: `--allow config` on `config let BOX = {x: 140}` proposes the 140,
     tagged `config`, with weight 0.5.
   - Update the DM v2 study golden if its rankings move.
2. **ConfigStrict trigger** (`dm_v2/api.rs:564`): also when any
   `Hint::Tunable` exists. Test: a `tunable`-only program under
   `--config-strict` pins the other bindings.
3. **`static_bindings` unwrap and `range`** (`static_value.rs`), including
   named arguments in this position.
   - Tests: positional and named forms, a negative literal, and a shadowing
     `fn tunable` in the file (still unwrapped, since `config` already marks
     the knob).
   - Also: `show-static --json` gains `range`.
4. **C bridge**: the `"range"` field in `pb_source_bindings_json`, documented
   in `docs/usage/embedding-c.md`. Add a test in `bridge_tests.cpp`.
5. **`set_config_value` path routing** for a wrapped config binding. Test:
   `SPEED` on `config let SPEED = tunable(4.0, 0, 10)` patches 4.0, and
   across zero too.
6. **Lint wording** (`dm-hint-not-literal` for non-numeric literals) and the
   new `config-not-data` warning.
7. **Docs**:
   - `docs/syntax/config-let.md`: a new section "Giving a setting a range",
     and the table of `let` / `config let` / `const` / `state`;
   - `docs/Builtins.md` hints section;
   - `docs/writing-petal-guide.md` and `docs/language-guide.md`: the
     division-of-labour rule;
   - then sync petal-lang.org.
8. **Cheesecake (separate repo, optional)**:
   - the Tuning screen reads `range` for slider bounds;
   - `config_overrides.cpp` replaces only the first argument when the
     initializer is `tunable(…)`, so `--set` keeps the range;
   - re-vendor.

Steps 1 and 2 are bug fixes that stand alone. Steps 3 to 5 are the feature.
No parser, IR or bytecode changes, so the differential oracle, Kani proofs
and golden IR files are unaffected apart from the `is_config` bits that step 1
adds to nested terms (`ir_equiv` compares them).

## Open questions

Decisions taken at implementation (24 October 2026). The owner accepted E
(question 1). Questions 2 to 4 were not decided, so the implementation takes
the conservative default for each, and any of them can still be changed:

- **2: merged into one knob tier** (reversed by the owner on 24 October
  2026; the implementation first kept the two tiers, 0.25 for `tunable` and
  0.5 for `config let`, with 0.25 for a literal marked both ways). A literal
  marked by `config let`, by `tunable` or by both now weighs 0.5
  (`rank::KNOB_WEIGHT`, `Param::is_knob`); `tunable` keeps its bounds. The
  selector `config` gains the alias `knob`, and `--config-strict`,
  `config_only` and the `[config]` tag mean "a knob". The DM v2 study was
  re-run for both candidate values:

  | Weights | Ladder ok | Top verified | Median runs | Total runs | External verified | External runs |
  |---|---:|---:|---:|---:|---:|---:|
  | before (0.25 / 0.5) | 91/105 | 99 | 20 | 2,274 | 240/258 | 16,535 |
  | one tier, 0.25 | 91/105 | 99 | 21 | 2,279 | 240/258 | 16,535 |
  | one tier, 0.5 | 91/105 | 99 | 20 | 2,274 | 240/258 | 16,535 |

  Neither changed an outcome or a top edit. 0.25 cost five runs on
  `a01-rand`; 0.5 changed no run count, so it was chosen. `l07b` did not in
  fact need `tunable` to be cheaper: `shrink` wins on the size of the change.
  See the DM v2 plan, §9 "One knob tier".
- **3: no clamping.** `set_config_value` and `pb_vm_set_config` accept a
  value outside the declared range; the range is reported (`range`) and the
  docs say that a host wanting a hard limit clamps. This keeps the soft (UI)
  and hard (clamp) ranges separate, as Unreal learned to.
- **4: no new metadata hint.** Choice and doc metadata stay out of scope;
  `StaticBinding.comment` remains the place for prose.

The questions as they were put:

1. **Owner:** accept E, or prefer removal (A/B) despite the costs listed? The
   strongest argument for removal is above.
2. **Owner:** merge the DM weights of `config let` (0.5) and `tunable` (0.25)
   into one tier? (Answered: yes, at 0.5; see above.) Then `config let x = tunable(v)` without bounds would
   change nothing for DM. The cost is re-running the DM v2 study, whose
   rankings use both tiers (l07b relies on `tunable` being tried first).
3. **Owner:** should `set_config_value` and `pb_vm_set_config` clamp to a
   declared range, or leave clamping to the host? DM treats bounds as box
   constraints, and Unreal's lesson is to keep soft (UI) and hard (clamp)
   ranges separate.
4. **Owner, later:** the choice and doc metadata moses asked for (strings
   with a fixed set of values). Should it be a sibling hint (for example
   `choice("title", ["title", "select", "play"])`) read under `config` the
   same way, or comment conventions (the binding's leading comment is
   already in `StaticBinding.comment`)? Out of scope here, but E is meant to
   leave room for it.
5. **Cheesecake maintainer:** whether and when to adopt the `range` field in
   the Tuning screen and the `tunable`-aware `--set` scanner.

## Implementation notes (24 October 2026)

What was built, in the order of the sketch:

1. **Nested `is_config`.** `source_diff::config_data_literals` walks a
   `config let` initializer's data positions (the same rule as the sign
   fold, which now uses it), and the compiler marks the term of every literal
   it finds `is_config` (`Compiler::config_literals`). So the `140` in
   `config let BOX = {x: 140}` gets weight 0.5, matches `--allow config`,
   Garden's `:DragAllow config` and `ParamSelector::ConfigOnly`, and is kept
   by ConfigStrict. Test: `dm_v2_solve::config_record_and_list_literals_are_config`.
2. **ConfigStrict** fires when any term is `is_config` *or* `read_hints`
   finds a `Hint::Tunable` mark (`dm_v2/api.rs`). The pin reason now reads
   "the program declares `config let` or `tunable` knobs". Test:
   `dm_v2_solve::config_strict_counts_tunable_knobs`.
3. **`static_bindings`.** `static_value::tunable_args` sorts a `tunable`
   call's arguments (positional or named `value`/`min`/`max`/`step`, one to
   four, each once); `config_value_expr` is the value position of a
   binding. Under `config` only, `value` and `text` are the inner value's and
   the new `StaticBinding.range: Option<StaticRange>` holds the bounds
   written as number literals (`None` when no bound is written; a bound that
   is not a literal is left out). A file's own `fn tunable` does not stop the
   unwrap. `petal show-static --json` gains the same `range` on a
   `config let` declaration.
4. **C bridge.** `pb_source_bindings_json` adds
   `"range": {"min":…, "max":…, "step":…}` (each key only when written),
   documented in `petal_bridge.h` and docs/usage/embedding-c.md. Tests:
   `source::tests::a_tunable_config_reports_its_value_and_range` (cargo) and
   `a_tunable_config_reports_its_range_and_sets_by_name` (bridge_tests.cpp).
5. **Path routing.** The unwrap lives where a path starts, so every path
   reader agrees: `literal_edit` (`set_path` and the other primitives, hence
   `set_config_value`, `pb_vm_set_config` and `pb_source_set_value`) and
   `rewrite::find_binding_path` (`pb_source_expr`, `pb_source_set`). `SPEED`
   sets the 4.0 in `config let SPEED = tunable(4.0, 1, 10)`, live, across
   zero, and unclamped. Test: `hot_reload::a_tunable_config_is_set_by_name_without_a_recompile`.
6. **Lints.** `dm-hint-not-literal` on a string, bool, `nil`, colour, list or
   record says `` `tunable` marks a number; `true` is a bool. To expose a
   setting of any type, declare it with `config let` ``, in `petal lint` and
   in the warning DM v2 reads from the IR. The new report-only rule
   `config-not-data` reads `` a `config let` initializer is read by hosts
   without running the program; `level.SPAWN` is a field access, which needs
   evaluating, so hosts see no value `` (the reason phrase is
   `static_bindings`' own).

Deviations from the sketch:

- **Reload reports changed too.** The sketch said "nothing changes for
  reload". For the paths to agree, the source diff gives the value inside
  `config let X = tunable(v, …)` the path `X` (it was `X[0]`), and a bound
  no path. Editing the number or a bound is still a value patch.
- **`X[0]` no longer reaches `v`** on a wrapped `config let`. Nothing in the
  repos used that path (no `config let … tunable(` existed outside docs), and
  keeping both would have meant two names for one literal. A plain
  `let X = tunable(…)` keeps `X[0]`.
- **Every literal is marked, not only numbers**: strings and bools in a
  config record are `is_config` too (DM only moves numbers, so this shows
  only in `show-ir --json`). A `tunable`'s bounds under `config let` are
  marked as well, and stay unmovable as bounds.
- The proposal's docs list also named docs/writing-petal-guide.md and
  docs/language-guide.md; both got the one-line rule. The petal-lang.org
  mirror is not synced here.

## Notes for Cheesecake's maintainer

Nothing in Cheesecake breaks: the change is additive for a host that ignores
`range`, and no game declares `config let X = tunable(…)` today (`tunable(`
appears in none of Cheesecake's `.ptl` files). What changes after a
re-vendor, and what the engine could adopt:

**Re-vendoring.** Follow `third_party/petal/VENDORED.md`, "To update": `git
-C ~/petal archive <commit> …` into a temp dir, `rsync -a --delete --exclude
VENDORED.md`, re-apply the `garden/gpp/Cargo.toml` modification, update the
commit line, then run the listed checks (`cargo test --lib -- builtins::decls
heap::`, the bridge's `cargo test`, `tools/build.ts test`). Use a commit at or
after the one implementing this note.

**What a re-vendor changes by itself.**

- The Tuning screen (`engine/editor/src/tuning_panel.cpp`): a binding written
  `config let X = tunable(4.0, 0, 10)` arrives as a plain float `4.0` with the
  path `X`, so it draws as one number field. Before, it arrived as
  `Kind::Call "tunable"`, which `is_leaf` rejects (it is not `vec2`/`vec3`),
  so it showed as a `tunable(...)` header opening onto three numbers, value,
  min and max, and editing the second or third rewrote the range.
- `reloaded_values()` (`natives_scene.cpp:327`): an edit of that value is
  reported under the path `X` (was `X[0]`); an edit of a bound carries no
  path.
- More DM preference for record and list config literals (`POST.bloom` and
  friends), if the editor uses DM v2's ranking anywhere.

**Adopting `range` in the Tuning screen.** `File::reparse`
(`engine/editor/src/ptl_source.cpp:402`) reads the JSON keys one by one; add
`range` beside `config` (three optional doubles on `ptl::Binding`). In
`draw_value`, for the top-level float or int of a binding with a range, use
`min`/`max` for the slider's ends (or to clamp what `ui.number` produces) and
`step` for its quantum instead of `quantum_of`. The range applies to the
binding's own value only, not to fields of a record. Clamping is the host's
call: Petal does not clamp in `set_config` or `set_value`.

**The `--set` / `profiles.json` scanner (`engine/script/src/config_overrides.cpp`).**
Checked against the code: it does not mis-parse `config let X = tunable(…)`.
`expression_end` balances brackets, so the initializer it finds is the whole
`tunable(4.0, 0, 10)` (a trailing comment is kept), and `--set X=7` yields
`config let X = 7`. The override takes effect, but it **drops the wrapper**:
for that run the knob has no range in the Tuning screen and no `tunable`
preference for DM. To keep it, when the replaced text starts with `tunable(`
(after `skip_blank`), replace only the first argument: from just past the
`(` to the first top-level `,` or the matching `)`, using the same
string/comment/bracket skipping `expression_end` does. A named first argument
(`tunable(value: 4.0, …)`) is legal; skipping an identifier followed by `:`
covers it. Two tests beside the existing overrides tests in
`tests/script_host_tests.cpp`: `--set X=7` on
`config let X = tunable(4.0, 0, 10)   // note` gives
`config let X = tunable(7, 0, 10)   // note`, and on a multi-line call keeps
the following lines.
