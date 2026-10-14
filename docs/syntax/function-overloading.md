# Function overloading, named arguments and default parameters

Three features decide how a call's arguments reach a function's parameters:
a name may be declared [more than once](#overloading-by-argument-count) with
different parameter counts, an argument may be passed
[by name](#named-arguments), and a parameter may have a
[default](#default-parameter-values). They interact, so they share this page.

## Overloading by argument count

A top-level `fn` name can be declared more than once with different numbers
of parameters. Each call picks the variant whose parameter count matches the
number of arguments. (A variant with
[default parameter values](#default-parameter-values) accepts a range of
counts; the rule for that case is [below](#defaults-and-overload-selection).)

```petal
fn greet() print("hi") end
fn greet(name) print("hi", name) end
fn greet(a, b) print("hi", a, b) end

greet()           // hi
greet("world")    // hi world
greet("a", "b")   // hi a b
```

### The rules

- **Argument count is what matters** (with defaults, the names written too —
  see [below](#defaults-and-overload-selection)). Types play no part:
  `fn f(x: int)` and `fn f(x: string)` have the same count, so the second
  simply replaces the first. Annotations are checked, not dispatched on.
- **Same count, same function.** Two declarations with the same number of
  parameters do not overload; the later one wins.
- **Top level only.** Overloading works for `fn` declarations at the top of a
  file. Inside a function body, a second `fn g(...)` with a different count
  replaces the first rather than joining it. Lambdas never overload.
  A nested `fn` that reuses a top-level name *shadows* the whole set for the
  rest of its enclosing function, and never becomes a variant of it — the set
  keeps its own arities everywhere else:

  ```petal
  fn box(w) w * w end
  fn box(w, h) w * h end

  fn outer()
      fn box(x) 10 end   // shadows both variants, only inside outer()
      box(3)             // 10
  end

  print(outer())    // 10
  print(box(3))     // 9
  print(box(3, 4))  // 12
  ```
- **Declaration order does not matter.** Top-level `fn`s are hoisted, so a
  call can appear above every variant.
- **The set is one value.** `let k = greet` binds all variants; `k("x")` and
  `map(names, greet)` dispatch the same way a direct call does.

### Variants can call each other

A common pattern is a short variant that fills in defaults and delegates:

```petal
fn count(n) count(n, 0) end
fn count(n, acc)
    if n <= 0 then acc
    else count(n - 1, acc + 1) end
end

print(count(5))      // 5
print(count(3, 10))  // 13
```

Variants capture outer variables like any other function:

```petal
let prefix = "Dr."
fn title(name) title(prefix, name) end
fn title(pre, name) print(pre, name) end

title("Smith")        // Dr. Smith
title("Mr.", "Jones") // Mr. Jones
```

### Methods overload too

A [method](../language-guide.md#methods) is a named `fn`, so it overloads by
the same rule. The receiver is an ordinary first parameter and counts:

```petal
class Point
  x: int,
  y: int,
end

fn Point.shifted(p: Point, d: int)           // 2 parameters
  Point(p.x + d, p.y + d)
end
fn Point.shifted(p: Point, dx: int, dy: int) // 3 parameters
  Point(p.x + dx, p.y + dy)
end

print(Point(1, 2).shifted(5))     // { x: 6, y: 7 }
print(Point(1, 2).shifted(5, 6))  // { x: 6, y: 8 }
```

Each qualified name is its own set: `Point.shifted`, `Other.shifted` and a
plain `shifted` never mix. An arity error names the method. `petal check`
counts the arguments written at the call site:

```petal ignore
Point(1, 2).shifted(1, 2, 3)
// error: method `Point.shifted` expects 1 or 2 arguments, got 3
```

Where the checker cannot see which class the receiver is, the same mistake is
a runtime error, and for an overloaded method that one counts the receiver:
`Point.shifted() expects 2 or 3 arguments, got 4`.

## Named arguments

An argument may be written `name: value`, binding it to the parameter of that
name rather than to its position:

```petal
fn scale(value, by, offset)
    value * by + offset
end

print(scale(2, by: 10, offset: 1))    // 21
print(scale(by: 10, offset: 1, value: 2))  // 21
```

Every positional argument must come before every named one; going back to
positional is a parse error:

```petal ignore
scale(value: 2, 10, 1)
// Error: A positional argument after a named argument: once an argument is
// named, every later argument must be named too
```

For a function without defaults every parameter needs a value, so a slot left
unfilled shows up as the ordinary arity error (`scale() expects 3 arguments,
got 2`). The two failures that are specific to names are an unknown parameter
and a slot filled twice:

```petal
scale(2, 10, nudge: 1)     // error: scale() has no parameter named 'nudge'
scale(2, value: 3, by: 1)  // error: scale() got multiple values for parameter 'value'
```

Wherever the callee is known before the program runs — a function or a
constructor named at the call site, or a binding that holds one — `petal check`
reports both of these as errors too, whether or not the line ever executes,
and lists the detail the runtime error has no room for:

```
error: scale() has no parameter named 'nudge' (parameters: 'value', 'by', 'offset')
error: scale() got multiple values for parameter 'value' (argument 1 already fills it)
```

`petal check` exits non-zero on them, so a bad name fails CI without the
branch ever being taken. The runtime error is still the only report for a
method call and for a callee this pass cannot see (a function passed in as a
parameter).

Lambdas take named arguments too, since they have parameter names like any
other function:

```petal
let sub = fn(a, b) -> a - b
print(sub(b: 1, a: 10))  // 9
```

A method's receiver occupies the first parameter, so `p.shift(dx: 2)` names the
parameters after it — and naming the receiver's own parameter is the
double-bind error above (`Point.shift() got multiple values for parameter
'p'`).

**Builtins take names too**, under the parameter names the
[Builtins Reference](../Builtins.md) gives them:

```petal
print(clamp(15, lo: 0, hi: 10))              // 10
print(pow(exp: 3, base: 2))                  // 8.0
print([1, 2, 3, 4].slice(start: 1, end: 3))  // [2, 3]
print(15 |> clamp(lo: 0, hi: 10))            // 10
```

The names are put in the builtin's own argument order before it runs, so a
named call does exactly what the positional one does, at the same cost. Method
syntax and `|>` supply the first parameter, and the names bind against the
rest. A builtin's parameter list reads like a `fn`'s, with these differences:

- Where a `fn` has [default values](#default-parameter-values), a builtin has
  **optional trailing parameters** — written `end?` in the reference
  (`slice(collection, start, end?)`, `round(x, places?)`). A call may leave
  them off, just as it may leave off a defaulted parameter. Unlike a default,
  one cannot be skipped to reach a later one — the builtin has no expression
  to fill the gap with — so `noise(x: 1, z: 2)` is `noise() is missing a
  value for parameter 'y'`.
- A builtin that reads its arguments differently by count declares each form
  (`distance(x1, y1, x2, y2)` and `distance(v1, v2)`), the way a `fn` declares
  overloads, and the names take part in picking
  the form. A call no form takes is reported as it is for an overloaded `fn`:
  `range(start: 1, stop: 5)` is `range() has no variant that accepts 2
  arguments with some named 'start', 'stop' (variants: range(end),
  range(start, end, step?))`.
- A parameter may be named with a keyword, which an argument label is free to
  be: `range(start: 0, end: 5)`.

The errors are the ones above (`clamp() has no parameter named 'low'`), and
`petal check` reports them the same way. The variadic builtins, `print` and
`format`, have no parameter names and refuse one outright:
`builtin 'print' does not accept named arguments`. So does a host native whose
embedder declared no names for it (see
[Native functions](../usage/ffi.md#native-functions)).

A function the standard prelude or a library writes in Petal
(`sum(xs: [1, 2])`, petal-ui's `draw_rect(x: 0, y: 0, …)`) is an ordinary `fn`
and takes the names its declaration gives.

### Named arguments and overload selection

Named arguments are bound *after* the
variant is chosen. The count (positional plus named) picks the variant; the
names then map onto that variant's parameters. (Names help *choose* the
variant only when no variant has exactly that many parameters, or the one that
does lacks the name — see [Defaults and overload selection](#defaults-and-overload-selection).)

```petal
fn box(w) box(w, w) end
fn box(w, h) [w, h] end

print(box(w: 3))          // [3, 3]
print(box(h: 2, w: 5))    // [5, 2]
```

So a name the chosen variant does not declare is caught only after
selection: `box(depth: 1)` picks the one-parameter `box`, then fails with
`box() has no parameter named 'depth'`. `petal check` reports it ahead of the
run in the same words, plus the parameters the chosen variant does have
(`box() has no parameter named 'depth' (parameters: 'w')`).

Note that the message names `box`, not the `box#1` the compiler calls the
one-argument variant internally. That internal name never appears in output —
not in an error, not in `show-ir`, `show-bytecode`, `show-graph`, `explain`, a
recorded trace, or the function table a host calls through.

## Default parameter values

A parameter may be given a default with `= expression`. A call that leaves the
argument out gets the default instead:

```petal
fn greet(name, greeting = "Hello", punct = "!")
    "{greeting}, {name}{punct}"
end

print(greet("Ada"))                  // Hello, Ada!
print(greet("Ada", "Hi"))            // Hi, Ada!
print(greet("Ada", punct: "?"))      // Hello, Ada?
```

Trailing defaulted parameters can simply be left off, and *any* defaulted
parameter can be skipped by [naming](#named-arguments) a later one, as
`punct: "?"` does above. A type annotation goes before the default:
`fn fade(c, amount: num = 0.5)`.

**The default is an expression, and it is evaluated on every call that omits
the argument.** It is not computed once when the function is declared, and its
result is never kept from one call to the next. It runs inside the function,
as the call starts, so it behaves exactly as if it were the first line of the
body:

```petal
fn tagged(item, tags = [])
    push(tags, item)
end

print(tagged("a"))   // ["a"]
print(tagged("b"))   // ["b"]    a fresh list each call — not ["a", "b"]
```

```petal
var issued = 0
fn next_id()
    set issued = get issued + 1
    get issued
end

fn make(label, id = next_id())
    "{id}: {label}"
end

print(make("first"))       // 1: first
print(make("second"))      // 2: second     next_id() ran again
print(make("third", 99))   // 99: third     an argument was passed — it did not run
print(make("fourth"))      // 3: fourth
```

So `fn stamp(at = time())` reads the clock on each call, and
`fn roll(n = random_int(1, 6))` rolls again each time.

Because it runs in the function, a default can use anything the body could:
the parameters declared **before** it, and any outer binding the function
captures.

```petal
let margin = 4

fn inset(w, h = w, pad = margin * 2)
    [w - pad, h - pad]
end

print(inset(20))           // [12, 12]    h defaults to w
print(inset(20, 10))       // [12, 2]
print(inset(20, pad: 0))   // [20, 20]
```

Defaults are evaluated left to right, and only for the parameters the call
left out. A default cannot read its own parameter or one declared after it —
that is a compile error, since the later parameter has no value yet:

```petal ignore
fn span(lo = hi - 10, hi = 100) lo end
// Error: The default value of parameter 'lo' refers to 'hi', which is declared
// after it — a default can only use the parameters declared before it
```

The rules:

- **Defaulted parameters come last.** A parameter without a default cannot
  follow one that has one (`fn f(a = 1, b)` is a parse error) — a call fills
  parameters left to right.
- **Passing `nil` is passing an argument.** `greet("Ada", nil)` binds
  `greeting` to `nil`; only *omitting* the argument evaluates the default.
- **A required parameter must be given.** Leaving one out names it:
  `greet()` is `greet() is missing a value for parameter 'name'`, and too many
  arguments is `greet() expects 1-3 arguments, got 4`. `petal check` reports
  both before the program runs.
- **Lambdas and methods take defaults too**: `fn(x, step = 1) -> x + step`, and
  `fn Rect.grow(r, by = 1)` (the receiver is always supplied, so it is never
  the one with a default). A callback that is handed fewer arguments than it
  declares uses its defaults for the rest: `map(xs, fn(x, scale = 10) -> x * scale)`.
- **An annotation is checked against the default**: `fn f(n: int = "one")`
  is a `petal check` warning.
- **Class fields and enum variant fields do not take defaults.** A constructor
  still needs every field; write a function with defaults that calls it.
- **Builtins have optional parameters rather than defaults.** A builtin is
  not written in Petal, so it has no default *expression*; the reference marks
  a parameter a call may leave off with `?` (`round(x, places?)`). It can be
  omitted or [named](#named-arguments) like a defaulted one, but only from the
  end: it cannot be skipped to reach a later one.

A default is ordinary code in the function, so it costs nothing when the
argument is passed, and a call that passes every argument positionally to a
function that has no defaults is exactly as fast as it was.

### Defaults and overload selection

A variant whose trailing parameters have defaults takes a *range* of
argument counts, and its optional parameters can be skipped by naming later
ones. Selection therefore asks one question of every variant — **does it
accept this call?** — and a variant accepts when

- there are no more arguments than it has parameters,
- every written name is one of its parameters (and is not already filled by a
  positional argument or by the same name twice), and
- every parameter the call leaves unfilled has a default.

Then:

1. **An accepting variant with exactly as many parameters as arguments were
   written wins.** A call that names nothing and matches an arity exactly
   always lands here, which is the whole rule as it stood before defaults —
   every such call resolves as it always has. One thing outranks it: a single
   other accepting variant in which a written name **skips a parameter** —
   the name lands past the slots the call's own count reaches, leaving an
   earlier parameter to its default. In the exact-count variant that name
   sits where a positional argument would have gone anyway; in the other it
   is the only way to write the call, so that is the variant it was written
   for. (Two such variants, and the call is ambiguous.)
2. **Otherwise the call must be accepted by exactly one variant.**
3. **Accepted by more than one, with no exact match, it is an error** — never
   settled by declaration order.

```petal
fn box(w) "square {w}" end
fn box(w, h, depth = 1) "box {w} {h} {depth}" end

print(box(2))               // square 2      exact: the 1-parameter variant
print(box(2, 3))            // box 2 3 1     no 2-parameter variant; one accepts
print(box(2, 3, 4))         // box 2 3 4     exact
print(box(2, 3, depth: 9))  // box 2 3 9
```

Written names take part in the choice. Here two arguments are written both
times, and only the name tells the variants apart:

```petal
fn at(x, y) "point {x},{y}" end
fn at(angle, radius, turns = 1) "polar {angle} {radius} x{turns}" end

print(at(1, 2))                   // point 1,2     exact arity
print(at(radius: 2, angle: 1))    // polar 1 2 x1  the 2-parameter `at` has no `radius`
```

Two variants whose defaults both stretch to cover a call are ambiguous for
that call, and the error says which:

```petal
fn pad(s, left = 1) s end
fn pad(s, left = 1, right = 1) s end

pad("x", 2)          // fine: exact arity, the 2-parameter variant
pad("x", right: 2)   // fine: only the 3-parameter variant has `right`
pad("x")
// Error: pad() is ambiguous: pad(s, left = …) and pad(s, left = …, right = …)
// both accept this call — pass or name another argument to pick one
```

The ambiguity is reported for the call, not the declarations: such a pair is
still useful for every call that says enough to pick one. `petal check`
reports it before the program runs wherever the callee is known. To avoid it
altogether, give each variant a distinct number of *required* parameters, or
fold the variants into one function with defaults.

Variants are still identified by their total parameter count: two
declarations with the same number of parameters do not overload, whatever
their defaults, and the later one replaces the earlier.

### Two shapes with the same count

Since one count is one variant, a function that takes two *shapes* of the same
length — a point record and a radius, or two coordinates — declares that
length once, under the names of one shape, and tells a positional call apart
by looking at an argument. The other shape gets its names from a variant of a
different length, whose defaults stretch down to the shared count; a named
call that does not fit the exact-count variant goes there by rule 2:

```petal
fn _is_num(v) type(v) == "int" || type(v) == "float" end

fn dot(center, radius)                       // also, positionally, (cx, cy)
    if _is_num(center) then "at {center},{radius} r=1"
    else "at {center.x},{center.y} r={radius}" end
end
fn dot(cx, cy, radius = 1) "at {cx},{cy} r={radius}" end

print(dot({x: 1, y: 2}, 5))            // at 1,2 r=5
print(dot(1, 2))                       // at 1,2 r=1    told apart by type
print(dot(cx: 1, cy: 2))               // at 1,2 r=1    told apart by name
print(dot(center: {x: 1, y: 2}, radius: 5))
```

The `ui` package's draw calls are written this way
(`core-libs/petal-ui/prelude/ui.ptl`). Where a call passes the *second* shape's leading
arguments positionally and then skips one of its parameters with a name the
exact-count variant also has — `draw_circle_outline(cx, cy, radius, c,
width: 2)`, whose count and `width` also fit `(center, radius, c, a, width)`
— the skip sends it to the second shape's declaration, as rule 1 says.

## Wrong argument count

A call that matches no variant is an error listing the counts on offer:

```petal
fn add(a, b) a + b end
fn add(a, b, c) a + b + c end

add(1)  // Error: add() expects 2 or 3 arguments, got 1
```

A variant with defaults is listed as a range — `box() expects 1 or 2-3
arguments, got 0` for the `box` above. Ranges that share a count are listed as
the one range they cover (`3-5` and `5-7` read `3-7`). When the count fits a variant but the
written names fit none, the error lists the variants instead:
`box() has no variant that accepts 2 arguments with one named 'nope'
(variants: box(w), box(w, h, depth = …))`.

`petal check` reports the same thing as an error before the program runs
(`` `add` expects 2 or 3 arguments, got 1 ``) and exits non-zero on it.
Constructors and methods are checked the same way; for a method the error
counts the arguments written at the call site, without the receiver.

## Across files

An overloaded name is one binding, so either every variant is `pub` or
none is; a mixed group is a compile error. Importing `f` from two modules by
name is still a collision — a selective import is an explicit request, and two
of them for one name are ambiguous. Two *plain* imports that both export `f`
are not a collision at the import: see [Which name
wins](../module-system.md#which-name-wins) and the merge rules below.

### Sets merge across modules

A binding that lands on a name **another module** already put in scope *joins*
its overload set instead of replacing it. So a library can add an arity to a
name it does not own — the thing a component library wants when the `ui`
package's `draw_rect` takes a record and a color and the library wants a
one-argument default-colored form:

```petal ignore
// lib.ptl
import ui

pub fn draw_rect(r)
  ui.draw_rect(r, { r: 9, g: 9, b: 9 })
end

// every arity is callable here: the one this file added, and every
// form `ui` declares
pub fn paint()
  draw_rect({ x: 0, y: 0, w: 10, h: 10 })
  draw_rect({ x: 20, y: 0, w: 10, h: 10 }, { r: 1, g: 2, b: 3 })
end
```

The rules:

- **Both sides must be function sets.** A binding that is not one — a `let`, a
  `var`, a record, a builtin native — shadows the whole set as it always did.
- **An arity both sides define goes to the higher-precedence binding**, and the
  lower one is simply unreachable at that arity. It is not an error. The
  precedence order is [the tier order](../module-system.md#which-name-wins): the
  core prelude (`std`) < the file's plain imports
  < its selective imports < its own declarations.
- **Two plain imports are the same tier**, so neither outranks the other.
  Their sets merge when no arity is defined by both — with a `check` warning
  at each call through the merged name — and a shared arity makes the bare
  name ambiguous, an error where it is called. A module that itself imports
  the other one wins outright instead: its set already merged the other's
  arities where it was declared.
- **Only across module boundaries.** Two declarations of the same arity in one
  file still replace each other, and a nested `fn` still shadows the whole set
  inside its enclosing function.
- **A variant's own name still means that variant inside its own body**, the
  self-recursion binding. To reach another arity from inside an added variant,
  call it through the module (`ui.draw_rect(r, c)`), as above.

Merging reaches the weak bindings too — that is the point of it — so a module
that declares `fn count(xs)` keeps `std`'s `count(xs, pred)` callable. One
wrinkle there: `std` is only merged into a program that *references* one of its
exports, and the gate ignores a name the file itself declares. A file whose only
mention of `count` is its own declaration does not pull `std` in, so there is
nothing to merge with; naming any other `std` export brings it back.
