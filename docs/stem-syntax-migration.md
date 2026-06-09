# Stem Syntax Migration Guide

Petal's surface syntax has moved from C-style curly-brace blocks to **Stem** —
a keyword + `end` block style inspired by Ruby, Lua, and Elixir.

---

## Why Stem?

The headline win is unambiguous record literals.

In the old syntax `{}` served two distinct purposes: opening a code block and
constructing a record. The parser had to inspect surrounding context to decide
which was intended, producing edge cases and surprising errors when records
appeared in positions where a block was also legal.

Stem dissolves the ambiguity at the source: every block-opening keyword (`fn`,
`if`, `for`, `while`, `match`, `enum`) owns its block and closes it with `end`.
A `{` encountered anywhere in Stem source is **always** a record literal or a
string-interpolation escape — never a block opener. The rule fits in one
sentence and every tool can state it confidently.

Secondary wins:

- Code blocks read with named keywords (`if … then … end`, `for … do … end`),
  making nesting hierarchy explicit without counting braces.
- The dangling-else ambiguity is structurally impossible: there is exactly one
  `end` per `if`, and `else`/`elsif` can only appear inside the same
  `if … end` structure.
- Influences: Ruby (`end`-delimited blocks), Lua (`do … end` for loops), Elixir
  (`when` guards, `elsif` chaining).

---

## What Changed: Complete Before / After Reference

### Function declarations

```
// Before (curly-brace)
fn add(a, b) {
    a + b
}

// After (Stem)
fn add(a, b)
    a + b
end
```

Named functions no longer use a `{` after the parameter list. The body runs
until `end`.

---

### Lambdas — single-expression (arrow form)

```
// Before
let double = fn(x) { x * 2 }
filter(xs, fn(x) { x % 2 == 0 })

// After
let double = fn(x) -> x * 2
filter(xs, fn(x) -> x % 2 == 0)
```

When the body is a single expression, use `->`. No `end` is needed. The
expression ends at the next statement boundary or at any closing delimiter
(`end`, `elsif`, `else`, `when`, `)`, `]`).

---

### Lambdas — multi-statement (end form)

```
// Before
let log_and_double = fn(x) {
    print("input: {x}")
    x * 2
}

// After
let log_and_double = fn(x)
    print("input: {x}")
    x * 2
end
```

When the body needs more than one statement, omit `->` and close with `end`.

---

### if / elsif / else — statement form

```
// Before
if x > 0 {
    print("positive")
}

// After
if x > 0 then
    print("positive")
end
```

`{` becomes `then`; the closing `}` becomes `end`.

---

### if / elsif / else — chained

```
// Before
if score >= 90 {
    "A"
} else if score >= 70 {
    "B"
} else {
    "C"
}

// After
if score >= 90 then
    "A"
elsif score >= 70 then
    "B"
else
    "C"
end
```

`else if` collapses to a single keyword `elsif`. The entire chain shares one
`end`. `else` does not take a `then`.

---

### if as an expression — inline one-liner

```
// Before
let sign = if x > 0 { 1 } else { -1 }

// After
let sign = if x > 0 then 1 else -1 end
```

The one-liner form is still valid anywhere an expression is expected.

---

### for … do … end

```
// Before
for item in [1, 2, 3] {
    print(item)
}

for i in range(0, 10) {
    if i == 5 { break }
    print(i)
}

// After
for item in [1, 2, 3] do
    print(item)
end

for i in range(0, 10) do
    if i == 5 then break end
    print(i)
end
```

`{` after `in <expr>` becomes `do`; the closing `}` becomes `end`.

---

### while … do … end

```
// Before
while x > 0 {
    x -= 1
}

// After
while x > 0 do
    x -= 1
end
```

Same pattern as `for`: `{` becomes `do`, `}` becomes `end`.

---

### match — single-expression arms

```
// Before
match shape {
    Circle(r)  -> 3.14 * r * r
    Rect(w, h) -> w * h
    _          -> 0
}

// After
match shape
    when Circle(r)  -> 3.14 * r * r
    when Rect(w, h) -> w * h
    when _          -> 0
end
```

The `match` block no longer uses `{…}`. Each arm gains the `when` keyword.
Single-expression arms use `->` exactly as before.

---

### match — multi-statement arms

```
// Before
match cmd {
    "quit" -> exit(0)
    "save" -> {
        flush()
        write_disk()
        print("saved")
    }
    _ -> print("unknown")
}

// After
match cmd
    when "quit" -> exit(0)
    when "save" do
        flush()
        write_disk()
        print("saved")
    end
    when _ -> print("unknown")
end
```

