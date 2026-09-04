# Function overloading

A top-level `fn` name can be declared more than once with different numbers
of parameters. Each call picks the variant whose parameter count matches the
number of arguments.

```petal
fn greet() print("hi") end
fn greet(name) print("hi", name) end
fn greet(a, b) print("hi", a, b) end

greet()           // hi
greet("world")    // hi world
greet("a", "b")   // hi a b
```

## The rules

- **Argument count is the only thing that matters.** Types play no part:
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
names then map onto that variant's parameters.

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

## Wrong argument count

A call that matches no variant is an error listing the counts on offer:

```petal
fn add(a, b) a + b end
fn add(a, b, c) a + b + c end

add(1)  // Error: add() expects 2 or 3 arguments, got 1
```

`petal check` reports the same thing as a warning before the program runs
(`` `add` expects 2 or 3 arguments, got 1 ``), and `petal check --strict`
fails on it. Constructors and methods are checked the same way; for a method
the warning counts the arguments written at the call site, without the
receiver.

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
