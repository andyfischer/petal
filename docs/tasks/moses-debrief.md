# Moses debrief: what a 27,000-line game asks of Petal

Status: suggestions, 11 October 2026. Items 1, 2, 3 and 8 have since been
done and are marked where they are discussed.

`games/moses` in the Cheesecake repo (*The Power Broker*) is the largest
Petal program so far: 27,000 lines in 33 modules, written in three days by
seven parallel agent teams against a README "contract". This is what its code
and its profile say Petal is missing, ordered by how much each item would
have saved. The engine half of the same debrief is
`~/cheesecake/docs/moses-debrief.md`; it has the full measurements.

Method: `--vm-profile` and `sample` on three headless runs (steady play in
1956, a whole autoplayed career, the title), a read of every `.ptl` file, the
git history, and the test suite. Petal claims were checked against
`docs/language-guide.md`, `docs/Builtins.md` and a current
`core/target/release/petal` (9 October). Vendored commit: 6eb8f75.

## The short list

| # | Item | Kind | Evidence | Size |
|---|---|---|---|---|
| 3 | Trivial builtins as VM intrinsics (`float`, `int`, `min`, `max`, `clamp`, `len`, `type`) | perf, core | about 3,000 calls a frame; `float(i)` costs 5x the loop around it | medium |
| 4 | Real lints, run by default | tooling | one shipped bug `petal check` had already warned about; hundreds of redundant casts; builtins re-implemented | medium |
| 5 | `petal test` with non-aborting expectations | tooling | 7 hand-rolled check harnesses, a test DSL inside the game | medium |
| 6 | A build-once constant (`const`) | language | 93 `state` cells used as constants; editing them needs F5 | medium |
| 7 | Checked record shapes | language | a 1,091-line README is the type system; 966 `??`, 324 `field()` | large |
| 9 | Int-keyed maps and sets, bit operators | language | string keys built per lookup in the hottest function | medium |
| 10 | Growable numeric arrays and an in-place table of records | language, perf | 628,000 `append` calls and 12% GC in a career | large |
| 11 | petal-ui: layers, hit testing, retained drawing, layout | library | the game rebuilt most of petal-ui in 1,246 lines | large |
| 12 | Call a function with a record of arguments | language | agent tools declared twice, 280-line dispatch chain | small |
| 13 | Module-level state without `get`/`set` noise | language | 354 module-level `state var` cells, 1,013 `get`s | medium |
| 14 | Stdlib gaps: number formatting, `min_by`, `group_by`, `fill`, `ord`, integer hash | stdlib | each re-implemented two to four times | small each |
| 15 | Bridge: save and load state, stack left by a failed host call | embedding | headless debrief items 1 and 5 | medium |

Item 3 is measured and local. Items 4 to 6 are cheap and would have
prevented most of the friction below. Items 7 to 10 are the language work
that a second game of this size will hit again.

## 1. Performance

Steady play, unprofiled: 2.65 ms of script a frame, about 97,000
instructions and 6,000 native calls. A 5-second `sample` of that run, main
thread, inclusive:

| Share | What |
|---|---|
| 41% | `do_builtin_call` (all natives and builtins, callee time included) |
| 16% | `run_straight` |
| 13.2% | `petal_ui::text::native_text_width` |
| 6.1% | `push_closure_frame` |
| 6.1% | `collect_garbage` (11.7% in the career run) |
| 1.6% | `RunDeps::finish_run` (3.3% in the career run) |

The `text_width` and `RunDeps` rows were items 1 and 2. Both were fixed in
Petal on 11 October 2026 and are removed from this list (the other numbers
are kept): petal-ui keeps a face's decoded metrics until one of the font
bindings is set again, and a binding the host has not set since the last run
is not fingerprinted again. Cheesecake binds its fonts only when they change,
so it gets both when it next vendors Petal.

### 1.3 Trivial builtins are full calls (item 3)

Per play frame: `int` 1,090, `float` 940, `max` 490, `type` 295, `clamp`
240, `len` 175. In the career run `float` alone is 1.8 million calls. A
micro-benchmark on the current release CLI:

```
for i in range(0, 1000000) do set s = get s + float(i) * 0.5 end   // 0.06 s
for i in range(0, 1000000) do set s = get s + i * 0.5 end          // 0.01 s
```

`float(i)` costs about 50 ns, five times the rest of the loop body. This was
priority 4 in `~/cheesecake/docs/ideas/petal-performance.md` and is still
open. Compile `float`, `int`, `min`, `max`, `clamp`, `abs`, `floor`, `len`
and `type` of a known-arity call to opcodes that `run_straight` handles.

