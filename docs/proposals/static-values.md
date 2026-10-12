# Proposal: static values (build-once constants that follow the source diff)

Status: proposed, 12 October 2026. Stage 0 (the classifier, `petal
show-static`, the `CONST` flag, `--static-audit`) is implemented and its
go/no-go gate passed. Of stage 1, the cache itself is implemented
(`core/src/static_cache.rs`: slots, the skip lowering, the immutable bit,
clear-all on reload); stage 2 is not started. Results:
[`docs/tasks/static-value-optimization.md`](../tasks/static-value-optimization.md#s1-caching-cleared-on-every-reload).

## Summary

A Petal host re-runs the whole script every frame, so every top-level table is
rebuilt every frame, and the only thing that survives a run is `state`. This
proposal makes values that depend on no dynamic data **static**: built once,
kept between runs, and rebuilt after a hot reload only when the source diff
touches something they depend on.

The request was for three properties: automatic detection, storing only the
values dynamic code reads ("leaves"), and diff-aware rebuilding. A first design
with all three at full strength was written and then attacked by three
independent reviews (soundness against the code, the author's experience,
measured payoff). All three said **adopt a smaller version**. The first design
was unsound in four places and two to three times larger than its payoff
supports.

**Recommendation: adopt, in stages, with the three properties kept but
reshaped.**

| Requested | What this proposal does |
|---|---|
| Auto-detect constant expressions | Yes. A classifier marks each top-level declaration static or dynamic, with the reason. An optional `const` asserts it, so a table cannot silently fall back to per-frame. |
| Store only the leaves | Yes, at declaration granularity: a static declaration gets a slot only if dynamic code reads it. Sub-expression leaves and constants inside function bodies are deferred. |
| Diff-aware dirtying | Yes, in stage 2: the existing source diff names the changed declarations, a dependency graph between declarations gives what they affect, and only those slots are rebuilt. Stage 1 clears everything on a reload. |

What drove the recommendation:

1. **The speed gain is small.** Measured by hand-simulating the feature:
   about 5% of instructions on Moses (the 27,000-line game), 0 to 3% on the
   example panels. This is not a performance feature.
2. **The live-coding gain is large.** Moses keeps 74 constant tables (1,772
   lines) in `state` to avoid the rebuild, so editing them does nothing until
   F5. That is "exactly the content authors edit most"
   (`docs/tasks/moses-debrief.md` §3.1).
3. **Diff-awareness pays only for big programs, and there it is necessary.**
   Moses spends about 2.3 s building its tables at boot. If those become
   static values that are all cleared on every reload, every structural edit
   costs that again.

The strongest argument against: the same live-coding benefit is available from
a plain `const` keyword with a per-declaration hash, at perhaps a fifth of the
size, and soundness of the automatic version rests on a new whole-program
purity analysis where a wrong answer is a stale or corrupted value, not a slow
one. Stage 0 exists to find out whether the classifier is good enough before
anything depends on it.

## Motivation

### What happens today

- All imported modules are merged into one root block, and `Env::run`
  re-executes all of it each frame (`core/src/env/run.rs:85-118`). Every
  top-level `let`, every `MakeClosure` and every enum's constant data runs
  again.
- "Hoisted constant lets" (`hoistable_const_lets`,
  `core/src/compiler/mod.rs:2504`) are about declaration order only. They are
  still evaluated every run. `docs/writing-petal-guide.md:104-106` reads as if
  they were evaluated once; `~/cheesecake/docs/petal-api.md:43` says
  "recomputed every frame", which is what the code does.
- There is no constant folding and no composite constant: the constant table
  holds scalars only (`core/src/constant_table.rs`).
- A plain `state X = expr` is the one build-once mechanism, and its
  initializer never re-runs: "a `state` slot that was initialized from the old
  value keeps its value" (`docs/hot-reload.md:54-56`).

### What authors do about it

Counts are from a heuristic scan of 684 `.ptl` files (about 180,000 lines);
treat them as ±10%.

| Pattern | Count | Where |
|---|---|---|
| Top-level `let` with a plausibly constant table or derived value | 1,580 of 7,408 lets | all trees; 827 are literal tables, 127 of those 10+ lines |
| Plain `state` holding a never-assigned constant | 100 | 78 in Moses (74 in game files, 1,772 lines), 16 in `examples/` |
| Lazy-init guards on a module cell (`if X == nil then set X = build() end`) | about 13 | 12 in Moses, e.g. `map.ptl:1472`, `terrain.ptl:613` |
| Constant tables built inside a function on every call | 279 | e.g. `moses/hud.ptl:250`, `examples/dashboards/timeline-explorer/app.ptl:236` (133 lines) |
| Top-level lets that look constant and are not | 149 screen size, 54 text measurement, 11 theme | e.g. `examples/productivity/kanban/app.ptl:130` |

The Moses README states the rule every author on that project had to learn
(`~/cheesecake/games/moses/README.md:1082-1085`):

> Module top levels run **every frame**: a top-level `let` table is rebuilt
> each time. Small constant tables are fine; put big ones in `state NAME =
> [...]` (built once; F5 to rebuild after an edit) and `freeze()` the very big
> ones.

### Measured baselines

M-series Mac, release builds. Wall-clock differences under about 5% were
inside noise; instruction and allocation counts are exact.

| What | Cost |
|---|---|
| Rebuild a 100-record literal table | 22–31 µs per run, 503 allocations |
| Rebuild 1,000 computed records | 155–170 µs per run |
| A `state` cell hit (the workaround's per-frame cost) | about 72 ns |
| Rebuild a 3-element list literal | about 62 ns, so parking small tables in `state` is a loss |
| Enum data read `K.v.code` | 66 ns over an empty loop; a record field is 0.8 ns |
| `K.all` | 173 ns, one list allocation per read |
| Enum data declaration | about 220 ns per variant per run |
| Moses: 180 literal top-level lets moved behind build-once by hand | 83.1k → 79.1k instructions per frame (−4.8%), record allocations −41%, GC time −14% |
| Same experiment on four example panels | −0.03% to −2.8% instructions |
| `petal check` on Moses (a full compile) | 587 ms |
| Moses boot (first 60 frames) | about 2.3 s, much of it building tables |
| A 400-record `let` table passed to a memoized function | 58 misses in 30 frames; 0 when its identity is stable |

The last row is a second-order win nobody asked for: memo validation compares
arguments structurally with a 512-node budget (`core/src/memo.rs`
`ARG_COMPARE_BUDGET`), so a large table rebuilt each frame misses every
frame. A value with a stable heap identity hits.

### What was already planned

- `docs/tasks/moses-debrief.md` item 6 asks for an explicit
  `const NAME = expr`, "re-evaluated when its initializer's source (or a
  `const` it reads) changes".
- `docs/dev/reactive-rendering-plan.md` P2 plans an interprocedural pass
  tagging each term with the input classes it depends on, and "hoist the
  constant class out of the frame … computed once per program version". That
  is coarser than diff-aware, and it is the same classifier this proposal
  needs. This proposal is P2's constant class, done first.
- The same plan warns against storing every intermediate ("trace memory is
  the trap") and against making users think in graphs.

## Design

### 1. What is static

A top-level declaration is **static** if its initializer can only ever produce
one value for a given source text. It is decided at compile time from the
program's structure, never from literal values, so a literal-only edit never
changes the classification.

An initializer is static when everything in it is one of:

- a literal, a list or record of static parts, an operator on static operands;
- another static declaration, or a `config let` (its inputs are source
  literals; `--set` and `set_config_value` already go through the reload
  path);
- a call to a builtin or native carrying a new **`CONST`** effect flag;
- a call to a user function that is **static-pure**, with static arguments.

Everything else is dynamic, including anything unknown.

**`CONST` is not today's `PURE`.** The existing `NativeEffects::PURE` row
means "reads no tracked input", and the review found pure rows that are not
constant: `noise` (reads the noise seed, `core/src/builtins/mod.rs:250`),
`fn_info`/`fn_ast` (carry source positions), `font` in petal-ui (resolves
through the host's font provider), `is_valid`, several Garden panel stubs, and
the C bridge's default of flags `0` meaning pure. `CONST` is an explicit
opt-in per native: same arguments, same result, in any run, on any host. A
native without it is dynamic. `native fn` declarations compiled without a
host (`petal check`, the LSP) are dynamic too, so those tools may report less
as static than a host does; they never report more.

**Static-pure functions** need an interprocedural summary, which does not
exist today (the dependency walks in `program_analysis.rs` stop at calls). A
function is static-pure if its body, transitively:

- reads no `state`, no dynamic native, and no cell other than its own locals;
- emits nothing (a debugging `print` makes it dynamic, and `show-static` says
  so);
- calls only static-pure functions. Callees must resolve through the two ways
  functions are actually bound: direct closure captures, and the write-once
  cells that hoisted forward references use. Without that rule
  `codes(TILES)` in `moses/config.ptl:129` would be dynamic, because `codes`
  is declared 144 lines below its use.
- Higher-order builtins (`map`, `filter`, `sort_by`) are static when the
  function argument is static-pure. Method calls are static only if every
  method of that name is.

Three structural rules close holes the first draft had:

- **Loops and branches are static only as a whole.** A value computed inside
  a loop from the loop variable is dynamic unless the entire loop is one
  static region with no dynamic term inside. (The draft would have cached
  `let sq = [i, i * i]` once and served iteration 0's value to every
  iteration.)
- **A value that is mutated is never cached.** `let xs = []` followed by
  `xs = append(xs, i)`, or `let by_sec = [0, 0, 0, 0, 0]` followed by
  `by_sec[i] = v` (`examples/productivity/settings/app.ptl:334`), is lowered
  to an in-place write on the literal's own storage today. Such a
  declaration stays inline. `hoistable_const_lets` already has this rule.
- **A cached value contains no closure, `var` cell, handle or pending
  result.** Checked when the value is stored; a value that fails is not
  stored and the declaration runs per frame, with a warning.

### 2. What is kept

A static declaration is **read** if dynamic code uses it: as an operand, as a
closure capture, or by name from a host or tool. Only read declarations get a
slot. A static declaration that only feeds other static declarations is
computed during a build and then dropped.

The saving is real for source tables that feed a derived one. In
`moses/map.ptl`, `POLYS`, `HOODS_SRC`, `SITES_SRC` and `WORKS_SRC` (about
400 lines, 3,500 values) exist only to build the grid; with this rule only
the grid is retained. For most other declarations it saves little, because a
leaf's value usually still references its inputs.

The cost is that rebuilding one slot re-runs the declarations it depends on,
since their values are gone. Section 6 says where that hurts.

### 3. How it runs

Evaluation stays in the VM at run time. The compiler evaluates nothing, the
IR term graph is unchanged, and only bytecode lowering differs (one
`OptFlags` bit, like `match_dispatch`).

- **No code is moved.** Each contiguous run of static instructions in the
  root block gets a `StaticSkip { after, slots }` in front of it, which jumps
  past the run when every slot the run feeds is filled. Each read declaration
  ends in a `StaticStore`. If a run is skipped, every later run that needed
  its registers feeds a subset of the same slots and is skipped as well.
- **A run that feeds no slot always executes.** An unused `let BAD = XS[7]`
  still raises at its line, and a top-level `assert(len(TABLE) == 12)` still
  runs every frame. (`assert` is pure, so the draft would have deleted it.)
- **A failing build is not stored**, and raises at the same point on every
  run, as now.
- **Builds are eager.** On the first run after a load or reload, control
  reaches every top-level declaration, so the build cost lands at the reload
  and not in the middle of play when a rarely opened panel first reads a
  table.
- **Reads are an indexed load** from a dense per-stack vector: no hashing, no
  call path, no touch-and-sweep. That is cheaper than a `state` hit (72 ns).
- **The table belongs to the `Stack`**, like `enum_data` today, so forks and
  restores resolve heap ids in the right heap. It is a GC root, and is
  stamped with (program epoch, opt flags) and discarded on a mismatch, which
  covers `restore_execution` onto a reloaded program.
- **Cached values are guarded.** An immutable bit is set on a stored value
  and checked in the seven `*_in_place` heap operations
  (`core/src/heap.rs:1222-1419`), which copy when it is set. Escape and
  last-use analysis treat a slot load as a non-fresh root. The bit is a
  backstop for the "never cache a mutated value" rule, not a substitute.
- **Small things are left alone.** A static run with fewer than about two
  allocations gets no slot; rebuilding it is cheaper than checking.
- **Off when it cannot help or must not interfere:** under the `baseline`
  and `explain` policies, for run-once `petal run`, and whenever observations
  or the per-term trace are being preserved. That last condition currently
  includes Garden panels, which always record observations
  (`garden/garden-script/src/panel.rs:2772`); they get nothing until skipped
  runs can replay their named values the way memo scopes do.

### 4. What a reload does

**Stage 1: clear everything.** `transfer_stack_state`
(`core/src/transfer_state.rs:30-57`) already clears closures, enum data and
memo on every patched or recompiled reload. The static table joins that
list. The next frame rebuilds every slot once, which is what every frame does
today.

**Stage 2: follow the diff.** This is the requested behaviour:

1. **Detect the diff.** `source_diff::diff_source` already compares old and
   new ASTs per file and reports, for a structural edit, which top-level
   constructs were added, removed or changed by `(kind, name)`; for a
   literal-only edit, which literal under which binding path.
2. **Find what it affects.** The classifier records, for each static
   declaration, the set of declarations it depends on after name resolution
   (other lets, functions, enums, every method of a called name), keyed by
   module-qualified name. That set is closed transitively.
3. **Dirty and rebuild.** A slot is kept across the reload only if all three
   hold: its declaration is unchanged in the diff; its resolved dependency
   set is the same set of names in the old and new program; and nothing in
   that set changed. Otherwise it is cleared and rebuilt on the next frame.

Condition two is what makes this sound without content hashing. If an edit
changes what a name resolves to (a new import, a shadowing declaration, an
added overload or method), the dependency set differs and the slot is
cleared.

For a literal-only ("patched") reload nothing is recompiled, so the
dependency graph is still valid: the edited literal's enclosing declaration
is the changed one, and its dependents are dirtied. A literal inside a
function body dirties everything that calls that function.

A rebuilt value that is structurally equal to the old one keeps the old heap
value, so memo records that hold it stay valid once memo survives reloads.

**The reload report** gains three lines, printed by default in the SDL and
Garden hosts: constants rebuilt, constants kept, and **stale seeds**: `state`
cells whose initializer read something that changed. Those keep their old
value, as they always have (`state adj = ZERO` in
`examples/productivity/photo-adjust/app.ptl:554`). The report is what makes
that visible.

### 5. Syntax and tools

- **No new syntax is needed for the mechanism.** A `let` that is static is
  cached.
- **`const NAME = expr`** is a `let` that must be static. If it is not,
  compilation fails with the reason chain:

  ```text
  error: const LAB_W is not constant
    --> app.ptl:784
     | const LAB_W = build_label_w(events)
     = build_label_w calls text_width at app.ptl:382
     = text_width depends on the host's fonts
  ```

  The author review argued, and I agree, that this belongs in the first
  version rather than later. Auto-detection alone gives no way to state
  intent, and its failure is silent: a 400-line table that picks up one
  `text_width` call goes back to being rebuilt every frame with no message.
- **`petal show-static file.ptl`** lists every top-level declaration as
  static (kept / not kept, with retained size) or dynamic, with the reason.
- **LSP** shows the class on hover.
- **Lint `state-as-constant`**: a plain `state` that is never assigned and
  has a static initializer gets a suggestion to become `const`. It is a
  suggestion, not automatic: `state notes = [` in
  `examples/productivity/notes/app.ptl:74` is syntactically identical to
  `state SCRIPT = [` in `moses/events.ptl:95` and is real state, and hosts
  write top-level state by name.
- **`--static-audit`**, like `--effect-audit`: build static runs while
  watching the dynamic-input counters and fail if a "static" build touched
  one. This is the oracle for the `CONST` flags.

### 6. Where it works and where it does not

**Works well**

- Literal tables at module top level in a program that runs every frame.
- Tables built by constructor functions (`hd(...)`, `ch(...)`, `P()`/`O()`)
  and tables derived from other tables (`codes(TILES)`, `build_lut()`).
- Big programs with expensive builds, once stage 2 lands: an edit to one
  table rebuilds that table and its dependents, not all 2.3 s of them.
- Large tables passed to memoized functions.

**Does nothing**

- Scalars. 2,350 of the 3,930 plausibly constant lets are one `LoadConst`
  already.
- Anything derived from the screen, the theme or text measurement. In
  `petal/examples` half of all top-level lets are dynamic for this reason;
  Garden panels and `~/worlds-fair/ui/ptl` have almost nothing static. This
  is the next tier (values cached per input class until that class changes),
  and it is not in this proposal.
- Constants inside function bodies (deferred; 279 sites, mostly small).
- Enum data reads through a variable (`k.code`). See the separate fix below.
- Garden panels, until observations can be replayed.

**Works badly**

- **One dynamic read poisons a table.** A theme colour in one field, a
  `text_width` in a helper, a debugging `print` in a constructor, or a host
  native missing its `CONST` flag makes the whole declaration dynamic. About
  1–2% of tables in today's corpus, but the corpus was written when
  everything was rebuilt anyway. `const` turns this from silent to an error.
- **Half-reload.** `moses/map.ptl:1472` builds its grid into `state var`
  cells under `if G == nil`. Convert the source tables to constants and they
  now reload while the grid, still in `state`, does not. The fix is to
  rewrite the guard as `const G = build_grid()` and remove `get` at about 20
  read sites; nothing does that for the author. The stale-seed report names
  the cell.
- **Heavy builds under continuous edits.** Dragging a literal that feeds a
  128×128 flood fill rebuilds it on every tick. Today that table is either
  rebuilt every frame anyway (no worse) or parked in `state` and never
  rebuilt (so the drag does nothing). Builds over a time threshold should be
  debounced to the end of the drag.
- **Rebuilding one slot re-runs what it depended on**, because intermediates
  are not kept. For the builds measured this is negligible (`build_glyphs`
  is 0.16 ms). It is bad only when one expensive intermediate feeds several
  kept values and the edit is downstream of it. If that shows up, an
  intermediate whose build exceeded a time threshold can be kept as well.
- **An embedder's wrong `CONST` flag is a stale value forever**, where a
  wrong `PURE` row today costs only a missed memo. `--static-audit` covers
  the natives a test run actually calls and no others.
- **Two modes to keep equal.** Another lowering flag doubles the
  differential-oracle matrix.

### 7. Separable work that should not wait for this

- **Enum data read path.** A field read on a variant splits the tag string,
  does two string-hash lookups and an uncached map get
  (`core/src/backend/ops.rs` `enum_variant_field`). Key the data by a dense
  variant index, cache `K.all` per enum, and fold a literal
  `Enum.variant.key` at lowering. About 150–300 lines, and it is what
  actually fixes the 66 ns read. Static values do not fix `k.code` through a
  variable, which is the common case.
- **A frozen heap generation.** A rooted 100,000-record table costs a 10 ms
  pause per collection today. Cheesecake's local `Heap::freeze` patch took
  its pauses from about 25 ms to about 2 ms. Upstreaming it is worth more to
  Moses than this proposal and is independent of it. Static values make it
  more valuable by keeping more data alive.

## Before and after

"Before" is quoted from the corpus. The literal-table, enum and builder
forms were checked to run on the current release CLI. "After" is proposed
and has not been run.

**1. The common case: source unchanged.**
`examples/productivity/kanban/app.ptl:38`

```petal
let LANES = [
  {name: "BACKLOG",     color: #6c7a90, limit: 0},
  {name: "IN PROGRESS", color: #5b9dff, limit: 3},
  {name: "REVIEW",      color: #d9a441, limit: 2},
  {name: "SHIPPED",     color: #3fae7a, limit: 0},
]
```

After: identical. Built once; rebuilt when a literal in it changes. Nothing
to learn.

**2. The case this is for: a constant parked in `state`.**
`~/cheesecake/games/moses/events.ptl:95` (406 lines)

```petal ignore
state SCRIPT = [
  {id: "order_129", at: [1935, 1], …
```

```petal ignore
const SCRIPT = [
  {id: "order_129", at: [1935, 1], …
```

One keyword changes, by the lint. Editing an event now shows on the next
frame, not after F5. The README rule quoted above is deleted. Newly
possible mistake: decisions already in flight hold records from the old
script in other `state`; the stale-seed report lists them.

**3. A derived table.**
`examples/games/fractal-explorer/app.ptl:430`

```petal ignore
state lut = build_lut()
```

```petal ignore
const lut = build_lut()
```

Editing a palette stop now rebuilds the lookup table. Today it needs a
restart.

**4. Enum data and its derived tables.**
`~/cheesecake/games/moses/config.ptl:121-129`

```petal ignore
pub enum Tile
  water(code: 0), land(code: 1), park(code: 2), marsh(code: 3), beach(code: 4),
end
pub let TILES = Tile.all
pub let T = codes(TILES)
```

After: identical, and `TILES` and `T` are built once, where today they are
rebuilt every frame. Deleting `T` in favour of `Tile.park.code` needs the
enum read-path fix in section 7, not this proposal.

**5. No difference, and a trap.**
`examples/dashboards/timeline-explorer/app.ptl:783-784`

```petal ignore
state events = build_events()
state lab_w = build_label_w(events)
```

`events` can become a `const`. `lab_w` cannot: `build_label_w` calls
`text_width`, which depends on the host's fonts. The two lines look alike.
Written as `let lab_w`, it would measure every title every frame, silently.
Written as `const lab_w`, it is the compile error shown in section 5, and it
stays `state`. The old form is the right one here.

**6. Collides with `state` and hot reload.**
`~/cheesecake/games/moses/map.ptl:1472`

```petal ignore
if G == nil then
  let g = build_grid()
  let ht = build_hood_tiles(g)
  …
  set G = freeze(g)
```

After converting the source tables this block is unchanged and `G` is stale
after every edit to them. The honest "after" is a manual rewrite to
`const G = build_grid()`.

**7. Deferred: a constant inside a function.**
`~/cheesecake/games/moses/hud.ptl:250`

```petal
  let icons = ["pause", "play", "ff", "fff"]
  let tips = ["Stop the calendar (Space)", "One month in four seconds (1)", "Fast (2)", "Fastest (3)"]
```

Unchanged and still rebuilt per call. The gain is two four-element lists a
frame, and doing it needs per-constant thunks because a function's registers
are per call.

**Migration.** Existing code keeps working unchanged. `state` → `const` is a
lint suggestion with a fix. Lazy-init guards are a manual rewrite.

## Assessment

### Project goals

Advances live coding directly: constant data becomes editable while running,
which Moses gave up. Advances ease of use by deleting a rule every author on
a large project has to learn. It works against introspection unless
`show-static` and `const` ship with it, because the classification is a
whole-program property with no mark in the source. It respects "don't make
users think in graphs" when writing code and breaks it at the moment someone
asks why a frame got slower; the reason chain is the mitigation. This is
Petal-specific surface: it exists because the top level re-runs every frame
and because hot reload is a goal.

### Ease of writing

A newcomer writing `let TABLE = [...]` learns nothing new. To predict the
classification they must know which natives are dynamic (screen, theme, text
measurement, time, random, noise) and that it is transitive through their own
functions. Two of seven real declarations walked through in review were not
predictable: `text_width("MMMMMMMMMM", SIZE)` with literal arguments looks
constant and is not, and `codes(TILES)`, which fills a local record in a
loop, looks dynamic and is not. "Constant" now has three meanings in the
docs (hoisted `let`, `config let`, static); the guide has to state one.

### Bug-freedom

Prevents: stale constant tables after an edit; forgotten lazy-init guards.
Makes visible: stale-seeded `state`. Newly possible: a stale value from a
wrong `CONST` flag; a silent fall back to per-frame without `const`;
half-reload. All four defects found in review (loop-variant caching,
in-place mutation of a cached value, deleted asserts, moved errors) are
addressed by the rules in sections 1 and 3, and each needs a regression
test. The feature stays testable: cache on versus off must give identical
output over the corpus and under fuzzing, and a reloaded run must equal a
cold start of the new source with state preserved.

### Performance

See the baselines. Expected, by workload:

| Workload | Gain | Basis |
|---|---|---|
| Moses | −4.8% instructions, −41% record allocations, −14% GC time from literal tables; perhaps 1–2 points more from derived tables | measured by hand simulation; the increment is an estimate |
| Example panels | 0 to −2.8% instructions | measured |
| Schema-heavy panel (settings) | −2.8% instructions, −50% allocations | measured; memo already covers most of it |
| Large tables as memo arguments | every-frame miss becomes a hit | measured on a synthetic script; not seen in four real apps |
| Run-once scripts, tests | none; the feature is off | by design |

Costs: a classification pass on every compile (estimated low tens of
milliseconds on Moses against 587 ms); a skip check per static run per
frame; retained memory (about 65 KB for Moses's literal tables, estimated);
longer GC marking of retained data until a frozen generation exists. Gating
benchmarks: the Moses headless profile (49.87M instructions per 600 frames,
2.71 ms per frame), the four panels, `test/benchmarks/*` under `petal run`
(must not regress), `core/examples/reload_timing.rs` for the 0.8 ms patched
path.

### Live coding and hot reload

Editing a literal in a constant table: visible next frame, where a `state`
table shows nothing. Editing a constructor's body: the tables built with it
rebuild. Adding an enum variant: no visible change. The hot-reload contract
(a patched program equals a fresh compile) holds because the IR is untouched
and the classification never reads literal values. Stage 1 costs one full
rebuild per reload; on Moses-sized programs that is the argument for stage 2.

### Introspection, debugging and program manipulation

The term graph is unchanged, so provenance, slicing and `explain` see the
same program; `explain` and trace-preserving modes run with the cache off.
Direct manipulation of a literal in a cached table goes through the patched
path and dirties the slot. Not verified: whether `propose-edit` needs values
of intermediates that are no longer kept.

### Maintainability

Sizes are the implementation review's estimates.

| Piece | Size |
|---|---|
| Classification pass with reason chains | large: 800–1,200 lines plus tests, comparable to `escape.rs` |
| Read-set selection and the mutation and state-init exclusions | about 150 |
| Escape, last-use, copy-propagation and instruction tables | about 140 |
| Lowering, VM dispatch, stack, GC root, epoch stamp | about 320 |
| Heap immutable bit | about 80 |
| `CONST` flag for natives and the C bridge | small to medium |
| `const`, `show-static`, LSP, lint | about 400 |
| Differential and fuzz oracle | medium to large |
| Stage 2: dependency sets, diff-driven dirtying, reload report | medium to large |

Invariants future work must keep: every new term op, builtin and native
defaults to dynamic; a slot load is never a fresh root in any in-place
analysis; no cached value holds a cell, closure, handle or pending result;
the table is valid for one (program epoch, flags) pair and any new way to
swap or patch a program must clear or dirty it; hoisting changes must keep a
static run contiguous.

### Domains

| Domain | Effect | Example |
|---|---|---|
| Games | Helps most: every frame runs, big data tables | `moses/events.ptl:95`, `moses/map.ptl:105-828` |
| Graphics and creative coding | Helps a little | `fractal-explorer/app.ptl:430` |
| Productivity and UI apps | Neutral | `kanban/app.ptl`: a few colours, one table, then everything is screen-derived |
| Dashboards and data | Helps seeded datasets; text-measurement trap | `timeline-explorer/app.ptl:783-787` |
| Web (WASM) | Neutral | the diagram-canvas editor does a full load per edit, so stage 2 does nothing there |
| Editor scripting (Garden) | Neutral, and off until observations replay | `main_menu.ptl:69-87` is theme, palette, screen size |
| Embedding hosts | A small burden | each native's `CONST` flag is now load-bearing for correctness |

### Compatibility and migration

No existing program breaks. `const` is a new keyword; a grep of the
corpus found no binding or parameter named `const`. The feature can ship off by default
behind the policy flag while the oracle runs. It depends on no unfinished
work, and it builds the classifier that reactive-plan P2 needs.

### Prior art

The closest user-facing precedent is Flutter hot reload: static field
initializers are not re-run on reload, and its documentation lists that as a
known gotcha. This proposal is the fix for the same gotcha. The analysis is
binding-time analysis from partial evaluation. The runtime shape is GHC's
full laziness, whose known failure is space leaks from values floated out
and retained for the program's life; keeping only what dynamic code reads is
the answer to that. Incremental build systems (salsa, rustc) show that
identity must not be source position and that early cutoff on equal values
is the cheapest validation. React Compiler shows that assumed purity fails
silently and needs an opt-out. Common Lisp's `defconstant` and Clojure's
`defonce` show both ways a reload can go wrong: a constant inlined
everywhere and then redefined, and an initializer silently ignored.

## Alternatives considered

| Option | Benefit | Size | Why not, or why |
|---|---|---|---|
| A. Do nothing | none | 0 | Leaves the Moses workaround and its lost hot reload |
| B. `const` only, cleared on any reload | most of the steady-state gain if authors annotate; fixes stale tables | 300–500 lines | Every structural edit re-runs every initializer; nothing for unannotated code |
| C. B plus a per-declaration hash | B plus most of the diff-aware value | +200–400 lines | The performance review's pick. No interprocedural analysis, so a `const` initializer calling a user function must be trusted or rejected |
| D. Auto-detect with today's syntactic predicate plus `CONST` builtins, no user-function calls | 70–80% of the Moses gain, all of the dashboards' | 500–800 lines | Misses every table built through a constructor function, which is how the large ones are written |
| E. The first draft: lazy cones, sub-expression leaves, Merkle content keys, in-function constants, enum unification, frozen generation with demotion | 100% | several thousand lines | Unsound as written in four places; content keys not computable from what the compiler has |
| **This proposal** | D's steady state, plus constructor-built tables, plus diff-aware reloads in stage 2 | between D and E | Staged so that C's behaviour is reached early and the analysis is proven before it is trusted |

Stage 1 with `const` is close to option C in what an author sees. The
difference is that the classifier, not the author, vouches for a `const`
that calls a user function.

## Implementation sketch

| Stage | What | Gate to the next |
|---|---|---|
| 0 | The classifier with reason chains, `show-static`, the `CONST` flag, `--static-audit`. **No behaviour change.** | How much of the corpus classifies static, compared against the survey's 1,580; zero audit failures across the corpus and Moses |
| 1 | Slots, skip lowering, the mutation and state-init exclusions, the heap guard bit, epoch stamp, clear-all on reload. `const` and the lint. Differential fuzzing. | Oracles green; the Moses profile shows the expected drop |
| 2 | Dependency sets, diff-driven dirtying, value-equal cutoff, the reload report with stale seeds | Moses structural-edit time against stage 1 |
| Later, each on evidence | Observation replay (Garden); constants inside functions; per-input-class slots for theme and viewport; keeping expensive intermediates | A profile showing more than about 5% still in static work |

Independent of the stages and worth doing first: the enum read-path fix and
upstreaming the frozen generation.

Tests: a regression for each defect found in review; cache on versus off over
the example corpus and the golden corpus; a reloaded run versus a cold start;
`core/tests/hot_reload.rs` unchanged; a fuzz target in the style of
`cell_fuzz.rs`.

## Open questions

Decided on 12 October 2026: 1 yes (`const` is in), 2 yes (stage 0 is a
go/no-go), 7 yes (the enum read path and the frozen generation go first).
The work order is in
[`docs/tasks/static-value-optimization.md`](../tasks/static-value-optimization.md).
Questions 3 to 6 are still open.

1. **Is `const` acceptable?** The request was for automatic detection. This
   proposal keeps detection as the mechanism and adds `const` as an
   assertion. Without it the main failure is silent.
2. **Stage 0 as a go/no-go.** If the classifier marks much less of the corpus
   static than the survey suggests, the fallback is option C. Agreed?
3. **Eager or lazy top-level builds?** Eager keeps hitches at the reload;
   lazy never builds what is not read. This proposal says eager.
4. **Should a `state` initializer that reads a changed constant be re-seeded**
   when the cell was never assigned, or only reported? This proposal only
   reports.
5. **`CONST` as a new flag, or tighten `PURE`?** A new flag leaves the memo
   rows alone but gives natives two purity notions.
6. **Garden.** Is it acceptable for the first version to do nothing in Garden
   panels?
7. **Order against the separable work.** The enum read path and the frozen
   generation each deliver more measured speed than this. Do them first?
