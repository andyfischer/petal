# Differentiation and solving: the `autodiff` package

"If I nudge this input, how far does that output move?" and "what inputs make
this output hit that value?" are answered in Petal by running the code again
with an input nudged and measuring what changed (finite differences), then
stepping toward the goal (Gauss-Newton). `import autodiff` gives four
functions that do it for any function you hand them:

```petal
import autodiff

fn f(x)
  x * x + 2.0 * x
end

let d = deriv(f, 3.0)
print(round(d.value, 6))   # 8.0
print(d.quality)           # smooth
```

`8.0` is `2x + 2` at `x = 3`. `f` is ordinary code: it is called with plain
numbers and knows nothing about derivatives. The result is a record, because
a measured derivative comes with a verdict on how far to trust it
(`quality`) and the step it was measured with.

This is the same machinery [direct manipulation](direct-manipulation.md)
uses to turn a drag into a source edit: the same rules classify each
measurement, and the same Gauss-Newton step moves the inputs. The signatures
are in [Builtins.md](../Builtins.md#automatic-differentiation).

- [The functions](#the-functions)
- [`deriv`](#deriv)
- [Quality: how far to trust a derivative](#quality-how-far-to-trust-a-derivative)
- [`jacobian`](#jacobian)
- [`solve_step` and `solve`](#solve_step-and-solve)
- [Functions that touch `state` or draw](#functions-that-touch-state-or-draw)
- [Options](#options)
- [Cost](#cost)
- [Limits](#limits)
- [How it relates to direct manipulation](#how-it-relates-to-direct-manipulation)
- [Migrating from `dual`, `value_of` and `deriv_of`](#migrating-from-dual-value_of-and-deriv_of)

## The functions

| Function | What it answers | Returns |
|---|---|---|
| `deriv(f, x, opts?)` | The slope of `f` at `x` (one number in) | `{value, quality, step}` |
| `jacobian(f, xs, opts?)` | Every input's effect on every output (a list in) | `{rows, quality, step}` |
| `solve_step(f, xs, target, opts?)` | One step of `xs` toward `f(xs) == target` | `{xs, residual, improved, stuck, quality}` |
| `solve(f, xs, target, opts?)` | `solve_step` repeated until the goal is met | as `solve_step`, plus `iterations` |

`f` may return a number or a list of numbers. Like any package function,
these can be called bare, qualified (`autodiff.deriv(f, 1.0)`) or as methods.

## `deriv`

`deriv(f, x)` calls `f` at `x`, then at `x + h` and `x - h`, and divides
(central differences). The step `h` is `1e-4 * max(|x|, 1)`, so it scales with
the input; pass `{h: …}` to choose it. An int `x` is probed as a float.

```petal
import autodiff

print(deriv(sin, 0.0).quality)   # smooth
print(deriv(sin, 5.0).step)      # 0.0005
```

The value is an approximation. For `atan` at `1.0` it is
`0.5000000008337224`, not `0.5`: round it, or compare within a tolerance,
when you print or test it. If `f` returns a list, `value` is a list, one slope
per element.

## Quality: how far to trust a derivative

Code is only piecewise smooth: an `abs`, an `if` or a `floor` breaks it into
pieces, and a difference taken across a break measures the break, not a
slope. So `deriv` probes a little more where the first two probes disagree,
and says what it found:

| `quality` | Meaning | `value` |
|---|---|---|
| `"smooth"` | Both sides agree (or differ only by the curve's bend, as at the bottom of `x * x`) | The central slope |
| `"kink"` | Both sides are slopes, but different ones: `abs(x)` at 0, `min`, `max`, `clamp` at a bound | The central slope (their average) |
| `"branch_flip"` | One side jumps: it took another branch of an `if`, or is not a number there (`sqrt` left of 0) | The slope of the side that didn't, after trying a step ten times smaller |
| `"step"` | Nothing moved at the first step, but something did at a larger one: `floor`, `round` | A secant over the jump |
| `"zero"` | Nothing moved at any step tried | `0.0` |
| `"errored"` | `f` returned something that is not a number; the record also has `error` | `0.0` |

```petal
import autodiff

print(deriv(abs, 0.0).quality)                                         # kink
print(deriv(fn(x) if x < 1.0 then x else x + 5.0 end end, 1.0).quality)   # branch_flip
print(deriv(floor, 0.97).quality)                                      # step
print(deriv(fn(x) 7.0 end, 2.0).quality)                               # zero
```

`step` is the `h` the value was finally measured with: smaller than asked
after a branch flip, larger after a step search (which grows the step ten
times at a time, up to a tenth of `max(|x|, 1)`).

A `"smooth"` or `"kink"` value is a slope you can step along. A
`"branch_flip"` value is a slope on one side only, a `"step"` value is how far
a jump carries the output, not a slope, and a `"zero"` input cannot move the
output from here at all.

## `jacobian`

For a function of several inputs, pass them as a list. `jacobian` measures
one column per input (two runs each, plus one at the point), and returns
`rows[i][j]`, how output `i` moves per unit of input `j`, with one `quality`
and one `step` per input:

```petal
import autodiff

fn tip(p)
  let cx = p[0]
  let length = p[1]
  let angle = p[2]
  [cx + length * cos(angle), length * sin(angle)]
end

let j = jacobian(tip, [200.0, 80.0, 0.6])
print(map(j.rows, fn(r) map(r, fn(v) round(v, 3) end) end))   # [[1.0, 0.825, -45.171], [0.0, 0.565, 66.027]]
print(j.quality)   # ["smooth", "smooth", "smooth"]
```

The tip of the clock hand moves one pixel right per pixel of `cx`, 0.83
pixels per pixel of `length`, and 45 pixels left per radian of `angle`. A
function that returns one number has one row, so its gradient is
`rows[0]`:

```petal
import autodiff

fn bowl(p)
  let dx = p[0] - 1.0
  let dy = p[1] + 3.0
  dx * dx + 2.0 * dy * dy
end

let p = [0.0, 0.0]
for step in range(0, 40) do
  let g = jacobian(bowl, p).rows[0]
  p = [p[0] - 0.2 * g[0], p[1] - 0.2 * g[1]]
end
print(round(p[0], 3), round(p[1], 3))   # 1.0 -3.0
```

## `solve_step` and `solve`

Most of the time the derivative is a means to an end: you want the inputs
that put an output somewhere. `solve_step(f, xs, target)` measures the
Jacobian at `xs` and takes one damped Gauss-Newton step toward
`f(xs) == target`; `solve` repeats it until the output is within `tol` of the
target. `target` is a number or a list, matching what `f` returns.

```petal
import autodiff

fn tip_x(a)
  200.0 + 80.0 * cos(a[0])
end

let s = solve(tip_x, [0.6], 240.0)
print(round(s.xs[0], 4))   # 1.0472
print(s.iterations)        # 4
```

The result has:

| Field | Meaning |
|---|---|
| `xs` | The new inputs, as floats. Unchanged when no step got closer. |
| `residual` | The distance from `f(xs)` to `target` (Euclidean, over the outputs) at the returned `xs` |
| `improved` | Whether the step brought the output closer. For `solve`, whether any step did. |
| `stuck` | The indices of inputs that do not move the output here (`"zero"` or `"errored"` columns). They are left where they are, not divided by. |
| `quality` | The Jacobian's quality, one per input, so a stuck input (`"zero"`) can be told from one that jumped a branch |
| `iterations` | (`solve` only) How many steps it took |

When there are more inputs than outputs, the step moves each input in
proportion to its size. A step that does not bring the output closer is
halved, up to four times; when none of those does either, `xs` comes back
unchanged with `improved: false`, and `solve` stops.

Per-frame use, as in an aiming game, takes one `solve_step` a frame, with
bounds and a cap on how far each input moves:

```petal ignore
let r = solve_step(
  fn(xs) at(xs[0], xs[1], k) end,          # [x, y] where the shot lands
  [angle, speed],
  [target.x, target.y],
  {bounds: [[0.05, 1.5], [80.0, 900.0]], max_step: [0.06, 30.0], damping: 0.25},
)
angle = r.xs[0]
speed = r.xs[1]
```

## Functions that touch `state` or draw

Every probe runs inside [`speculate`](speculate.md): whatever `f` writes to
`state` or to a `var`, whatever it draws or pushes to an output buffer, and
any random number it draws is put back when the probe returns, and a native
whose effect escapes (saving a file, playing a sound) is not called. So you
can differentiate a whole view or one step of a simulation, not just an
arithmetic helper.

```petal
import autodiff

var count = 0
fn view(x)
  set count = (get count) + 1
  x * 3.0
end

print(round(deriv(view, 1.0).value, 6))   # 3.0
print(count)                              # 0
```

A `state` slot first reached inside a probe starts from its initializer in
every probe ([`state` inside the call](speculate.md#state-inside-the-call)),
so each probe sees the same starting point.

`speculate` does not nest. When `deriv` and the others are called from code
that is already speculating (inside a `ui/drag` candidate rendering, or a
`speculate` call of your own), they call `f` directly: the outer call already
puts everything back.

If `f` raises an error, the error propagates out of `deriv`, as from any
call. Only a result that is not a number is reported as `"errored"`.

## Options

Every function takes an optional record of options last:

| Option | Functions | Default | Meaning |
|---|---|---|---|
| `h` | all | `1e-4 * max(abs(x), 1)` per input | The first step |
| `mode` | all | `"central"` | `"central"` (two runs per input) or `"forward"` (one run, `x + h` against the point). Forward is half the runs, less accurate, and cannot see a jump on the side it does not probe. |
| `bounds` | `solve_step`, `solve` | none | `[[lo, hi], …]`, one per input; an entry or either end may be nil |
| `max_step` | `solve_step`, `solve` | none | How far one step may move each input: a number for all, or a list with one per input |
| `damping` | `solve_step`, `solve` | `1.0` | The step's scale; `0.25` takes a quarter of the Gauss-Newton step |
| `tol` | `solve` | `1e-6` | Stop when `residual` is at most this |
| `max_iter` | `solve` | `20` | Stop after this many steps |

## Cost

The cost is counted in runs of `f`:

| Call | Runs of `f` (central, smooth) |
|---|---|
| `deriv` | 3: the point and two probes |
| `jacobian` over `n` inputs | `1 + 2n` |
| `solve_step` over `n` inputs | `1 + 2n`, plus 1 to verify (up to 5 when it halves the step) |
| `solve` | `solve_step`'s, times `iterations` |

`mode: "forward"` makes the probes one per input instead of two. A kink, a
branch flip or a step search costs a few probes more, for that input only.

Each probe also pays for the `speculate` call around it: its fixed cost
(saving and restoring what a call could change) is under a microsecond, which
is noise next to a view or a simulation step, and noticeable only for a
two-line arithmetic `f` called in a tight loop.

## Limits

- **Approximate.** A central difference is off by about `h²` times the third
  derivative; `deriv(atan, 1.0).value` is `0.5000000008337224`. Round or
  compare within a tolerance. Exact derivatives are planned to return as a
  symbolic analysis of the code, answering through the same functions (with
  a `quality` of `"exact"`).
- **The quality is a judgement from samples.** A jump smaller than the
  output's float noise, or a kink sharper than one step, can pass as
  `"smooth"`. In `"forward"` mode a jump to the left of the point is not
  seen.
- **First derivatives only.** For a second derivative, differentiate a
  function that calls `deriv`.
- **No reverse mode.** A gradient over `n` inputs costs `n` columns.
  `solve` is meant for a handful of inputs, like direct manipulation's
  solver, not for training a model.
- **Errors propagate.** A run of `f` that raises an error fails the whole
  call.

## How it relates to direct manipulation

[Direct manipulation](direct-manipulation.md) (`petal propose-edit`, Garden's
drag mode) asks the same question of the source code: which literals, and by
how much, move a drawn shape to where the author dragged it. It re-runs the
whole program with a literal nudged, and classifies each column with the
same rules `deriv` uses (they are one implementation, in
`core/src/dm/jacobian.rs`, reached from Petal through the `fd_column`
native). DM's runs also record which branches each run took, so it sees a
branch flip directly; a script's probes have no such record, so `deriv`
tells a jump from a kink, and a kink from a curve's bend, by one more probe at
a tenth of the step.

`solve_step`, `ui/drag`'s [`vary`](../../core-libs/petal-ui/docs/drag.md) and
DM's path levers take their steps through one kernel, the `gn_step` native.
The difference is what moves: `solve` returns new values for your inputs,
while direct manipulation proposes an edit to the source.

## Migrating from `dual`, `value_of` and `deriv_of`

Earlier versions of `autodiff` had a number kind that carried a derivative
through arithmetic. It was removed (docs/tasks/differentiation-rewrite.md),
and `dual`, `value_of` and `deriv_of` are now a compile error that points
here. The usual shapes translate directly:

| Before | Now |
|---|---|
| `deriv_of(f(dual(x, 1.0)))` | `deriv(f, x).value` |
| One `dual(…, 1.0)` seed per input, one pass each | `jacobian(f, xs).rows` |
| A Newton step written by hand from `deriv_of` | `solve_step(f, xs, target)` |

The answer is now a close approximation, not the exact derivative; check
`quality` to see how far to trust it. A `state` slot saved by
`--state-storage` with one of the old numbers in it loads as its value, with
a warning, for one release.

## See also

- [Builtins.md, Automatic Differentiation](../Builtins.md#automatic-differentiation):
  the signatures.
- [`speculate`](speculate.md): what a probe puts back.
- [Direct manipulation](direct-manipulation.md): the same solver, editing
  source literals.
- [`ui/drag`](../../core-libs/petal-ui/docs/drag.md): `vary`, a drag that
  solves for state paths every frame.