**Done, 13 October 2026.** Those nine are `Inst::Intrinsic`, computed in
`run_straight` for numbers (and lists and strings for `len`, anything for
`type`); every other argument, and a profiled or audited run, is the builtin
call it was. Over an empty loop `float(i)` went from 51 ns to 1 ns, `min`,
`max` and `clamp` from 55 ns to 5 ns, and the loop above from 0.066 s to
0.019 s. Every remaining native call lost an allocation (51 ns to 44 ns for
`sqrt`). Figures and what did not change are in
`docs/dev/performance.md` (Inline builtins). The two related sources of
calls below are not part of it.

Two related sources of calls:

- **petal-ui's overload wrappers.** `draw_rect#6` (ui.ptl:198) and
  `draw_polyline#4` (ui.ptl:490) are Petal functions in front of the
  natives: 7% and 3.2% of all instructions. The `type()` calls come from
  their `if type(c) == "record"` tests. Make `draw_rect(x, y, w, h, color,
  a)` one native that accepts a colour, or resolve overloads on annotated
  types at compile time.
- **Redundant casts.** See 2.1.

### 1.4 Garbage (item 10)

Collections cost 6% in play and 12% in the career (67 collections, 315 ms).
The drivers, all in script:

- Records rebuilt to change a few fields:
  `{...l, vol: v, vc: vc, speed: ..., cars: ...}` on a 28-field record
  (traffic.ptl:1140); a 50-field neighborhood record spread twice per
  update (sim.ptl:532-537).
