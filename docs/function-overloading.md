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
none is; a mixed group is a compile error. Overload sets do not merge across
files: importing `f` from two modules is a collision. See
[Module system](module-system.md#exporting).
