# Function overloading

A top-level `fn` name can be declared more than once with different numbers
of parameters. Each call picks the variant whose parameter count matches the
number of arguments. (A variant with
[default parameter values](#default-parameter-values) accepts a range of
counts; the rule for that case is below.)

```petal
fn greet() print("hi") end
fn greet(name) print("hi", name) end
fn greet(a, b) print("hi", a, b) end

greet()           // hi
greet("world")    // hi world
greet("a", "b")   // hi a b
```

## The rules

- **Argument count is what matters** (with defaults, the names written too —
  see [below](#default-parameter-values)). Types play no part:
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

## Variants can call each other

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

## Methods overload too

A [method](language-guide.md#methods) is a named `fn`, so it overloads by
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
plain `shifted` never mix. An arity error names the method, counting the
receiver:

```petal ignore
Point(1, 2).shifted(1, 2, 3)
// Error: Point.shifted() expects 2 or 3 arguments, got 4
```

## Named arguments

[Named arguments](language-guide.md#named-arguments) are bound *after* the
variant is chosen. The count (positional plus named) picks the variant; the
names then map onto that variant's parameters. (Names help *choose* the
variant only when no variant has exactly that many parameters, or the one that
does lacks the name — see [Default parameter values](#default-parameter-values).)

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

A variant whose trailing parameters have
[defaults](language-guide.md#default-parameter-values) takes a *range* of
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

The `ui` prelude's draw calls are written this way
(`petal-ui/prelude/ui.ptl`). Where a call passes the *second* shape's leading
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

An overloaded name is one binding, so either every variant is `export`ed or
none is; a mixed group is a compile error. Importing `f` from two modules by
name is still a collision — a selective import is an explicit request, and two
of them for one name are ambiguous. See
[Module system](module-system.md#exporting).

### Sets merge across modules

A binding that lands on a name **another module** already put in scope *joins*
its overload set instead of replacing it. So a library can add an arity to a
name it does not own — the thing a component library wants when the host
prelude's `draw_rect` takes a record and a color and the library wants a
one-argument default-colored form:

```petal ignore
// lib.ptl — no `import ui` needed; `ui` is the host's implicit import
export fn draw_rect(r)
  ui.draw_rect(r, { r: 9, g: 9, b: 9 })
end

// every arity is callable here: the one this file added, and the
// prelude's 2, 3, 7 and 8-argument forms
export fn paint()
  draw_rect({ x: 0, y: 0, w: 10, h: 10 })
  draw_rect({ x: 20, y: 0, w: 10, h: 10 }, { r: 1, g: 2, b: 3 })
end
```

The rules:

- **Both sides must be function sets.** A binding that is not one — a `let`, a
  `var`, a record, a builtin native — shadows the whole set as it always did.
- **An arity both sides define goes to the higher-precedence binding**, and the
  lower one is simply unreachable at that arity. It is not an error. The
  precedence order is unchanged: the core prelude (`std`) < a host's implicit
  imports < the file's own `import`s < the file's own declarations.
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