- Geometry built as a list of three-element lists and flattened once
  (terrain.ptl:291: "appending is O(1); joining two flat lists copies
  both"). That is one heap list per vertex.
- A record or list allocated per widget per frame (`claim`, `R`, `blend` in
  widgets.ptl).

Language answers are in 3.4. The VM answers (reuse of swept slots, a
nursery) are item 7 in the performance document and still open.

### 1.5 Still open from the earlier performance list

The opcode mix has not moved: `LoadConst` 20%, `Move` 9%, `JumpIfPending`
6%. The two-target `Branch` and register coalescing remain the largest
interpreter items. Note that with `--vm-profile` on, every instruction takes
the slow path, so profiled totals overstate interpreter time; the `sample`
numbers above are the unprofiled truth.

One thing that is fixed and should be said in the docs: `set xs = append(get
xs, v)` in a loop is linear now (20,000 and 80,000 elements both under
10 ms). `Builtins.md` still does not say so, and Moses carries code written
to avoid it.

**Done, 13 October 2026**, with what checking it found. `Builtins.md`
(`append`) now says which appends are linear: a list a `let`, `var` or
`state var` holds directly. Three shapes are still quadratic and are listed
there with their timings: a list in a record field (`set r.items =
append(r.items, v)`), a list in another list (`set grid[k] = append(grid[k],
v)`), and an append through a function that takes the list and returns it.
A fourth was: any call in the appended value, builtin calls included (`set
xs = append(xs, float(i))`). The nine intrinsics no longer count as such a
call; every other one still does.

## 2. Tooling

### 2.1 Lints (item 4)

`petal lint` has one rule, `prefer-let`. The game's code suggests these, all
with mechanical fixes (**done, 13 October 2026**: what each became, and what
it found in the games, is under "What shipped" below the table):

| Rule | Count in Moses | Example |
|---|---|---|
| `float()` or `int()` on an operand that promotes anyway | 893 `float(` and 790 `int(` calls in the game; in the files sampled (terrain, sim, widgets) most are redundant | `(float(tx) - float(W) * 0.5) * T` (terrain.ptl:324) |
| `0.0 - x` for `-x` | 55 | `atan2(0.0 - dx, 0.0 - dz)` (structures.ptl:169) |
| `len(filter(xs, p)) > 0` for `any(xs, p)` | about 10 | terrain.ptl:632, economy.ptl:407 |
| `set X = {...get X, f: v}` for `set X.f = v` | about 15 | panels.ptl:912, :1326 |
| `set xs[i] = get xs[i] + d` for `set xs[i] += d` | 103 | sim.ptl:336 |
| a local re-implementing a builtin | `gf` four times for `r?.k ?? d`; `find`, `sum`, `any`, `color_lerp`, `text_ellipsize`, `text_wrap` | hud.ptl:112, widgets.ptl:267 |
| an `elsif x == "..."` chain of five or more arms | 1,477 literal comparisons in all | `icon`, widgets.ptl:537-742, 45 arms |
| a dead alias of a `var` | 51 | `let dd = dist` (traffic.ptl:858) |

Authors cast defensively because `/` on two ints truncates silently and
fields have no types. A real-division operator, or a warning on int `/` int
whose result meets a float, would remove the reason.

`petal check` already warns about a continuation line that starts with `-`.
Moses shipped that bug anyway (cheesecake af3ff22: a camera drag term
"computed and dropped") because nothing ran `check` in the edit loop. Make
warnings visible where authors already look: have `Env::load` return them so
a host prints them on reload.

Also open from `module-system-followup.md` and seen here: a qualified call
to a private or missing export is not checked, which is how every
cross-module call in the game is written; and a plain import of a module
that exports `log` replaces the builtin (legacy.ptl needs `import core:
log`).

**What shipped, 13 October 2026.** The rules, in `petal lint` by default
unless marked (docs/CLI.md has the table). A rule now fixes only what it can
show to be the same program and reports the rest; `--fix` prints what it
left. Counts are findings over the fifteen Cheesecake games (245 files) and
how many of them `--fix` rewrote:

| Rule | Games | Fixed | Notes |
|---|---|---|---|
| `no-redundant-cast`, extended: `float(n)` on a proven int beside a proven float operand of `+ - * / %` | 548 | 548 | was 0: the checker did not know a `for` variable over `range` of ints is an int. A cast beside an int operand stays (int `/` int truncates), and of `float(a) / float(b)` one goes |
| `prefer-compound-assign`, extended to `xs[i]` and `r.f` | 264 | 264 | 102 before |
| `neg-zero-sub` | 158 | 4 | fixed only for `0 - x` on a proven int. `0.0 - x` is reported and left: `0.0 - 0.0` is `0.0`, `-0.0` is not, and `atan2(0.0 - dx, …)`, the debrief's own example, tells them apart |
| `prefer-field-set` | 140 | 29 | fixed where the file shows the cell to hold a plain record. `{...nil, f: v}` is a record and `set X.f = v` on nil fails, so a cell that starts as `nil`, a parameter and a loop variable are reported and left: most of the 111 |
| `prefer-any` | 89 | 68 | fixed where the predicate is an in-place lambda with no call but to pure builtins; `any` stops at the first match |
| `dead-alias` | 50 | 48 | locals of one function only; a module-level `var` read into a `let` is a snapshot and is never reported |
| `shadows-builtin` (report only) | 10 | 0 | `find`, `commas`, `log`, `count`, `sort_by`, and `ui`'s `card`, `pill`, `wrap` |
| `reimplements-builtin` (report only) | 6 | 0 | `gf` five times and `g`: all `field(r, k, d)` |
| `int-division` (opt-in, report only) | 7 | 0 | all seven are a row or column index scaled by a float (`float(k / 2) * 3.0`), which is why it is opt-in |

All 1,102 fixes were applied to a scratch copy of the games: every file
compiles, `petal check` prints the same findings as before, and a second
`petal lint` has nothing left to fix. Over this repository's own corpus (214
files under `examples`, `test` and `garden`) `--fix` made 1,512 rewrites and
`tools/verify.ts --plan lint-fix` found 85 files IR-identical, 55 with the
same run trace, and none changed.

The `elsif` chain rule is `prefer-match`, which existed. The warning for int
`/` int is a `petal check` warning on a literal division that drops a
remainder (`1 / 2` is 0); the general form is the opt-in rule above.

Warnings reach a host through `ReloadReport::new_diagnostics`: the findings
a reload's compile has that the program before the edit did not, rendered
with `Env::render_diagnostics` (docs/embedding-guide.md); the C bridge hands
it out as `pb_vm_new_diagnostics_text`.

A qualified call to a private or missing export is a check error with its
position (`hud.hidden()`; it had no source position, so the name scan
skipped it). The plain import that replaces a builtin already had its
warning by the time this was checked (`` `log` here is the one module 'hud'
exports, which hides the builtin `log` ``).

### 2.2 `petal test` (item 5)

`assert` and `assert_eq` exist and abort at the first failure. Moses has
zero uses. Instead it has seven `dev/*_check.ptl` files with seven private
`fn check(name, ok)` helpers in four return conventions, hand-kept counters,
`if frame == N then` staging, and a printed `"<team> check: OK"` line for a
regex to find. Because "a regular expression cannot compare two numbers",
the game also grew a command language (`check PATH OP VALUE`, `mark`,
`wait_until`; auto.ptl:87-144) to assert on its own state.

Wanted:

