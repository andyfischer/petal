# Proposal: remove the `config` keyword in favour of `tunable(...)`

Status: Phases 1 and 2 implemented 6 November 2026 (see
[Phase 1 notes](#implementation-notes-phase-1-6-november-2026) and
[Phase 2 notes](#implementation-notes-phase-2-6-november-2026));
phases 3 to 5 proposed. This revisits
[config-let-vs-tunable.md](config-let-vs-tunable.md), where removal was
rejected on 24 October 2026 and design E (`config let X = tunable(…)`
composing) was built. The owner now wants removal. This plan answers each
blocker that proposal raised and lays out the steps to reach parity first and
remove second.

## Summary

Deprecate `config let` and then remove it. A **setting** becomes a top-level
`let` (or `pub let`) whose whole initializer is a `tunable(...)` call:

```petal
pub let SPEED = tunable(4.0, 0, 10)                     // a number with a range
pub let START = tunable("title", options: ["title", "select", "play"])
pub let GOD   = tunable(false)
pub let POST  = tunable({exposure: 1.2, bloom: 0.32})   // every literal inside is a knob
```

`tunable` is widened from "numbers only" to **any static data**. It gains one
named argument, `options:`, which carries the choice-list metadata Moses asked
for. `min`, `max` and `step` stay number-only. A `tunable(...)` anywhere else
(in a function body, at a call site, inside a record field) keeps its current
meaning as a DM hint and is not a host-visible setting. `tunable` and `rigid`
become reserved names, so the function form gets the keyword's immunity to
shadowing.

**Recommendation: adopt, in five phases.** Phase 1 makes `tunable` reach full
parity while `config let` still works, and changes nothing that hosts already
read. Phase 2 adds the C bridge's `tunable` section with deprecated `config`
and `range` mirrors. Phases 3 to 5 handle migration, deprecation and removal.
No phase breaks a vendored host before that host has been updated.

What changed since 24 October, and why removal is now cheaper than that
proposal measured:

1. **The sign fold is gone.** Commit 7de14fa2 parses `-0.5` as one literal
   everywhere (`source_diff.rs:37`, test `:1582`). So `let X = tunable(-0.5)`
   flipped to `0.5` is already a value patch, and one of the three "removal
   loses this" rows no longer applies.
2. **The DM gallery already works without `config`.** I rewrote the four
   petal-lang.org samples that use it (`orbits`, `flower`, `fractal-tree`,
   `quilt`) to `let X = tunable(…)` and ran `scripts/check-dm-samples.mjs`
   against today's vendored WASM. The verbose output (every shape, every
   gesture, every chosen edit) was **byte-identical** to the `config`
   version. The merged knob tier (0.5, `rank::KNOB_WEIGHT`) did that. Only
   the gallery's UI code and copy need to change (see
   [§DM samples](#the-dm-samples-on-petal-langorg)).
3. **Every remaining gap is a reader gated on `is_config`**, not a language
   property. The unwrap, `range`, path `X` and host listing all sit behind
   `is_config.then(...)` in `static_value.rs:703-725`, `literal_edit.rs:659-671`,
   `rewrite.rs:135-142` and `source_diff.rs:1033-1040`. Changing that gate
   to "the top-level initializer is a core `tunable` call" is mechanical.

The strongest argument against is unchanged. Cheesecake has 1,337
`config let` lines in 163 game files, and its `--set` scanner, Tuning
screen and effects panel all key on the keyword. Removal is a coordinated
two-repo migration. It is safe only if the ordering below holds.

## What `config let` does today, and what replaces each part

Read at HEAD 4b148e45. Rows marked **gap** are what Phase 1 has to build.

| `config let` behaviour | Where | `let X = tunable(v)` today | Plan |
|---|---|---|---|
| Any data type | `compiler/stmt.rs:55-77` | Runs, but marks only a numeric literal. Strings, bools, records: `dm-hint-not-literal` (`dm/hints.rs:199-232`) | **gap**: widen (§1) |
| Every literal in a record, list or `rgb(…)` is a knob | `source_diff::config_data_literals` (`:62`), `compiler/expr.rs:12-17` | Only the single literal the argument resolves to | **gap**: reuse the same walk on the `tunable` argument |
| Host listing: `StaticBinding.is_config`, bridge `"config": true` | `static_value.rs:612`, `petal-c-bridge/src/source.rs:194` | `is_config=false`, value is `Call{tunable}` (`static_value.rs:1683`) | **gap**: §3 |
| Unwrap: value, text and path `X` name `v`; `range` reported | `static_value.rs:703-725`, `literal_edit.rs:659-671`, `rewrite.rs:135-142` | Only under `config`. A plain `tunable` gets `X[0]` and no range | **gap**: drop the `is_config` gate |
| `set_config_value` / `pb_vm_set_config` by `X` | `env/reload.rs:588` (works on any top-level let) | `X` replaces the whole call: `NeedsReload`. Only `X[0]` works | **gap**: same unwrap |
| Reload report `values[].config`, construct kind `"config let"` | `source_diff.rs:600, 953`; `vm.rs:2267, 2283` | `config: false`, path `X[0]` (test `source_diff.rs:1618`) | **gap**: §4 |
| Sign across zero is a value patch | was the sign fold | **Already parity** (7de14fa2) | none |
| DM v2 knob weight, `ConfigOnly`, `--config-strict`, `knob` selector | `dm/params.rs:114`, `rank.rs:25`, `api/mod.rs:318-342` | **Already parity**: `is_knob = config \|\| Tunable` | rename only |
| DM v1 policy (`apply_policy`, `source_declares_knobs`) | `direct_manipulation.rs:218`, `:404-489` | `is_config` only. **`tunable` does not trigger it** | **gap**: key on `is_knob` |
| Cannot be shadowed | keyword | A user `fn tunable` hides the hint. `tunable_args` and `provenance.rs:269` still go by name | **gap**: reserve the names (§2) |
| Top level only; `config var/state/fn/const` are errors | `parse.rs:698-718` | `tunable` is legal anywhere | Setting = top-level `let` only. A lint covers `var X = tunable` (§5) |
| `show-static` kind `"config let"`, `DeclKind::ConfigLet`, `ir_equiv`, `apply_change` refusals | `static_classify/mod.rs:231, 1325`, `ir_equiv.rs:518`, `apply_change/convert_to_var.rs:82` | Plain `let` | Recognise the tunable form as `DeclKind::Tunable` |
| Typed: `config let t: int = 1.5` warns | ordinary typed let | `let s: int = tunable(1.5)`: no warning | **gap**: type `tunable` as `(T, …) -> T` |
| `config-not-data` lint | `lint/config_data.rs` | none | Port to the tunable form |
| `dm-hint-state-init` says "use `config let`" | `lint/dm_hints.rs:117-121` | n/a | Reword |

The only behaviour the plan deliberately drops: `config let X = level.SPAWN`
(a computed initializer, 4 sites). Under `tunable(level.SPAWN)` it is still
listed, with a `config-not-data`-style warning that hosts see no value. That
is the same as today.

## Design

### 1. `tunable` takes any static data

```
tunable(value, min?: num, max?: num, step?: num, options?: list)
```

- **Run time:** unchanged. It is the identity on `value`, with a `CONST` effect
  row, and the bounds and options are evaluated and ignored. Petal does not
  clamp (decision 3 of the earlier proposal stands).
- **DM marks:** the hint walks `value`'s data positions with
  `config_data_literals` (literals, list items, record fields, call arguments
  such as `rgb(…)`) and marks every **numeric** literal it finds as a knob. A
  bare string or bool marks nothing for DM, which only moves numbers, but it
  is still a valid setting.
- **Bounds on non-numbers:** `tunable("x", 0, 1)` gets the lint
  `tunable-bounds-not-number`, which reads "`min`/`max`/`step` describe a
  number; `"x"` is a string". It replaces today's `dm-hint-not-literal`
  message for that case. `dm-hint-not-literal` stays for a computed value
  (`tunable(a + b)`).
- **`options:`** is a list of static values. The lint
  `tunable-value-not-in-options` fires when `value` is not one of them. Hosts
  receive the list (§3) and can draw a dropdown. It is legal on any type
  (`options: [8, 16, 32]` is fine).
- **Positional arguments 2 to 4 stay `min`, `max`, `step`**, so every
  existing `tunable` call keeps its meaning.

### 2. `tunable` and `rigid` are reserved

- Declaring `fn tunable`, `let tunable`, a parameter named `tunable`, or an
  import alias of that name is a compile error: "`tunable` is a builtin hint
  and cannot be redefined".
- `Env::register_native` refuses both names with an error. Today it
  silently disables the hint (`dm/hints.rs:130`, `dm-hint-disabled`).
- **Corpus cost: zero.** No `.ptl` in petal, garden, cheesecake, worlds-fair,
  petal-creative or petal-lang.org defines either name. No host in
  cheesecake/engine, worlds-fair or petal-creative registers a native
  called `tunable` or `rigid`. Checked with grep, 5 November 2026.
- This makes recognition by name sound. The readers that already go by name
  (`static_value::tunable_args`, `provenance.rs:269`, `lint/dm_hints.rs:65`)
  stop being a soundness hole, and `lint/dm_hints.rs:65`'s "skip the file if
  it binds `tunable`" escape is deleted.

### 3. What a setting is, to hosts

A **setting** is a top-level `let` or `pub let` whose whole initializer is a
core `tunable(...)` call. For it:

- `StaticBinding` gains `tunable: Option<TunableMeta>`, where `TunableMeta` is
  `{ min, max, step: Option<f64>, options: Option<Vec<StaticValue>> }`.
  `value` and `text` are the inner `v`, and `range` folds into `TunableMeta`.
- `StaticBinding.is_config` stays for one release as `keyword ||
  tunable.is_some()`, so every existing reader (DM `ConfigOnly`, Cheesecake's
  panels through the bridge, `fn_introspect`) works unchanged. It is removed
  in Phase 5.
- Path `X` names `v` in every path reader (`literal_edit`, `rewrite`,
  `source_diff`), the same as `config let X = tunable(…)` today. **`X[0]`
  stops reaching `v`** on a top-level tunable let. It does reach it today on a
  plain `let X = tunable(…)`. That is the one observable change for current
  `tunable` users: 6 top-level sites, all in petal, none read by `X[0]`.
  worlds-fair's `wf-tuning` already rejects a call right-hand side
  (`sim/wf-tuning/src/lib.rs:292`), so it is unaffected.
- `tunable(...)` nested inside a setting's record is fine and means "this
  field has a range" (`tunable({bloom: tunable(0.32, 0, 1)})`). Listing
  per-field metadata to hosts is open question 3.
- `tunable(...)` in a non-setting position stays a DM hint only. This is the
  same as today.

### 4. The C bridge's `tunable` section

The ABI is untouched: `pb_abi_check` checks struct sizes only, and every
change below is a JSON key or a new function. That is how the bridge has
always grown (see the "Bridge additions the engine does not call yet" list
in Cheesecake's `VENDORED.md`).

**`pb_source_bindings_json`** (`source.rs:182-222`): each setting gains a
`tunable` object.

```json
{"name": "START", "exported": true, "line": 5,
 "text": "\"title\"", "value": "title",
 "tunable": {"options": ["title", "select", "play"]},
 "config": true}
{"name": "SPEED", "exported": true, "line": 9, "text": "4.0", "value": 4.0,
 "tunable": {"min": 0, "max": 10},
 "config": true, "range": {"min": 0, "max": 10}}
```

- `"tunable"` is present exactly when the binding is a setting. It is `{}`
  when no metadata is written. Each key appears only when written.
- `"config"` and `"range"` are **deprecated mirrors**. `config` is
  `tunable` present (or the keyword), and `range` is the numeric part. They
  are emitted until Phase 5, so Cheesecake's `ptl_source.cpp:402`,
  `tuning_panel.cpp:85, 337` and `effects_panel.cpp:338` keep working with no
  C++ change after a re-vendor.

**Reload report** (`pb_vm_last_reload_json`, `vm.rs:2242-2330`):
- `values[]` gains `"tunable": bool`, with `"config"` mirrored.
- `constructs[].kind` gains `"tunable let"`. `"config let"` stays while the
  keyword parses.

**Functions:**
- `pb_vm_set_tunable(vm, file, path, value_json)` is the new name for
  `pb_vm_set_config`, which stays as an alias until Phase 5. The C++ wrapper
  becomes `set_tunable`, and `set_config` is marked `[[deprecated]]`.
- `pb_vm_propose_edit`: `"allow": ["knob"]` is canonical and `"config"` is an
  alias. `"knob_strict"` is accepted beside `"config_strict"`.
- **New:** `pb_source_set_setting(file, name, value_json)` sets the value of
  a setting by name: it replaces only `v` and keeps the bounds, `options` and
  comment. It is `pb_source_set_value(path = name)` with a check that the
  binding is a setting. It gives Cheesecake's `--set` a structural
  replacement for its text scanner (§Migration, Cheesecake).

**Header and docs:** a "Tunables" section in `include/petal_bridge.h`, next
to the bindings docs at `:1279-1340`. It groups the JSON shape, `set_tunable`,
`set_setting` and the deprecation notes. A matching "Tunables and settings"
section goes in `docs/usage/embedding-c.md` (today `:817-936`), replacing the
`config let` wording. Tests: extend `source::tests` and `bridge_tests.cpp:2092-2150`
with a string with options, a bool, a record, and the mirrors.

### 5. Errors and lints at the end state

- `config let X = v` gives a parse error: ``config let` was removed. Write
  `let X = tunable(v)` (`petal lint --fix` rewrites it)``. The `config`
  ident stays an ordinary name (`let config = …` works, as today).
- `var X = tunable(v)` at the top level gets the lint `tunable-on-var`: "a
  `var` is not a setting; hosts list only `let X = tunable(…)`". This
  replaces the parse error for `config var`.
- `state x = tunable(v)` keeps `dm-hint-state-init`, reworded: "declare the
  start value as a setting, `let START = tunable(v)`, and use `state x =
  START`".

## Before and after

All "before" snippets run today. The DM samples are checked by
`check-dm-samples.mjs` (above). The Cheesecake extracts were run in the
[earlier proposal](config-let-vs-tunable.md#before-and-after). The "after"
snippets are proposed. The ones marked *runs today* also run on current
Petal; they only lack the reader changes.

### 1. The common case: numeric knobs (`examples/games/asteroids/app.ptl:50-52`)

```petal
// before
config let start_lives   = 3
config let turn_rate     = 4.4      // rad/s
```
```petal
// after (runs today)
let start_lives   = tunable(3)
let turn_rate     = tunable(4.4)    // rad/s
```

Each line gains a wrapper. The lint fix re-pads the aligned comments,
because the line grows by 3 characters net. A reader now sees a call that
does nothing at run time, which is also true of `rigid`, and the guide
already teaches both. One concept fewer to learn: there is no "setting
versus hint" split.

### 2. Non-numeric, choice-like settings (`~/cheesecake/games/rivet/config.ptl:5-10`)

```petal
// before
pub config let START = "title"        // "title" | "select" | "play": where a run begins
pub config let GOD = false            // take no damage (screenshots, the level tour)
```
```petal
// after (proposed: `options:` and the non-numeric marks are new)
pub let START = tunable("title", options: ["title", "select", "play"])  // where a run begins
pub let GOD   = tunable(false)        // take no damage (screenshots, the level tour)
```

This is the case the earlier proposal said `tunable` could not express. With
`options:`, the choice list moves out of a comment that nothing checks and
into data that the lint and the Tuning screen can use. Ten of the 86 string
settings in Cheesecake carry such a comment today, and the migration does not
convert them automatically (a comment is not a spec). Newly possible mistake:
`options:` drifting from the code that switches on the string. The lint only
checks that the default is a member.

### 3. A record setting (`~/cheesecake/games/rivet/game.ptl:32-40`)

```petal ignore
// before
pub config let POST = {
  exposure: 1.2, bloom: 0.32, ...
  effects: [{effect: "chromatic_aberration", amount: 0.12}, ...],
}
```
```petal ignore
// after (proposed)
pub let POST = tunable({
  exposure: 1.2, bloom: tunable(0.32, 0, 1), ...
  effects: [{effect: "chromatic_aberration", amount: 0.12}, ...],
})
```

The binding-level flag that `effects_panel.cpp:338` needs survives as
`"tunable"`, mirrored as `"config"`. Every number inside is still a knob. The
cost is a closing `})`, and 51 multi-line initializers need it (49 in
Cheesecake). An inner `tunable` can add a range to one field.

### 4. Ranges and DM hints (`~/biz/petal-lang.org/frontend/public/garden/samples/flower.ptl:12, 17`)

```petal
// before
config let ring = tunable(78, 30, 140)
config let blend = tunable(0.3, 0, 1)
```
```petal
// after (runs today; DM output measured identical)
let ring = tunable(78, 30, 140)
let blend = tunable(0.3, 0, 1)
```

This is pure deletion, and the strongest case for the change: the earlier
design made a writer type two markers to say one thing.

### 5. Interaction with `state` and hot reload (`examples/games/asteroids/app.ptl:50, 80`)

```petal
// before
config let start_lives = 3
state var lives = start_lives
```
```petal
// after (runs today)
let start_lives = tunable(3)
state var lives = start_lives
```

There is no behaviour difference. Editing `3` is a value patch, and `lives`
keeps its slot. The migration edit itself (`config let` to `let … tunable`)
is a structural change (`source_diff.rs:728`). A running game recompiles
each migrated file once, with state kept, as measured in the earlier
proposal. Migrate with games stopped, or accept one recompile.

### 6. Where the old form reads better

```petal
// before (examples/games/2048/2048.ptl:188)
config let pattern = ["left", "up", "right", "up", "down", "left", "up", "right"]
// after
let pattern = tunable(["left", "up", "right", "up", "down", "left", "up", "right"])
```

Wrapping long data in a call adds nesting and a trailing `)` that is easy to
lose. A declaration modifier is visually lighter for big records and lists.
This is a real, if small, readability cost, paid on about 120 lines (records
and lists).

## The DM samples on petal-lang.org

The measured result: DM outcomes are identical. What still has to change:

| File | Change |
|---|---|
| `frontend/public/garden/samples/orbits.ptl:11` | `let sun_r = tunable(34, 10, 60)` |
| `…/flower.ptl:12, 17` | drop `config` (pair 4) |
| `…/quilt.ptl:17-19` | `let tile = tunable(52)`, `let gap = tunable(10)`, drop `config` on `base_hue` |
| `…/fractal-tree.ptl:6-7, 13` | `let spread = tunable(0.42)`. The comment "is a config knob" becomes "is tunable", and "Alt-click `spread` **twice**" becomes "**once**": Garden's cycle is free → tunable → rigid (`garden-script/src/dm_hints.rs:27-32`), and `spread` now starts at tunable |
| `frontend/src/lib/dm-samples.json` | the fractal-tree `style` ("recursion · config let") and its first two `tries` (the `config let` knob, "Alt-click twice") |
| `frontend/src/components/dm/DmStudio.tsx:178-224` | `findKnobs` drops the `config let` regex branch (`:187-188`, the `kind: "config"` heuristic range at `:207-220`). Every knob is then a `tunable` literal. `Knob.kind` loses `"config"`. `candidateState` (`:238`) shows `"knob"` for `c.config` until garden-web renames the field |
| `frontend/src/app/page.tsx:281` | the home-page snippet `config let spread = 0.42   // a knob` |
| `content/docs/syntax/config-let.md` and 9 other mirrored pages | re-sync from petal's docs after Phase 3 (`scripts/import-docs.ts`) |

Then `pnpm build:sdk`, which re-vendors garden-web, and
`node scripts/check-dm-samples.mjs`. Add a check to that script that fails
if any sample contains `config let`, so the gallery cannot regress.

## Assessment

### Project goals

**Findings:**
- **Fewer concepts.** One marker, legal everywhere, replaces a declaration
  modifier plus a hint that composed in a way the guide had to explain
  (`docs/syntax/config-let.md` spends a section on it).
- **DM and goal-based editing** are unchanged. The ranking is measured
  identical on the gallery.
- **Introspection gains one thing:** `options:` turns prose into data.
- **It works against "easy to read" in one place:** big records and lists in
  a call (pair 6).

**Opinion:** this fits Petal's design. Hints are values, and nothing about
running the program depends on them.

### Ease of writing

**Findings:**
- **A newcomer from JavaScript, Python or Lua** has seen neither form. A
  function call is more guessable than a contextual modifier, and it is
  already how `rigid` is written.
- **One way to do it at the end state,** after a transition during which
  there are two.
- **Error messages:** the removal error carries the rewrite and points to
  `--fix`. Misplacing `tunable` on a `var` is a lint, not a silent no-op.

### Bug-freedom

**Prevented:**
- Choice-list drift becomes visible (`tunable-value-not-in-options`).
- Shadowing becomes impossible for both hints.
- The DM v1 gap, where `tunable` did not trigger `apply_policy`, closes.
- `let s: int = tunable(1.5)` now warns, once `tunable` is typed as identity.

**Newly possible:**
- `tunable(...)` placed around part of an initializer (`let X = tunable(4) *
  2`) is a DM hint, not a setting, so hosts silently stop listing `X`. A lint
  `tunable-not-whole-initializer` on a top-level `let` covers it, and
  `show-static` shows the kind.

**Oracles:** no IR shape change beyond `is_config` bits. The differential
oracle, Kani proofs and golden IR keep holding. `ir_equiv` compares the new
marks.

### Performance

**Findings:**
- **Compile time:** one extra walk per top-level `tunable`. The same walk runs
  today for `config let`, so this is a wash.
- **Run time:** a top-level `tunable` is a native call per evaluation. In a
  sketch that re-runs its top level every frame, that is one call per knob
  per frame. A `config let` was a plain constant.
- **Possible fix:** the optimizer could lower a core `tunable(v, …)` to `v`
  in bytecode while keeping the marks in the IR. That is sound because the
  call is `CONST`, identity, and its name is now reserved.

**To measure:** `petal bench` on `examples/games/boids` (14 knobs) before and
after migrating its knobs. Lower the call only if the difference is visible.

### Live coding and hot reload

**Findings:**
- An edit inside `v` is a value patch, the same as today.
- Adding or removing the wrapper is structural (a recompile with state kept).
  The same is true of `let` ↔ `config let` today.
- Reload equals fresh compile: unchanged.

### Introspection and program manipulation

**Findings:**
- `literal_edit` and DM paths become `X` for every setting, matching
  `config`.
- `explain` shows one more call node, which already happens for every
  `config let X = tunable(…)`.
- Garden's hint toggle already writes `tunable(…)`, so the IDE's own edits
  produce the new form natively.

### Maintainability

**Findings:**
- **Net removal at the end:**
  - from the parser: `CONTEXTUAL_KEYWORDS` entry, two parse paths, six
    special errors, `starts_config_decl`;
  - `StmtKind::Let.is_config`, the CST two-ident rule;
  - `ConstructKind::ConfigLet`, the `let` ↔ `config let` rule;
  - the `is_config` gates in four readers;
  - `DeclKind::ConfigLet`;
  - tree-sitter (`grammar.js:155`, `highlights.scm:37`, regenerated files),
    vim syntax, the LSP keyword.
- **Added:**
  - name reservation;
  - the widened hint walk;
  - `TunableMeta`;
  - three lints;
  - the bridge section.
- **New invariant:** "a top-level `let` whose initializer is a core `tunable`
  call is a setting". It is enforceable only because the name is reserved.
- **Docs and tests:**
  - about 850 occurrences in 141 non-doc files (`hot_reload.rs` 59,
    `source_diff` 31 and so on). Most are mechanical through `--fix`;
  - about 30 docs, led by `syntax/config-let.md` (45 mentions), which is
    rewritten as `syntax/settings.md`;
  - the DM v2 plan;
  - `CLI.md`.

**Estimate:** about 1.5k changed lines in Petal, most of them test fixtures.

### Domains

| Domain | Effect |
|---|---|
| Graphics / creative coding | Helps: one marker, ranges in place. The gallery is measured unchanged |
| Games | Neutral to slightly positive after migration (`options:` for 86 string settings). The migration cost lands here |
| Productivity / UI, dashboards | Neutral (no uses) |
| Web (`petal-web-canvas`, `petal-web-html`) | Neutral. The gallery's `DmStudio` gets simpler |
| Editor scripting / Garden | Neutral. Garden's `init.ptl` uses no `config let`. 14 lines in Garden examples migrate |
| Embedding hosts | Additive bridge change, with mirrors during the transition. `wf-tuning` and petal-creative are unaffected |

### Compatibility and migration

**Corpus:** 1,549 `config let` lines in 217 files:
- Cheesecake 1,371 (games 1,337);
- petal/examples 131;
- garden 14;
- core tests 2;
- petal-lang.org 7 `.ptl` and 24 in docs.

worlds-fair and petal-creative have none.

**Mechanical rewrite** (`petal lint --fix`, rule `config-let-deprecated`,
modelled on `lint/export_to_pub.rs`):

| Initializer | Rewrite | Lines |
|---|---|---|
| already `tunable(…)` | drop `config` | 17 |
| number, bool, string, colour, `vec3`, `nil` | wrap in `tunable(…)` | about 1,400 |
| record / list, multi-line | wrap, with `tunable({` … `})` | 51 |
| computed (`level.SPAWN`) | wrap and keep the warning | 4 |

The fix re-pads trailing comments: 832 lines have one, and most are
column-aligned. `plan_pub_rewrite` only handles a keyword that shrinks the
line, so the padding logic is new.

**Ordering for Cheesecake (the only real hazard):**
1. Re-vendor at Phase 2. The mirrors keep the Tuning screen and effects
   panel working, and `config let` still parses.
2. Change `config_overrides.cpp:65-104` before migrating any game. Its text
   scanner matches `config let NAME =` only. Replace it with
   `pb_source_set_setting`, which also keeps bounds and options. That removes
   the "`--set` drops the range" bug the earlier proposal found. Update
   `script_host.cpp:457`'s message and the `main.cpp:122-128` help text.
3. `ptl_source.cpp:402` reads `"tunable"`. The Tuning screen uses `min`,
   `max` and `step` for sliders and `options` for a dropdown. Rename the UI
   strings (`tuning_panel.cpp:1-14, 202, 205, 347`) and update
   `ide_tests.cpp`.
4. Run `petal lint --fix` over `games/`, `tests/data` and the docs. Then
   `tools/build.ts test`, and spot-run the about 396 `--set` uses and 6
   `profiles.json` (the probe bundle and canopy, eggheist, moses, stride).
5. Only then does vendored Petal reach Phase 4 (warnings) and Phase 5
   (removal).

## Alternatives considered

- **Keep design E (status quo).** This leaves two markers for one idea. It is
  what the owner is now overruling.
- **A separate `setting(v)` for non-numeric data, `tunable` for numbers.**
  Rejected. It is design D from the earlier proposal under a new name: it
  splits one concept by value type, so `rivet/config.ptl` would mix
  spellings line by line.
- **Siblings per kind** (`choice(v, [...])`, `toggle(b)`, `color_knob(c)`).
  This is the Godot `@export_*` family, which the earlier proposal named as
  a failure mode, because widget hints proliferate. `options:` on `tunable`
  covers the one ask that exists (choices). A sibling can still be added
  later if a kind needs host semantics that a named argument cannot carry.
- **A declaration attribute** (`@tunable let X = …`). It would read well for
  big records (pair 6), but it adds new syntax to replace a keyword with
  syntax, and it does not reach call sites.
- **Remove `config` without widening `tunable`.** This loses the 419
  non-numeric settings and breaks Cheesecake's effects panel, which needs a
  record setting. It is not parity.

## Implementation plan

Each phase ends green on `make test`, `make test-c-bridge`, the DM v2 study
golden and `check-dm-samples.mjs`.

**Phase 1: parity in core** (additive; `config let` untouched)
1. Reserve `tunable` and `rigid`:
   - the resolver error;
   - `register_native` refusal;
   - delete `dm-hint-disabled` and the `lint/dm_hints.rs:65` escape.

   Tests: a user `fn tunable` is an error, and a host registration fails.
2. Type `tunable` as `(T, …) -> T` and `rigid` as `(T) -> T` in the
   checker. Test: `let s: int = tunable(1.5)` warns.
3. Widen the hint:
   - `dm/hints.rs` walks `value` with `config_data_literals` and marks
     numeric literals;
   - add `options:` to `core/packages/core/src/dm.ptl` and `tunable_args`;
   - add the lints `tunable-bounds-not-number`,
     `tunable-value-not-in-options`, `tunable-not-whole-initializer` and
     `tunable-on-var`;
   - port `config-not-data`.

   Test: `let BOX = tunable({x: 140})` ranks the `140` as a knob and
   matches `--allow knob`.
4. Settings in the readers:
   - replace the `is_config` gate with `is_setting(binding)` in
     `static_value.rs:703-725`, `literal_edit.rs:659-671`,
     `rewrite.rs:135-142`, `source_diff.rs:1033-1040` and `:953`;
   - add `TunableMeta`;
   - set `is_config = keyword || setting`;
   - add `DeclKind::Tunable` in `static_classify`, and `show-static` kind
     `"tunable"`.
5. DM v1 `apply_policy` keys on `is_knob`. Make `knob` the canonical
   selector name in the CLI (`--knob-strict`, with `--config-strict` as an
   alias), the MCP tools (`tools/petal-mcp.ts:251-280`), Garden
   (`panel_trace.rs:358-393`, `CandState::Config` → `Knob`) and garden-web
   (`lib.rs:256, 412, 425`, keeping a `config` field alias).
6. **The parity test:** a test that takes every `config let` in
   `examples/` and `core/tests`, rewrites it with the Phase 3 fixer, and
   asserts the same output on both forms:
   - `static_bindings` (value, text, path, metadata);
   - `set_config_value` by name, including across zero;
   - the reload report paths for an edit of each literal;
   - DM v2 rankings on the existing `dm_solve` fixtures.

   It is the acceptance gate for Phase 4.

**Phase 2: the C bridge `tunable` section** (§4):
- the JSON `tunable` object plus the mirrors;
- the reload report fields;
- `pb_vm_set_tunable` and `pb_source_set_setting`;
- the header section and the `embedding-c.md` section;
- tests in `source.rs` and `bridge_tests.cpp`.

Ship it, re-vendor into Cheesecake and worlds-fair (no behaviour change for
either), and drop Cheesecake's `6e124ec` hand patch at the same time.

**Phase 3: migration tooling and the Petal-owned corpus:**
- the `config-let-deprecated` fixer, report-only at first, with comment
  re-padding;
- run it over `examples/`, `garden/`, `core/tests`, Rust test fixtures and the
  tree-sitter corpus;
- rewrite `docs/syntax/config-let.md` as `docs/syntax/settings.md` and update
  the about 30 other docs, `Builtins.md` and `CLI.md`;
- petal-lang.org: the samples and UI from §DM samples, the docs re-sync,
  `pnpm build:sdk`, `check-dm-samples.mjs` plus its new `config let` guard;
- re-run the DM v2 study and confirm the golden is unchanged.

**Phase 4: deprecate** (only after Cheesecake steps 2-4 have landed):
- `config let` gets a warning, `config-let-deprecated`, which `--fix`
  applies;
- `CHANGELOG.md` entry;
- re-vendor Cheesecake.

**Phase 5: remove** (after a release with the warning):
- the parse error with the rewrite;
- remove the AST, CST, compiler and source_diff `config` paths, and
  `StaticBinding.is_config`;
- the bridge stops emitting `"config"` and `"range"`, and the
  `pb_vm_set_config` alias is removed;
- tree-sitter, vim and LSP updates;
- a final sweep of the ecosystem projects in `CLAUDE.local.md`.

## Open questions

1. **Owner: `options:` versus a sibling `choice(v, [...])`.** The plan
   recommends `options:` (one marker, no family of widget hints). Choose
   `choice` if you want the call to read as "this is an enum" at a glance.
2. **Owner: should the migration convert choice comments** (`// "a" | "b"`,
   10 strict and 2 loose sites) into `options:`? It can be done mechanically
   for the 10 strict ones, but it changes what the lint checks. The plan
   says leave them, and let the Cheesecake maintainer opt in per game.
3. **Owner: list nested tunables to hosts?** For example `POST.bloom` with
   its own range in `pb_source_bindings_json`. It is useful for the Tuning
   screen, and it adds a `fields` array to the JSON. The plan leaves it out of
   Phase 2 and allows it as an additive follow-up.
4. **Owner: lower `tunable` away in bytecode?** Only if the boids benchmark
   shows a cost.
5. **Owner: how long between Phase 4 and Phase 5?** Every consumer is
   yours, so one release is probably enough. Phase 4 cannot start until
   Cheesecake's `--set` change has landed.
6. **Cheesecake maintainer:** replace `config_overrides.cpp`'s text scanner
   with `pb_source_set_setting`, which also opens `--set cam.DIST=…` on
   imported modules (moses-debrief).

## Implementation notes (Phase 1, 6 November 2026)

Phase 1 shipped additively; `config let` is untouched and means what it did.

**What was built, by step:**

1. **Reserved names.** `compiler/hint_names.rs`: binding `tunable` or
   `rigid` anywhere (a `let`/`var`/`const`, `state`, `memo`, `fn`, a
   parameter, a lambda parameter, a loop or match variable, an enum or
   class, an import alias) is a compile error. A `native fn` declaration is
   exempt, because the core declares both hints that way. The
   `lint/dm_hints.rs` escape for files binding `tunable` is gone.
2. **Typed as identity.** `typecheck/builtin_types.rs`: `tunable` and
   `rigid` return their first argument's type.
3. **Hint widened, `options:` added.**
   - `dm/hints.rs::data_number_literals` marks every number literal in a
     record, list or constructor call (without bounds). A string, bool or
     `nil` marks nothing and gets no warning. Bounds on non-numbers are
     `dm-hint-bounds`. Numbers in `options:` are never moved, like bounds.
   - `core/packages/core/src/dm.ptl` gains `options?: list`, plus a second
     form, `tunable(value, options: list)` (see deviations).
   - New lints: `tunable-value-not-in-options`, `tunable-on-var`,
     `tunable-not-whole-initializer`. `config-not-data` covers a setting
     whose value is computed, and suppresses `dm-hint-not-literal` there.
4. **Settings in the readers.**
   - `static_value::is_setting` is the one definition.
   - `setting_value_expr` (was `config_value_expr`) unwraps a top-level
     `tunable` call without the `is_config` gate, in `static_bindings`,
     `literal_edit`, `rewrite::find_binding_path` and `source_diff`.
   - `StaticBinding` gains `options`. `is_config` now means "is a setting".
   - The compiler precomputes `Compiler::setting_stmts` per module and marks
     a setting's binding term and data literals `is_config`. So DM v1's
     policy, ConfigStrict, `ParamSelector::ConfigOnly`, `show-static` and
     the C bridge's `"config"`/`"range"` all see a `tunable` setting with no
     further change.
5. **Selectors.** `--knob-strict` / `knob_strict` (CLI, JSON requests,
   the MCP tool's `knobStrict`) is the new name; `--config-strict` stays an
   alias. `knob` is listed first in the selector docs.
6. **Parity test.** `core/tests/setting_parity.rs` rewrites every
   single-line `config let` in `examples/` and `core/tests` into the
   `tunable` form, then compares three things: `static_bindings` (value,
   text, range, setting, exported), a set-by-name of every rewritten
   binding, and the compiler's config-marked constants. It ran over
   20 files, 132 settings and 13 compiled programs, all identical.

**Checks run:**
- `cargo test` in `core`, `cli`, `core-libs/petal-ui` (including the DM
  study ladder), `garden` and `integrations/petal-c-bridge`;
- `make test-c-bridge` and `make test` (vitest), after regenerating
  `docs/stdlib.json`;
- the DM gallery: garden-web built from this tree, and
  `check-dm-samples.mjs` run over the published samples and over their
  `tunable`-only rewrites. Both outputs were identical to the vendored
  build's, apart from prelude line numbers in refusal messages, which come
  from unrelated prelude changes;
- `petal lint` over `examples/`, Garden's examples, the gallery samples and
  Cheesecake's games: no new findings. The 3 `config-not-data` hits in
  Cheesecake predate this change; only their wording changed.

**Deviations from the plan:**
- **Hosts can still register a native named `tunable` or `rigid`.**
  `register_native` returns an id and has no error path, and no host does
  this today. Such a registration is still reported as `dm-hint-disabled`,
  so only Petal source is refused.
- **A second native form, `tunable(value, options: list)`.** Native named
  arguments may not leave a gap below a supplied one
  (`NativeSignature::bind`), so `tunable("a", options: [...])` would
  otherwise be "missing a value for parameter 'min'". The same applies to
  `tunable(2.5, max: 5)`, which was already an error before this change.
- **Unchanged until later phases:**
  - `ConstructKind::ConfigLet` stays as it is, and a `tunable` setting's
    construct kind stays `Let` (Phase 2 added `TunableLet`);
  - `DeclKind::ConfigLet` keeps the `"config let"` label for every setting;
  - Garden's `CandState::Config` keeps its `"config"` label (nested
    setting literals now show as `tunable` through the widened hint);
  - the `"config"` tag in propose-edit's text output.
- **DM v1 needed no change.** Settings' terms carry `is_config`, which v1's
  `apply_policy` already reads.

## Implementation notes (Phase 2, 6 November 2026)

Phase 2 is additive: every key and function a host used before is still
there, so a re-vendor changes nothing for Cheesecake or worlds-fair.

**What was built:**
- **`"tunable"` in `pb_source_bindings_json`** (`source.rs::bindings_json`).
  It is present exactly when the binding is a setting (`StaticBinding.is_config`,
  which Phase 1 made "is a setting"), so `"config"` is true exactly when
  it is present. It holds `min`/`max`/`step` from the range and `options`
  as values in the bindings' JSON, each key only when written. A
  `config let X = v` gets `"tunable": {}`. `"range"` is unchanged.
- **`pb_source_set_setting(s, name, value_json)`**: `pb_source_set_value`
  with the name as the path, after checking the last binding of `name` is a
  setting (`PB_ERR_INVALID_ARG` if not, `PB_ERR_NOT_FOUND` if unbound). C++
  `Source::set_setting`.
- **`pb_vm_set_tunable`** and Rust `Vm::set_tunable`. `pb_vm_set_config`
  and `Vm::set_config` call them. The C++ `Vm::set_config` is
  `[[deprecated("use set_tunable")]]`; the C function and the Rust method
  are deprecated in their docs only, so no vendored build gains a warning
  from them.
- **Reload report:** `values[].tunable` beside `config` (the same value),
  and `constructs[].kind` `"tunable let"`, from a new
  `source_diff::ConstructKind::TunableLet`: a non-`var`, non-`const`
  top-level `let` that `is_setting` accepts without the keyword. Changing
  `let X = 1` to `let X = tunable(1)` when statements were also added or
  removed is therefore a remove plus an add, the same as gaining `config`
  is today.
- **Docs:** a "Tunables" section in `include/petal_bridge.h` (after the
  bindings overview) and the `pb_source_set_setting` entry; "Tunables and
  settings" in `docs/usage/embedding-c.md`, replacing the `config let` /
  `"range"` bullets; the C name renamed in `hot-reload.md`,
  `program-modification.md`, `Builtins.md`, `config-let.md` and the bridge
  README. `Env::set_config_value` (Rust core) keeps its name until Phase 3.
- **Tests:** `source::tests` (the section with mirrors for a string with
  options, a bool, a record, a `config let`, a plain `let` and a `var`;
  `set_setting`), `vm::tests::a_tunable_setting_is_set_and_reported_as_tunable`,
  `source_diff` (`TunableLet`, not for a `var` or arithmetic), and
  `bridge_tests.cpp::tunable_settings_have_a_tunable_section_and_set_by_name`.
  The existing C++ tests use `set_tunable`.

**Not done here (outside this repo):** re-vendoring into Cheesecake and
worlds-fair, and dropping Cheesecake's `6e124ec` hand patch. After a
re-vendor, Cheesecake's `script_host.cpp:908` (`s.vm.set_config(...)`) gets
a deprecation warning; renaming it to `set_tunable` is a one-line change.
