# Direct manipulation v2: the examples ladder

Programs of increasing complexity whose shapes can be dragged with direct
manipulation v2 (goal-based backpropagation through function calls,
[docs/dev/direct-manipulation-v2-plan.md](../../docs/dev/direct-manipulation-v2-plan.md)).
Each file says in its header comment what to drag and what edit it should
produce, in lines the study harness reads. The results, and what makes the
harder ones fail, are in
[docs/dev/direct-manipulation-v2-study.md](../../docs/dev/direct-manipulation-v2-study.md).

| File | What it exercises |
|---|---|
| `l00-literal.ptl` | a literal written in the draw call (v1 parity) |
| `l01-binding.ptl`, `l01b-binding-config.ptl` | a `let` feeding two shapes; the same with `config let` |
| `l02-helper.ptl` | one helper function (`center + offset`) |
| `l03-nested.ptl` | three nested helpers with a default parameter |
| `l04-clock-hand.ptl` | trig: a clock hand at a fixed hour |
| `l04b-orbit-time.ptl` | trig plus `time()`: the rate must not be picked |
| `l05-loop-row.ptl` | a row of shapes from a `for` loop |
| `l05b-map-running-sum.ptl` | `reduce` with a running sum of widths |
| `l06-records.ptl` | a list of shape records drawn by a dispatcher |
| `l07-fractal-tree.ptl`, `l07b-fractal-tree-hints.ptl` | recursion: drag a leaf; the same with hints |
| `l08-flex-row.ptl` | a flex-row-like layout from `text_width` and padding |
| `l09-projectile.ptl`, `l09b-…-pure.ptl`, `l09c-…-hints.ptl` | 200 Euler steps with `var`; the same as a fold; the same with `rigid` |
| `l10-bar-chart.ptl` | a bar chart with an auto-scaled axis |
| `l10b-ui-panel.ptl` | a settings card from the `ui` layout helpers and widgets |
| `a01-rand.ptl` | `random()` scatter |
| `a02-state.ptl`, `a02b-state-fresh.ptl` | a `state` animation at a live checkpoint; on a fresh stack |
| `a03-var.ptl` | a `var` accumulator in a loop |
| `a04-branch-flip.ptl` | a branch that flips under the drag |
| `a05-integer.ptl` | `round`, `floor`, int division |
| `a06-shared-literals.ptl` | one binding shared by every shape |
| `a07-hints.ptl` | `tunable` ranges and steps, `rigid` |
| `big-server-monitoring/` | a verbatim copy of `examples/dashboards/server-monitoring` (944 lines, 1,500 emits) |

Each file runs on its own in any petal-ui host (`petal-ui-run`, Garden);
`big-server-monitoring` is a Garden panel app.

## The header format

```petal
// dm-title: a short description
// dm-level: L3
// dm-frames: 30          (frames to run before the checkpoint; default 1)
// dm-state: fresh        (StateMode::Fresh instead of Frozen)
// dm-panel               (register Garden's panel stubs)
// dm-size: 1440x900      (default 800x600)
// dm-goal <name>: <target>[, <target>…] [| <options>]
```

A target is `tag#k.arg[.field] (+=|-=|=) number`: raw argument `arg` of the
`k`-th draw command with that tag (`circle`, `rect`, `line`, `text`,
`rect_outline`, … ; `*` counts every command), before the native truncates it
to an int. `rect#3.0.x` reads field `x` of a rect record argument. Several
targets make a multi-goal drag (`circle#2.0 += 16, circle#0.0 += 0` is "move
dot 2 right, keep dot 0 where it is").

Options:

- `expect=a,b,…`: the top alternative may change only these. A token is a
  binding name, `lit:<text>` (a literal as written), `fn:<name>` (a literal
  in that function's body), `L<n>` (a line of the entry file), `config`, or
  `none`.
- `allow=…` / `deny=…`: the client's filter (`var:x`, `fn:f`, `line:n`,
  `config`, `all`, `state-init`, `structural`, `rng`).
- `strict` (`Prefer::ConfigStrict`), `respell`, `tol=<px>` (default 0.5),
  `alts=<n>`, `runs=<n>`.
- `xfail=<reason>` (last): an expected failure, with why. The study reports
  it; the test does not fail on it.

## Running the study

From `core-libs/petal-ui`:

```sh
cargo test --release --test dm_v2_study ladder -- --nocapture
DM_STUDY_ONLY=l07 DM_STUDY_VERBOSE=1 cargo test --release --test dm_v2_study ladder -- --nocapture
```

`DM_STUDY_DUMP=1` prints each example's draw list (to find `tag#k`),
`DM_STUDY_VERBOSE=1` every alternative, its score and the candidate columns,
`DM_STUDY_EXPLAIN=1` the solver's explain log. The full study, which
rewrites the report, is in the study doc.