- `expect(cond, msg?)`, `expect_eq(a, b)`, `expect_near(a, b, eps)`:
  record the failure with file and line and both values, and continue.
- `test "name" do ... end` blocks, and `petal test [path]` that finds them,
  prints a summary and sets the exit code.
- A way for a host to run the same blocks with its own natives present (the
  engine wants `cheesecake --test`).

**Done, 13 October 2026.** [docs/testing.md](../testing.md) is the reference.

- `expect(cond, msg?)`, `expect_eq(actual, expected, msg?)` and
  `expect_near(actual, expected, tolerance, msg?)` record the failure (file,
  line, column, both values, the message) and return whether they held.
  `assert` and `assert_eq` still raise.
- `test "name" do … end` at the top level of any file. The block is a
  function nothing calls in an ordinary run, so check, lint, fmt and the
  language server read its body with no special cases. `test` stays an
  ordinary name everywhere else.
- `petal test [path...]` runs each test on a fresh stack after the file's top
  level, with `--filter`, `--list`, `--show-output`, `--timeout` and
  `--json`. Exit status 1 on any failure.
- For the engine: `Env::tests`, `test_begin`, `test_resume` and
  `PetalCxt::suspend`. A native such as `wait_frames(n)` suspends the test;
  the host runs its frames on the same stack and resumes. That replaces the
  `if frame == N then` staging: a test that spans frames is written top to
  bottom. A waiting test reads `state var` cells live with `get`; a `let` or
  plain `state` it captured stays as it was when the test began.