A multi-statement arm uses `when Pattern do … end` (the same `do … end`
rhythm as loops). The `end` closes only that arm, not the whole `match`. The
enclosing `match` gets its own `end`.

---

### match — guards

```
// Before
match score {
    n if n >= 90 -> "A"
    n if n >= 70 -> "B"
    _            -> "C"
}

// After
match score
    when n if n >= 90 -> "A"
    when n if n >= 70 -> "B"
    when _            -> "C"
end
```

Guards (`if <cond>` after the pattern) are unchanged. They sit between the
pattern and the `->`.

---

### match — list destructuring

```
// Before
match xs {
    []           -> "empty"
    [a]          -> "one: {a}"
    [h, ...tail] -> "head={h}"
}

// After
match xs
    when []           -> "empty"
    when [a]          -> "one: {a}"
    when [h, ...tail] -> "head={h}"
end
```

List patterns are unchanged; only the `when` keyword is added.

---

### match — wildcard

```
// Before
match value {
    _ -> "anything"
}

// After
match value
    when _ -> "anything"
end
```

---

### enum … end

```
// Before
enum Color { Red, Green, Blue, Custom(r, g, b) }

enum Shape {
    Circle(radius)
    Rect(w, h)
    Point
}

// After
enum Color
    Red, Green, Blue, Custom(r, g, b)
end

enum Shape
    Circle(radius)
    Rect(w, h)
    Point
end
```

`enum Name { … }` becomes `enum Name … end`. Variant definitions are
identical.

---

## What Did NOT Change

The following features are **syntactically identical** in Stem. No migration
needed.

### Records `{}`

```petal
let person = {name: "Alice", age: 30}
person.age = 31
let pt = {...defaults, x: 100}   // spread also unchanged
```

Records look exactly the same — and in Stem, the parser never has to decide
whether `{` opens a block or a record. It is always a record.

### Lists `[]`

```petal
let xs = [1, 2, 3]
xs[0] = 10
push(xs, 4)
```

### Color literals `#fff`

```petal
let red  = #ff0000
let pink = #f80
```

### String interpolation `"{x}"`

```petal
let msg = "hello {name}, 2+2={2+2}"
```

Braces inside string literals are still interpolation escapes. This is the
other place `{` appears legitimately in Stem source.

### JSX

```petal
let ui =
    <div class="root">
        <h1>Hello {name}</h1>
        <p style={style}>Welcome</p>
        <Icon />
    </div>
```

JSX is philosophically aligned with Stem: `<tag>…</tag>` explicit close-tags
follow the same "every opener has a named closer" principle as `keyword … end`.
No change required.

### Pipe operator and method sugar

```petal
[1, 2, 3, 4]
    |> filter(fn(x) -> x % 2 == 0)
    |> map(fn(x) -> x * 10)

xs.filter(fn(x) -> x > 0)
  .map(fn(x) -> x * 2)
```

`|>` and `.method()` are unchanged. (The lambda bodies above use the new
`->` arrow form, but the operators themselves are identical.)

### State

```petal
fn counter()
    state count = 0
    count += 1
    count
end
```

`state name = init` is syntactically identical. The runtime keys state slots
on source location; Stem's named blocks make containment even more explicit.

### Assertions

```petal
assert(x > 0, "must be positive")
assert_eq(result, 42)
```

### Autodiff

```petal
let dx = dual(3.0, 1.0)
let y  = dx * dx + 2.0
print(value_of(y))   // 11
print(deriv_of(y))   // 6
```

`dual`, `value_of`, `deriv_of` and dual-number propagation through arithmetic
are unchanged.

---

## New Reserved Keywords

Stem reserves five keywords that were valid identifiers in the old syntax:

| Keyword  | Role                                     | Collision risk |
|----------|------------------------------------------|----------------|
| `end`    | closes every block                       | Low            |
| `then`   | opens `if`/`elsif` body                 | Low            |
| `do`     | opens `for`/`while` body                | Low–medium     |
| `elsif`  | chains `if` branches                    | Low            |
| `when`   | introduces a `match` arm                | **Medium–high**|

`when` is the most likely collision. Event-driven UI code often uses `when` as
a field name or callback. **Record field names are safe** — `{when: handler}`
is parsed in key position, not expression position, and remains legal. Variable
names and function names named `when` must be renamed.

