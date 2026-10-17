# Differentiation: dual numbers and the `autodiff` package

A **dual number** is a number that carries a derivative along with its value.
Mark one input of a computation as the thing you are differentiating with
respect to, run the computation as usual, and the result comes out holding
both the answer and how fast the answer changes as that input changes.

```petal
import autodiff

let x = dual(3.0, 1.0)        // the value 3, marked as the input: dx/dx = 1
let y = x * x + 2.0 * x       // ordinary arithmetic: y = x^2 + 2x

print(value_of(y))   // 15.0
print(deriv_of(y))   // 8.0
```

`8.0` is `2x + 2` at `x = 3`. Nothing in the second line knows about
derivatives: the same code runs on a plain `3.0` and gives a plain `15.0`.
This is forward-mode automatic differentiation. The derivative is exact (it is
not a finite difference), and it is computed in the same pass as the value.

This page is the reference for differentiation as a script author sees it.
The signatures are in [Builtins.md](../Builtins.md#automatic-differentiation),
and the implementation notes are in
[dev/Architecture.md](../dev/Architecture.md#differentiation-forward-mode).

- [The `autodiff` package](#the-autodiff-package)
- [How a derivative moves through arithmetic](#how-a-derivative-moves-through-arithmetic)
- [Which builtins carry a derivative](#which-builtins-carry-a-derivative)
- [Comparisons and control flow](#comparisons-and-control-flow)
- [Differentiating a function](#differentiating-a-function)
- [Several inputs](#several-inputs)
- [Worked example: gradient descent](#worked-example-gradient-descent)
- [Worked example: how far does this output move?](#worked-example-how-far-does-this-output-move)
- [Supplying a missing rule](#supplying-a-missing-rule)
- [The `dual` type](#the-dual-type)
- [Cost](#cost)
- [Limits](#limits)
- [Differentiation and the other tools](#differentiation-and-the-other-tools)

## The `autodiff` package

`import autodiff` brings three functions into scope. Dual numbers themselves
are a built-in kind of value: every runtime has them, and arithmetic on them
needs no import.

| Function | Result |
|---|---|
| `dual(value, derivative)` | A dual number. Both arguments are an `int` or a `float`, and both are stored as floats. |
| `value_of(x)` | The value part, as a float. A plain number comes back as a float: `value_of(5)` is `5.0`. |
| `deriv_of(x)` | The derivative part, as a float. A plain number has derivative `0.0`. |

Like any package function they can be called bare, qualified
(`autodiff.dual(3.0, 1.0)`) or as a method (`y.deriv_of()`).

The **seed** is the second argument of `dual`. `dual(x, 1.0)` says "this is
the input I am differentiating with respect to". `dual(x, 0.0)` is a constant
that happens to be a dual, and behaves like the plain number `x`. Any other
seed scales the result: the derivative that comes out is the derivative with
respect to the input, times the seed.

A dual prints as `dual(value, derivative)`, and `type(x)` is `"dual"`. It is
truthy when its value is not zero. `value_of` and `deriv_of` raise an error on
anything that is not a number, and so does `dual` when either argument is
already a dual.

## How a derivative moves through arithmetic

When either operand of `+`, `-`, `*`, `/` or `%` is a dual, the result is a
dual. A plain number on the other side counts as a constant (derivative 0).

| Operation | Value | Derivative |
|---|---|---|
| `a + b` | `a + b` | `a' + b'` |
| `a - b` | `a - b` | `a' - b'` |
| `a * b` | `a * b` | `a' * b + a * b'` |
| `a / b` | `a / b` | `(a' * b - a * b') / (b * b)` |
| `-a` | `-a` | `-a'` |
| `a % b` | `a % b` | `a'` (the divisor's derivative is ignored) |

```petal
import autodiff
let x = dual(2.0, 1.0)

print(deriv_of(x + 1))       // 1.0
print(deriv_of(3 - x))       // -1.0
print(deriv_of(x * x * x))   // 12.0
print(deriv_of(3 / x))       // -0.75
```

Dividing by a dual whose value is zero is the error `Division by zero`, as it
is for plain numbers. Arithmetic between a dual and anything that is not a
number is an error: `Cannot perform arithmetic on dual and string`.

A dual is an ordinary value. It can be bound, passed to a function, returned,
kept in a list or a record, and kept in [`state`](inline-state.md). So
derivatives flow through your own functions, through loops and recursion, and
through `map`, `sum` and `mean`, with no change to the code:

```petal
import autodiff
let x = dual(2.0, 1.0)

print(deriv_of(sum([x, x, 1.0])))   // 2.0
print(deriv_of(mean([x, 4.0])))     // 0.5
```

## Which builtins carry a derivative

A builtin has to know its own derivative rule to pass a derivative on. The
math builtins do. A builtin or host function that has no rule raises an
error when it is handed a dual, rather than read the value and lose the
derivative:

```text
Expected float at arg 1, got dual: this function has no derivative rule, so the derivative would be lost (pass value_of(x) to use the value alone)
```

**Carry the derivative (the chain rule is applied):**

| Builtin | Derivative |
|---|---|
| `sin(x)`, `cos(x)`, `tan(x)` | `cos(x)`, `-sin(x)`, `1 / cos(x)^2` |
| `asin(x)`, `acos(x)`, `atan(x)` | `1 / sqrt(1 - x^2)`, `-1 / sqrt(1 - x^2)`, `1 / (1 + x^2)` |
| `atan2(y, x)` | `(x * y' - y * x') / (x^2 + y^2)`, and `0` at the origin |
| `sqrt(x)` | `1 / (2 * sqrt(x))`, and `0` at `x = 0` |
| `hypot(x, y)` | `(x * x' + y * y') / hypot(x, y)`, and `0` at the origin |
| `exp(x)`, `log(x)` | `exp(x)`, `1 / x` |
| `pow(a, b)` | `b * a^(b - 1) * a' + a^b * log(a) * b'`. Each term counts only when its input is a dual with a nonzero derivative, so `pow(x, 3)` of a negative `x` never takes `log` of it. |
| `abs(x)` | `1` above zero, `-1` below, `0` at exactly zero |
| `fract(x)` | `1` (between the jumps) |
| `floor(x)`, `ceil(x)`, `round(x)`, `round(x, places)`, `sign(x)` | `0` (they are step functions). The result is still a dual. |
| `radians(x)`, `degrees(x)` | `pi / 180`, `180 / pi` |
| `lerp(a, b, t)` (package `creative`) | Of `a + (b - a) * t`, in all three arguments. A list or record `lerp` carries the derivatives of its elements; its `t` must be a plain number. |
| `map_range(v, in_lo, in_hi, out_lo, out_hi)` (`creative`) | Of the linear map, in all five arguments |
| `smoothstep(e0, e1, x)` (`creative`) | `6t(1 - t)` times the derivative of `t = (x - e0) / (e1 - e0)` inside the edges, `0` outside |
| `interp(table, x)` | The slope of the segment `x` is on, `0` past either end |
| `mag(x, y)`, `mag(x, y, z)`, `mag(x)`, `distance(x1, y1, x2, y2)` (`creative`) | Of the square root of the sum of squares |
| `float(x)` | Returns the dual unchanged. |
| `sum(list)`, `mean(list)` | They add and divide with the ordinary operators. |
| `safe_div(a, b)` | As `/`, and `nil` when the divisor's value is zero. |

A rule that takes several arguments returns a dual when any argument is one,
and a plain float when none is. A plain number counts as a constant.

**Return one argument unchanged:** `min(a, b)` and `max(a, b)` compare by
value and hand back whichever argument won, with whatever derivative it had.
So `max(x * x, 3.0)` is the dual `x * x` when that is larger and the plain
`3.0` when it is not. On a tie the first argument is returned. `clamp(v, lo,
hi)` does the same with a bound, so inside the range the result carries
`v`'s derivative and outside it the bound's. A clamp with a dual argument
always returns a dual.

**Refuse a dual:**

| Builtin | Error |
|---|---|
| `vec2(x, y)`, `vec3(x, y, z)` | ``vec2() cannot hold a dual number (argument 1): a vector stores plain floats, …``. Keep duals in lists or records and do the vector arithmetic per component. |
| `int(x)` | `Cannot convert dual to int` |
| Any other builtin or host function that reads a float: `random`, `noise`, a host's drawing calls | ``Expected float at arg N, got dual: this function has no derivative rule, …``. Pass `value_of(x)` to use the value alone, or [supply the rule](#supplying-a-missing-rule). |
| Anything that needs an integer, such as a list index or a bound of `range` | The usual type error |

Since vectors cannot hold a dual, `dot`, `normalize` and the other vector
builtins never see one. Arithmetic between a dual and a `vec2`, a `vec3` or
a list is also an error. The list operators that broadcast a plain number
over a list (`[1.0, 2.0] * 2`) do not accept a dual. Use `map`:

```petal
import autodiff
let x = dual(2.0, 1.0)

print(map([1.0, 2.0, 3.0], fn(c) c * x end))   // [dual(2.0, 1.0), dual(4.0, 2.0), dual(6.0, 3.0)]
```

When a derivative comes out as `0.0` and should not have, look for a step
function (`floor`, `round`, `sign`), a clamp or `smoothstep` past its edge,
or a branch that returns a constant on the path from the input to the
output.

## Comparisons and control flow

`==`, `!=`, `<`, `<=`, `>` and `>=` compare a dual by its value and ignore
its derivative. A dual equals the plain number with the same value, and two
duals with the same value are equal whatever their derivatives. `sort` orders
duals by value.

```petal
import autodiff

print(dual(2.0, 1.0) == 2)                 // true
print(dual(2.0, 1.0) == dual(2.0, 0.0))    // true
print(dual(1.0, 9.0) < dual(2.0, 0.0))     // true
```

So an `if`, a `match` guard or a loop condition that tests a dual picks a
branch exactly as it would for the plain value, and the derivative you get is
the derivative of the branch that ran:

```petal
import autodiff

fn leaky(v)
    if v > 0 then v else v * 0.1 end
end

print(deriv_of(leaky(dual(2.0, 1.0))))    // 1.0
print(deriv_of(leaky(dual(-2.0, 1.0))))   // 0.1
```

At the point where the branches meet (`v == 0` here) the function may have no
derivative, and you get the one from whichever branch the comparison chose. A
branch that returns a constant returns a plain number, whose derivative is
`0.0`.

## Differentiating a function

To differentiate a function with respect to one argument, call it with that
argument seeded and read the derivative off the result. The function is not
changed and does not need to import anything.

```petal
import autodiff

fn derivative(f, at)
    deriv_of(f(dual(at, 1.0)))
end

fn cube(x) x * x * x end

print(derivative(cube, 2.0))                   // 12.0
print(derivative(fn(t) sin(t) * t end, 0.0))   // 0.0
```

`derivative` here is six lines of script, not a builtin. There is no `grad()`
that takes a function and returns its derivative function: see
[Limits](#limits).

## Several inputs

One pass gives the derivative with respect to one seed. For a function of
several inputs, run it once per input, seeding that input with `1.0` and
leaving the others as plain numbers:

```petal
import autodiff

fn f(x, y) x * x * y + sin(y) end

let x = 3.0
let y = 2.0
print(deriv_of(f(dual(x, 1.0), y)))   // 12.0
print(deriv_of(f(x, dual(y, 1.0))))   // 8.583853163452858
```

Seeding two inputs in the same pass does not give two derivatives. It gives
one number, the derivative along that direction: with both seeds at `1.0`
above, the result is the sum of the two partials.

A gradient over a list of inputs is a loop over the positions:

```petal
import autodiff

fn gradient(f, point)
    for i in range(0, len(point)) do
        let seeded = for j in range(0, len(point)) do
            dual(point[j], if i == j then 1.0 else 0.0 end)
        end
        deriv_of(f(seeded))
    end
end

fn bowl(p)
    let dx = p[0] - 1.0
    let dy = p[1] + 3.0
    dx * dx + 2.0 * dy * dy
end

print(gradient(bowl, [0.0, 0.0]))   // [-2.0, 12.0]
```

Both `for` loops [collect](../syntax/implicit-return-values.md) their bodies
into lists. `gradient` calls `f` once per input, so a gradient over `n`
inputs costs `n` runs of the function.

## Worked example: gradient descent

Minimise `bowl` from the previous section by stepping against its gradient.
The minimum is at `(1, -3)`.

```petal
import autodiff

fn gradient(f, point)
    for i in range(0, len(point)) do
        let seeded = for j in range(0, len(point)) do
            dual(point[j], if i == j then 1.0 else 0.0 end)
        end
        deriv_of(f(seeded))
    end
end

fn bowl(p)
    let dx = p[0] - 1.0
    let dy = p[1] + 3.0
    dx * dx + 2.0 * dy * dy
end

let p = [0.0, 0.0]
let rate = 0.2
for step in range(0, 40) do
    let g = gradient(bowl, p)
    p = [p[0] - rate * g[0], p[1] - rate * g[1]]
end
print(round(p[0], 3), round(p[1], 3))   // 1.0 -3.0
```

The point `p` stays a list of plain floats. A dual is made for the length of
one call and taken apart again with `deriv_of`, which keeps derivatives from
leaking into the rest of the program. A one-input version is in
[`examples/console/differentiation.ptl`](../../examples/console/differentiation.ptl).

## Worked example: how far does this output move?

The derivative answers the question a slider or a drag handle asks: if I
nudge this parameter, how far does that output move, and which way? Take a
function that places the tip of a clock hand:

```petal
import autodiff

fn tip_x(cx, length, angle)
    cx + length * cos(angle)
end

let cx = 200.0
let length = 80.0
let angle = 0.6

let out = tip_x(cx, length, dual(angle, 1.0))
print(value_of(out))   // 266.02684919277425
print(deriv_of(out))   // -45.17139787160283

print(deriv_of(tip_x(dual(cx, 1.0), length, angle)))   // 1.0
print(deriv_of(tip_x(cx, dual(length, 1.0), angle)))   // 0.8253356149096783
```

The tip is drawn at about 266. It moves about 45 pixels left for each radian
added to `angle`, one pixel for each pixel added to `cx`, and 0.83 pixels for
each pixel added to `length`. That is enough to rank the three parameters by
how strongly each one moves the output, or to predict a small edit without
running it: `angle + 0.01` should land near `266.03 - 0.45 = 265.58`, and
`tip_x(cx, length, 0.61)` is `265.57`.

Run the other way round, the same number solves for a parameter. To put the
tip at a target position, divide the distance still to go by the derivative,
step the parameter by that much, and repeat (Newton's method):

```petal
import autodiff

fn tip_x(cx, length, angle)
    cx + length * cos(angle)
end

fn solve_angle(target, start)
    let a = start
    for i in range(0, 6) do
        let out = tip_x(200.0, 80.0, dual(a, 1.0))
        a = a + (target - value_of(out)) / deriv_of(out)
    end
    a
end

let solved = solve_angle(240.0, 0.6)
print(round(solved, 4))                      // 1.0472
print(round(tip_x(200.0, 80.0, solved), 4))  // 240.0
```

This goes through `cos`, which the runtime's own goal solving does not (see
[Differentiation and the other tools](#differentiation-and-the-other-tools)).
It needs a starting value near the answer and a derivative that is not zero
there.

## Supplying a missing rule

A function with no rule raises on a dual, and a function can be wrapped to
give it one. Compute the value from `value_of`, compute the derivative by
hand, multiply it by the incoming derivative (the chain rule), and build the
result with `dual`. When there is no formula for the derivative, a central
difference is a fair stand-in:

```petal
import autodiff
import noise

fn d_noise(x)
    let v = value_of(x)
    let h = 0.0001
    let slope = (noise(v + h) - noise(v - h)) / (2 * h)
    dual(noise(v), slope * deriv_of(x))
end

fn d_softplus(x)
    let v = value_of(x)
    dual(log(1 + exp(v)), deriv_of(x) / (1 + exp(-v)))
end

let x = dual(2.0, 1.0)
print(type(d_noise(x)))    // dual
print(d_softplus(x))       // dual(2.1269280110429727, 0.8807970779778823)
print(1 / (1 + exp(-x)))   // dual(0.8807970779778823, 0.1049935854035065)
```

The last line needs no wrapper: `exp` has a rule, so the sigmoid comes out
with its derivative. `d_softplus` is there to show the shape; `log(1 +
exp(x))` written out gives the same dual.

Because `value_of` and `deriv_of` accept plain numbers, these wrappers work
on a plain number too. They return a dual with derivative `0.0`.

## The `dual` type

`dual` is a [type annotation](../language-guide.md#type-annotations) like
`int` or `float`. Annotations are checked by `petal check` as warnings and
are not enforced when the program runs.

```petal
import autodiff

fn square(a: dual) -> dual
    a * a
end

let seeded: dual = dual(3.0, 1.0)
print(square(seeded))   // dual(9.0, 6.0)
```

- A slot declared `dual` warns when it is given a plain number (``argument 1
  to `square`: expected `dual`, found `float` ``). The call still runs.
- A slot declared `float` or `int` warns when it is given a value the checker
  knows to be a dual, such as `seeded` above. The checker does not work out
  the type of a `dual(...)` call or of arithmetic on duals by itself, so the
  annotation on the binding is what tells it.
- A slot declared `num` accepts an `int`, a `float` or a `dual`. Annotate with
  `num` the functions you intend to differentiate, since they take both.

A function's variants are told apart by their parameter count, not by
parameter types, so a function cannot have one body for `float` and another
for `dual` ([Function overloading](../syntax/function-overloading.md)). Test
`type(x) == "dual"` inside the function if you need to.

## Cost

A dual is stored inline in a value, as an int or a float is. Arithmetic on
duals allocates nothing and leaves nothing for the garbage collector. Each
operation does a few more floating-point operations than its plain
counterpart, to compute the derivative along with the value.

The cost that matters is the number of passes. One run of a function gives
the derivative for one seed, so a gradient over `n` inputs is `n` runs.
Forward mode suits a few inputs and any number of outputs. For many inputs
and one output (the shape of training a model) it is the expensive direction.

`petal bench --fn <name>` measures a function as the script calls it, so the
cost of a seeded call can be compared with a plain one directly
([CLI reference](../CLI.md#bench--what-a-call-of-a-function-costs)).

## Limits

- **Forward mode only.** There is no reverse mode (back-propagation), so no
  way to get every partial derivative from one pass.
- **No `grad()`.** A dual is all there is: no builtin takes a function and
  returns its derivative function, and none returns a gradient. The helpers
  on this page are script code. The `grad()` and `gradients()` calls in
  [`docs/aspirational-ideas/`](../aspirational-ideas/README.md) are a design
  sketch and do not run.
- **First derivatives only.** A dual holds two floats, and `dual` refuses a
  dual as an argument, so duals cannot be nested to get a second derivative.
- **One derivative per pass.** A dual does not record which input it was
  seeded from. See [Several inputs](#several-inputs).
- **A builtin with no derivative rule raises on a dual**, as does any host
  function that reads a float. Pass `value_of(x)`, or wrap it
  ([Supplying a missing rule](#supplying-a-missing-rule)). See
  [Which builtins carry a derivative](#which-builtins-carry-a-derivative).
- **Vectors and `f64_array` hold plain floats.** `vec2`, `vec3` and an
  `f64_array` refuse a dual component. Keep duals in lists or records and do
  the vector arithmetic per component.
- **A kink has no derivative.** At `abs(0)`, at a tie in `min` or `max`, at
  a `clamp` bound, and where two branches of an `if` meet, you get the answer
  of one side. `sqrt` and `hypot` report `0` at zero, where the true slope is
  unbounded.
- **Equality ignores the derivative.** `dual(2.0, 1.0) == 2.0` is `true`.
  Compare `deriv_of` as well if the derivative matters to the test.

## Differentiation and the other tools

Petal has three ways to ask how one value in a program relates to another.
They answer different questions and do not use each other.

**The dataflow tools say which values, a derivative says how much.**
[`petal graph`](../CLI.md#dataflow-query-commands) walks the compiled program
without running it and lists everything a term was computed from, or
everything that depends on it. [`petal explain`](../CLI.md#explain--show-the-value-chain-that-produced-a-term)
runs the program and shows the chain of values that produced one term. Take
this sketch:

```petal
import autodiff
let radius = dual(3.0, 1.0)
let scale = 2.0
let area = 3.14159 * radius * radius
let shown = area * scale
print(shown)   // dual(56.54861999999999, 37.699079999999995)
```

`petal graph --term radius --direction forward sketch.ptl` lists `area` and
`shown` among the terms that depend on `radius`. The seed on `radius` says by
how much: `shown` moves by about `37.7` for each unit of `radius`. The tools
work on a program that uses duals as on any other, and `explain` prints each
intermediate dual with its derivative:

```text
$ petal explain --term area sketch.ptl
Explain t146 (area):
  Provenance chain:
    => t146 area [line 4, column 12] = dual(28.274309999999996, 18.849539999999998)
     . t144 - [line 4, column 12] = dual(9.424769999999999, 3.14159)
     . t145 - [line 4, column 31] = dual(3.0, 1.0)
     . t142 - [line 4, column 12] = 3.14159
     . t143 - [line 4, column 22] = dual(3.0, 1.0)
     ...
```

**Direct manipulation does not use dual numbers.** When a host asks the
runtime to make an emitted value equal a target (the
[goal-based protocol](direct-manipulation.md#the-goal-based-protocol),
`petal propose-edit`), the runtime walks the argument's expression and
inverts `+`, `-`, `*`, `/` and negation algebraically, using the values the
traced run saw. The answer is exact, takes no iteration, and comes back as a
source edit. It stops at anything else: a function call, a field read, `%`.
An argument that passes through `cos` gets no proposal.

Dual numbers cover the part that inversion cannot reach, at the price of
doing it yourself in the script: they differentiate through function calls
and the builtins listed above, and a few Newton steps turn the derivative
into a solved parameter, as in
[the worked example](#worked-example-how-far-does-this-output-move). The
result is a number in the running program and not an edit to the source.
Writing it back is a separate step, for the host
([program-modification.md](program-modification.md) lists the ways).

## See also

- [Builtins.md, Automatic Differentiation](../Builtins.md#automatic-differentiation):
  the three signatures.
- [`examples/console/differentiation.ptl`](../../examples/console/differentiation.ptl):
  a runnable derivative check and gradient descent.
- [Direct manipulation](direct-manipulation.md): emit tracing and the goal
  protocol.
- [dev/Architecture.md](../dev/Architecture.md#differentiation-forward-mode):
  where the dual arithmetic lives in the runtime.