Not done: a way to catch a runtime error inside a test (a test of a failure
still needs the host), setup and teardown blocks, and a call chain on a
failure recorded inside a helper (it names the helper's line only). Moses'
seven `dev/*_check.ptl` files and its `check PATH OP VALUE` command language
were not converted; that waits for `cheesecake --test`.

### 2.3 Profiler

- Profile from frame 0 on request. Boot is where Moses spends 1.8 s, and
  the report starts at frame 60.
- Attribute native time to the calling line, not only the calling function.
- Report allocations by site (records, lists, strings). GC is 6 to 12% and
  nothing says who allocates.
- Count per-call-site outliers: a "natives over 1 µs a call" list would
  have found the `text_width` cost (the former item 1) at once.

### 2.4 The stale CLI

`~/.cargo/bin/petal` on this machine is from 21 September. It rejects
`import ./x` and still has quadratic `append`. Two reviewers were misled by
it. `petal --version` should print the commit and build date, and a project
with a `petal.toml` could state the minimum version.

## 3. Language

### 3.1 A build-once constant (item 6)

Module top levels run every frame, so a top-level `let` table is rebuilt
every frame. The game's README tells every author: "Small constant tables
are fine; put big ones in `state NAME = [...]` (built once; F5 to rebuild
after an edit)". The result:

- 93 of the game's 447 module-level `state` cells are plain `state`, not
  `state var`: data that never changes, such as the map polygons (map.ptl:105-828), the event script (events.ptl:95), the
  news, the timeline.
- Editing that data does not hot-reload, because `state` keeps its value.
  Live coding is given up for exactly the content authors edit most.
- Tables that stayed `let` are paid for every frame: the agent tool list
  (agent.ptl:393-442, 36 records) is part of the 8% the module top levels
  cost.
- Guards such as `if T == nil then set T = blank() end` appear in most
  modules.

Wanted: `const NAME = expr`, evaluated once, re-evaluated when its
initializer's source (or a `const` it reads) changes, and frozen. The AST
diff from the hot-reload work already knows which declarations changed.
`writing-petal-guide.md` says literal lets are hoisted while the engine's
`petal-api.md` says they are recomputed; whichever is true should be stated
once, and `const` makes the question moot.

### 3.2 Checked record shapes (item 7)

The game passes a `ctx` record with about 40 fields to every function, an
`item` record with about 70, and options records to every widget. None of it
is declared. The consequences:

- The README documents each record by hand and says "kept accurate to the
  code".
- 966 uses of `??` and 324 of `field(r, k, fallback)`, because a missing
  field is a hard error and nobody knows which fields are present. One
  module wraps every read (legacy.ptl:33: "Every read has a fallback: a
  missing field is a zero").
- Consumers sniff shapes at run time:
  `len(mv.delta_nw ?? []) == n && len(mv.delta_inc ?? []) == n` (sim.ptl:551);
  `(ctx.hoods[0].capacity ?? -1.0) >= 0.0` to detect the real simulation
  (traffic.ptl:989).
- Records are zero-filled three times so that readers do not fail
  (build.ptl:666, :685, :727).
- A misspelt option key silently takes the default. Undocumented keys work
  by accident (`pad_right`, `edge`, `label_color`).

Wanted, in order of value:

```petal ignore
type Date = {year: int, month: int, tick: int}
type Ctx  = {date: Date, built: [Item], grid: Grid, turbo: bool, ...}

fn month(ctx: Ctx) ...
fn button(r: Rect, text: string, opts: {style = "plain", enabled = true, icon = ""}) ...
```

`petal check` reports an unknown field read, an unknown key in a literal
passed where a shape is expected, and a missing required field. Optional
fields with defaults declared in the shape remove most `??`. Shapes need not
affect the runtime at all.

### 3.3 Enums that can be used (item 8)

Item 8 was done in Petal on 12 October 2026 and is removed from the short
list. What the game found is kept here: stages, kinds, tones, panels, sound
names and command names are bare strings, 1,477 comparisons against a string
literal across the game. A typo falls through to a default (`icon` draws a
diamond, `named` plays a click). Parallel tables fake the enum: `T` and
`T_IDS`, `F` and `F_IDS`, `USE` and `USE_IDS`, `KINDS` and `KIND_IDS`
(config.ptl:113-163). The game used neither `enum` nor `match` because a
variant was a top-level name, so `park` could not be a `Kind`, a `Use` and a
`Tile` at once.

What shipped, against what was wanted (see
[language-guide.md](../language-guide.md#enums), "Enums" and "Pattern
Matching"):

- Qualified variants: `Kind.park`, and `config.Kind.park` through a module.
  The bare name is not allowed anywhere, including where the type is known:
  a bare `park` is an undefined name whose error suggests `Kind.park`.
- Exhaustive `match`, checked: a match over the variants of one enum that
  leaves one out does not compile. Arms take statement bodies and
  or-patterns (`when Kind.park | Kind.plaza ->`), so `match` can replace the
  `if`/`elsif` chains.
- Data on variants, replacing the parallel tables:
  `parkway(shape: "route", use: Use.road)`, read as `k.shape`, with
  `Kind.all`, `Kind.from("park")` and `k.name`.
- Variants with payloads, read by field name (`op.place`, `op.won`) instead
  of `len(op) > 4 && op[4]` (events.ptl:594).
- `match` on string literals, int literals and variants is entered through a
  hash jump: a 40-arm match went from about 410 ns to about 30 ns.

Cheesecake vendored it the same day, and nothing in the game broke, since it
declared no enum and every `match` it had ended in `when _ ->`. The game then
adopted it (cheesecake `docs/moses-debrief.md`, section 6): the four parallel
tables are enums with data, and 96 `if`/`elsif` chains are matches. What that
found:

- A data read on a written variant is not folded: `Fabric.pier.code` builds
  the variant and looks its data up by tag, about 70 ns against about 4 ns
  for a field of a record. The game keeps int tables built from the enums for
  its tile loops. `Kind.all` likewise builds its list on every read.
- `let F = config.Fabric` makes `F.rurl` an ordinary field read: the unknown
  variant is a runtime error, not a compile error, and `petal check` says
  nothing. Only the enum's own name (or `import ./config: Fabric`) is held to
  its variants.
- `3 == Use.road` is false without a warning; a grid of ints compared against
  variants would fail silently.
- `prefer-match` converts one-expression chains only, by choice. 55 of the 96
  chains had statement bodies, `||` alternatives or one `&&` condition (a
  guard), and were converted by a script.

### 3.4 Data structures (items 9 and 10)

**Int-keyed maps and sets.** `Cannot index-assign record with int`. The
hottest function in a career, `plan_tile` (buildings.ptl:1383, 9.5% of all
instructions), formats a string for every lookup:
`field(E.res_tiles, "{i}", false)`,
`field(reserved, "{tx * 3 + col}:{ty * 3 + row}", false)`. Tile sets are
`S["{t[0]},{t[1]}"] = true` in three modules. Counters round-trip through
`str(h)` and `parse_int(key)` (sim.ptl:328-333). Wanted: a map type with int
and tuple keys, and a set.

**Bit operators.** `&` does not lex. Three modules carry
`let POW2 = [1, 2, 4, ...]` and test bits with `(mask / POW2[b]) % 2 == 1`.

**Growable numeric arrays.** `f64_array(length)` exists, has no append, is
unused in the game, and the engine does not accept it. Mesh data goes
through lists of boxed floats. Wanted: `f64_array` and `i32_array` with
in-place `push` and `extend`, accepted by host natives as a pointer and a
length.

**A table of records.** `docs/syntax/var.md`: "`set ps[i].x = v` copies one
record". So traffic.ptl keeps its road network as 14 parallel
`state var E_*` lists; adding an edge is 14 appends and reset is 26 lines.
Wanted: either a columnar `table` type or ownership tracking one level down,
so that `set T[i].f = v` writes in place.

**Appending from a helper.** A function cannot append to its caller's list,
so every mesh builder is a module-level `var PARTS = []`, an emitter
`set PARTS = append(get PARTS, p)`, and a bare `nil` after it (79 `nil`
lines in buildings.ptl, 59 in structures.ptl; the `nil` is needed so the
`set` is not the function's value). Wanted: a statement `push xs, v` that
returns nothing, `extend(xs, ys)` in place, and a way to pass a builder.

**Tuples and destructuring.** Absent, so positional lists carry magic
indices: `f[7]`, `f[4]` for a box (buildings.ptl:2051), `sd[2 + m * 2]`
(terrain.ptl:389), `r[0]..r[4]` in the minimap.

### 3.5 Calling with a record of arguments (item 12)

The agent tool layer lists each tool's parameters as data and then reads
the same parameters by name in a 280-line `if name ==` chain
(agent.ptl:393-442 and :564-850). Defaults and ranges are written twice and
drift is caught only at run time (`fail("{name} is not wired up")`).
`fn_info()` can already describe a function; what is missing is
`f(...args)` for a record (by name) or a list (by position). With it, a
tool is one declaration:

```petal ignore
tool("toll", "Set the toll on a crossing.", fn(id: string, cents: int) ...)
```

The same call form would serve the game's command bus, which has six more
hand-written dispatch chains.

**Done, 13 October 2026.** `f(...args)` is in the language: a list binds by
position, a record by parameter name, after any positional arguments written
first (`f(a, ...rest)`). Defaults are honoured, and an unknown key, a missing
required parameter or one given twice is the written call's error, naming it.
Functions, closures, overload sets, module members, constructors, methods and
builtins (by their declared names) take one. It is an ordinary call — same
frame, same `state` slots per call site, same depth limit — not a nested
callback. `petal check` does not look inside a spread. See
[Calling with a list or record of arguments](../language-guide.md#calling-with-a-list-or-record-of-arguments),
which has the tool table written out with `fn_info`.

### 3.6 Module state (item 13)

"A `fn` captures module-level `let` / `state` by value when it is defined.
Anything a function reads live or writes is a module-level `state var`"
(the game's README). Hence 354 module-level `state var` cells and 1,013 `get`s.
`bus.ptl` is mostly `get` and `set`. Local `var` is then used for speed, and
closures over it need `get`, so authors snapshot into `let` first (the 51
dead aliases).

Suggestions:

- Read a module-level `state var` without `get` inside functions, since
  there is nothing else the name could mean there.
- Guarantee in-place `xs[i] = v` on a uniquely held local, so `var` is not
  needed for performance.
- `reset` for a module's state: every module hand-lists its cells in a
  `reset()` function, and one cell was already missed (events.ptl:579
  against :656-667). `state` groups, or "reset this module's state to its
  initializers", would remove about 300 lines and the bug class.
- Per-widget state keyed by an id (`state(key)`) exists and is unused; scroll
  positions are global strings instead. Document it for this use.

### 3.7 Names

- A module cannot be called `map`, `ui`, `log`, `lerp`, `list` or any other
  builtin or bundled package name. Moses renamed two files and imports
  `./map as citymap`. The README carries a list of names to avoid. A module
  file should be allowed any name, with the import site choosing the
  binding.
- `config let` names are global for `--set`, so every knob is prefixed by
  hand (`CAM_`, `BUILD_`, `AGENT_`, `MOS_`). Wanted: `--set cam.DIST=...`.
- "Was this knob overridden?" is tested against a copy of the default:
  `CAM_DIST != 6900.0 || CAM_PITCH != 62.0` (cam.ptl:97). Wanted:
  `config_overridden(NAME)`.
- `config let` carries no metadata. Ranges, choices and docs are restated
  in a JSON sidecar (`agent.json`) and again in CMake. Wanted: `config let
  SPEED = 1 in 0..3 "calendar speed"`. This is also the open "slider
  ranges" request from the editor work.

### 3.8 Memoization with a key

Call memoization is turned off in this game (`script_memo(false)`), as it
was in Neon: every function takes the whole `ctx`, so argument equality
never holds. In its place there are eleven hand-written caches in the UI
alone, each with an interpolated string key, a size cap that wipes
everything, and manual invalidation:

```petal
let key = "{get CAT}|{ctx.date.year}|{len(ctx.built)}|{ctx.power.n_offices ?? 0}|{ctx.grid.rev ?? 0}|{sig}"
```

Wanted: `memo(key) do ... end` with a bounded LRU, where `key` may be a
tuple of values compared by identity or revision rather than a string; and
`changed(value)` for the "run this when X moved" case, which the game spells
as a string compare in six modules.

## 4. Standard library (item 14)

Checked against `Builtins.md` and the CLI. Each is hand-written in the game
at least twice.

| Missing | Game's version |
|---|---|
| Number formatting: thousands separators, fixed decimals (`{x:.1f}` is a parse error) | `util.commas` builds the string a character at a time; `nice`, `fmt_m` three times |
| `plural(n, one, many)`, `list_and(xs)` | legacy.ptl:178-216, newspaper.ptl:172 |
| `min_by`, `max_by`, `argmin`, `top_k` | five argmin loops in traffic.ptl; a full sort for top-k |
| `group_by`, `count(xs, p)` | `append(field(by, s.kind, []), ...)` (models.ptl:1402) |
| `fill(n, v)`, list `repeat` | `for i in range(0, n) do 0.0 end` at 49 sites; `zeros()` twice |
| `interp(table, x)` (piecewise linear by year) | `curve` twice |
| `ord`, integer hash, `hash_str` | `rng.ptl`: four multiply-mods per value; `index_of(ALPHA, c)` for a character code |
| `flat_map`, a multi-generator `for` | 43 `flat(for ...)` |
| A priority queue | a label-correcting loop over an appended list (traffic.ptl:830-852) |
| Polyline `resample`, `simplify`, `round_corners` | models.ptl:808-907 |
| Record-wise `lerp` | fifteen fields interpolated by hand (atmosphere.ptl:138) |
| Computed record keys `{[k]: v}` | the README restates the workaround |

Present and unused, which is a documentation problem: `any`, `sum`, `find`,
`color_lerp`, `r?.field`, `text_wrap`, `text_ellipsize`, `set xs[i] += v`,
default and named parameters, `assert`. A one-page "before you write a
helper" index in `writing-petal-guide.md`, and the lint in 2.1, would
address it.

## 5. petal-ui (item 11)

`widgets.ptl` (1,246 lines) reimplements `button`, `tab_bar`, `tooltip`,
`modal`, `progress_bar`, list and scroll handling, `card`, colour mixing and
text fitting, all of which the prelude has. The reasons are real gaps:

- **No layers or modal inertness.** "Immediate mode has no z order, so the
  modules say which storey they draw on" (widgets.ptl:13). The game keeps a
  list of claimed rectangles, allocated per widget per frame, and tests the
  3D click against last frame's list, which leaks one click when a panel
  opens under the pointer. Callers must remember `claim`, `live()` and
  `hovered` separately and do so inconsistently.
  Wanted: `ui_layer(n)`, `ui_modal()`,
  `interact(rect) -> {hover, down, click}` that respects layer and clip and
  registers the rectangle, and `pointer_captured()`.
- **No retained drawing.** A canvas lives one frame. The minimap, a baked
  image that never changes, is re-emitted as several hundred `draw_rect`
  calls every frame: 11.5% of all instructions. The front page, the title
  poster and the halftone screens are the same.
  Wanted: `cached_layer(key, rect, fn)` that re-runs its body only when the
  key changes, and a batch `draw_rects(list, palette)`.
- **No layout.** About 2,000 lines of cursor arithmetic (`y += 17`), column
  tables, and scroll heights taken from the previous frame or from a second
  measuring pass that duplicates the draw pass. The tutorial re-derives
  other modules' button positions by hand (title.ptl:343).
  Wanted: a flow cursor, a measure-only mode, and named widget rectangles
  that tests and tutorials can ask for.
- **Text styles.** No shadow, outline or tracking, so strings are drawn
  twice and a display face was drawn in strokes.
- **Options.** 47 call sites end in `{})`.

The prelude's existing widgets are documented nowhere a game author looks:
the engine's API document mentions only `button`.

## 6. Embedding (item 15)

From the headless-mode debrief (`~/cheesecake/docs/headless-mode-debrief.md`)
and this pass; all still open:

- `pb_vm_save_state` / `pb_vm_load_state`: the CLI has `--state-storage`;
  the bridge can dump state and cannot load it. Blocks O(1) save points,
  undo and forks for agent sessions.
- A failed host call leaves its frames on the stack; later traces repeat
  them (`call_function_diag` error path).
- JSON across `pb` calls is loosely typed: whole floats become ints, and
  functions flatten to strings. A schema from `fn_info` would let the bridge
  coerce.
- Natives still cannot learn their call site.

## 7. What worked

Worth keeping in mind when changing things: seven teams wrote 27,000 lines
in three days with no crash-class fix in the history; determinism held
across every replay; hot reload stayed usable; `petal check` found real
bugs when it was run; and three `check` false positives found by the game
were fixed upstream within the day.

## 8. Is `const` sufficient?

Written 13 October 2026, after `const` and the static-value cache
(`docs/proposals/static-values.md`, `docs/tasks/static-value-optimization.md`)
and Cheesecake's adoption of them (its `docs/moses-debrief.md`, section 7).

**Against what 3.1 asked for: yes.**

| Wanted | Have |
|---|---|
| Evaluated once | A top-level value that depends on nothing dynamic is built on the first run and skipped after it. Under the engine 529 of Moses' 530 constants are untouched by a typical edit |
| Re-evaluated when its initializer's source, or a `const` it reads, changes | The reload follows the diff: an edit rebuilds the constants it reaches, through the functions that build them, and the report names them. Editing a table that was parked in `state` now shows on the next frame; before, it needed F5 |
| Frozen | A kept value is in the frozen generation: a write to it copies |
| `const` as a promise | A `const` that is not static data is a compile error with the chain of reasons. The host reports it when the reason is one of its natives |

It needs a host that runs with the cache on (Cheesecake does) and that
declares its pure natives constant (`PB_FX_CONST`). With the cache off a
`const` is a `let`, built on every run.

**The three gaps Cheesecake found.**

1. **`const X = append(for … end, Y)` was quietly not kept. Fixed, as a
   diagnostic.** The list handed to `append` gets no slot (a value that may
   be written in place is never kept), so the comprehension ran on every
   frame beside a kept `X`. The compile now warns on a `const` any part of
   whose build is left out for that reason, naming the part (`the value
   written on line 3 is handed to append`), the `const` itself (`T is passed
   to append`), or the `let` it is built from, and says to use `concat` or
   `++`. Keeping such a value instead was not attempted: it rests on the
   frozen-generation guard alone, where today three guards overlap, and the
   in-place analyses are told the plan before lowering. None of the fifteen
   games has such a `const` now.
2. **A cache in `state` filled from a constant was not named by the reload
   report. Fixed.** The report has a `stale cache:` line
   (`StaticReload::stale_caches`) for a `state` that code fills behind a
   test of the cell itself (`if get X == nil then set X = build() end`) or
   of a key remembered beside it, at the top level or in a function, when a
   declaration its cached value is built from was changed by the edit. A
   cell written on every run is not named, which is the false report
   `4269f9f` removed. It covers the shapes of Moses' three (`map.ptl`'s
   grid, `projects.ptl`'s `DEFS_C`, the overlay bake behind a remembered
   key) in this repository's tests; it has not been run under the engine.
   What it cannot see: a guard that is not an `if` on a cell (a `match`, a
   timer compared against game time), and a cache kept in a record field or
   a list element rather than in a cell of its own.
3. **`Kind.all` and variant data rebuilt per read: no longer so.** `Kind.all`
   answers with the one list the stack keeps for the enum, and a variant's
   data is read from its published record. Measured here over ten million
   calls of a function that does the read: a record field 62 ns a call, a
   variant's data 71 ns, `len(Kind.all)` 114 ns. So a data read costs about
   9 ns more than a field, and `Kind.all` costs one builtin call (about 50
   ns), not a list. Cheesecake's own figure is two instructions against one,
   and +0.5 to 0.7% on a run with 220 sites converted. Not changed here. If
   it matters, `__enum_all` is a candidate for the intrinsic lane the
   trivial builtins got.

**What remains.**

- The honest fix for a stale cache is to make the cached value a `const`.
  That is blocked where the build is slow and the cache is off (Moses' map:
  a one-second build on every frame), and where the value is then written
  (a grid the game edits). A build-once value that is *not* required to be
  static, rebuilt when what it reads changes, would cover it: `const` with
  the report's dependency walk, minus the purity rule. Not designed.
- A patched reload (a literal changed, no recompile) is not type-checked
  again, so a warning that depends on a literal's value does not appear
  until the next recompile.
- `let F = config.Fabric` still makes `F.rurl` a field read (3.3), and
  `3 == Use.road` is still false without a word. Both are check gaps, not
  `const` gaps.