In this codebase, none of the example programs or library source use these
words as identifiers, so no identifier renames are required.

---

## Pure Surface Syntax Change

Stem changes only what the programmer types. Everything downstream is
identical:

- **AST** — Stem's parser produces the same AST node types as the old frontend
  for equivalent programs.
- **IR / dataflow graph** — the compiler emits byte-for-byte identical IR.
  Provenance (source location, parent edges, datatype) is preserved.
- **Program slicing and ExplainTerm** — operate on the IR; require no changes.
- **Autodiff** — dual-number lifting happens at the evaluator level, agnostic
  to surface syntax.
- **State and hot-reload semantics** — state slots are keyed on source
  location. Stem's named blocks make the containment mapping easier to compute,
  not harder.
- **Temporal state arcs** — `state` inside `for do … end` is a distinct slot
  per iteration; `state` inside `if then … end` is a distinct slot per branch.
  Semantics are identical.

---

## Worked Examples

### Example 1: Traffic Light State Machine

A full program combining enums, `state`, `match` with single-expression arms,
and a `for` loop.

```petal
enum Light
    Red
    Yellow
    Green
end

fn next_light(current)
    match current
        when Red    -> Green
        when Green  -> Yellow
        when Yellow -> Red
    end
end

fn light_label(light)
    match light
        when Red    -> "STOP"
        when Yellow -> "CAUTION"
        when Green  -> "GO"
    end
end

fn tick()
    // Temporal state: survives across calls, keyed to this lexical location.
    state current = Red
    state ticks   = 0

    let duration = match current
        when Red    -> 4
        when Yellow -> 1
        when Green  -> 3
    end

    ticks += 1
    if ticks >= duration then
        current = next_light(current)
        ticks   = 0
    end

    print("Light: {light_label(current)}  (tick {ticks}/{duration})")
end

// Drive the machine for 12 steps
for _ in range(0, 12) do
    tick()
end
```

Key patterns to note:
- `enum Light … end` replaces `enum Light { … }`.
- `match current … end` with `when` arms replaces `match current { … }`.
- `match` used as an expression assigned to `let duration`.
- `if ticks >= duration then … end` replaces `if … { … }`.
- `for _ in range(0, 12) do … end` replaces `for _ in range(0, 12) { … }`.

---

### Example 2: Gradient Descent with Autodiff

Forward-mode dual numbers, `deriv_of`, and a descent loop. Demonstrates that
the autodiff builtins are completely unchanged; only the `fn` and `for`
block delimiters differ.

```petal
// Minimise f(x) = (x - 3)^2
// Analytical minimum: x = 3, f(3) = 0

fn f(x)
    (x - 3.0) * (x - 3.0)
end

let lr    = 0.1    // learning rate
let x     = 0.0    // starting point
let steps = 20

for step in range(0, steps) do
    // Lift x into the dual-number domain, derivative seed = 1
    let dx   = dual(x, 1.0)
    let y    = f(dx)
    let val  = value_of(y)
    let grad = deriv_of(y)

    print("step {step}: x={x}  f(x)={val}  f'(x)={grad}")
    x = x - lr * grad
end

print("converged to x={x}")
assert(value_of(f(dual(x, 1.0))) < 0.001, "did not converge")
```

Key patterns to note:
- `fn f(x) … end` replaces `fn f(x) { … }`.
- `for step in range(0, steps) do … end` replaces `for step in … { … }`.
- `dual`, `value_of`, `deriv_of` and all arithmetic are untouched.
- `assert` is untouched.

---

## Quick Cheat Sheet

| Old                            | New                              |
|-------------------------------|----------------------------------|
| `fn name(p) { … }`           | `fn name(p) … end`              |
| `fn(x) { expr }`             | `fn(x) -> expr`                 |
| `fn(x) { stmt; … }`         | `fn(x) … end`                  |
| `if c { … }`                 | `if c then … end`               |
| `if c { … } else { … }`     | `if c then … else … end`        |
| `else if c { … }`            | `elsif c then …`                |
| `if c { a } else { b }`      | `if c then a else b end`        |
| `for x in xs { … }`         | `for x in xs do … end`          |
| `while c { … }`             | `while c do … end`              |
| `match v { Pat -> e … }`    | `match v … when Pat -> e … end` |
| `match v { Pat -> { … } }` | `match v … when Pat do … end … end` |
| `enum E { … }`              | `enum E … end`                  |
