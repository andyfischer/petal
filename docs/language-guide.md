# Petal Language Guide

This is a user-facing reference for the Petal programming language. It covers all syntax,
types, operators, and features with examples.

For a compact, reference-style map of the whole surface syntax, see
[Syntax Overview](syntax/overview.md). For built-in functions (math,
collections, color, vectors, autodiff, etc.) see [Builtins.md](Builtins.md). For
the `petal` CLI and the IR JSON schema, see [CLI.md](CLI.md). For the design
philosophy behind the language, see [goals.md](dev/goals.md).

## Hello World

```petal
print("hello, world")
```

## Variables

Variables are declared with `let` and can be reassigned:

```petal
let x = 10
let name = "Petal"
x = 20
```

### `var` and `set`

`let` bindings are *dataflow* bindings: `x = 20` above does not overwrite a
slot, it rebinds the name to a new value, and the compiler can trace exactly
which earlier value each read came from. That is the default, and it is what
makes a Petal program readable as a diagram.

Some code genuinely wants a mutable slot instead — most often an accumulator
written from inside a callback. Declare it `var` and write it with `set`:

```petal
var count = 0
for i in [1, 2, 3] do
  set count = count + i
end
print(count)   // 6
```

The two write keywords are disjoint in both directions. `=` writes a `let`,
`set` writes a `var`, and each rejects the other:

```petal ignore
let a = 1
var b = 1

a = 2          // fine
set b = 2      // fine

set a = 2      // error: `a` is not a `var`; use `a = ...`
b = 2          // error: `b` is a `var`; use `set b = ...` to write it
```

That is deliberate. In a language where `=` already means "rebind", letting it
*also* mean "write through to a cell" would put two opposite operations behind
one glyph, told apart only by a declaration that may be far away. Every `set`
is a place the dataflow story goes dark, so it is written where it happens.

`set` also takes field and index targets (`set r.a = 1`, `set xs[0] = 1`) and
the compound forms (`set count += 1`), and never declares a binding — `set` on
an unknown name is an error, not a declaration.

A `var` binds a *box*, not a value, and that is what makes it useful: a
function or a closure that mentions the name shares the same box, so a write
from inside one is visible outside. This is the case `=` cannot express, and
the compiler says so: an `=` inside a function targeting a name bound outside
it is an error, because it would create a function-local shadow and leave the
outer binding alone. Reach for `var`/`set` when you meant the write to land;
reach for `let` when you meant a new local.

### `get`

Reading a cell from inside a function is written `get`:

```petal
var hits = 0

fn describe()
  "hits so far: {get hits}"
end

set hits = 3
print(describe())   // hits so far: 3
```

`get` is **required** wherever the read crosses a function boundary from the
declaration, and optional inside the declaring scope — the loop above writes
plain `count`, because there both spellings mean the same thing.

The reason is timing. A function captures the ordinary bindings it mentions
**by value, at the point the function is written**: a `let` or `state` read
inside a body answers with the value the name had on that line, and a later
rebinding produces a new value the function never sees. A cell read answers
with the box's contents *now*. Those are different questions, and before `get`
they were spelled the same way — so which one you got depended on a
declaration that might be hundreds of lines away. In a script that re-runs
every frame the gap is exactly one frame, every frame, which looks like input
lag rather than a mistake.

So the escape hatch has one keyword per operation, and none of it is implicit:

| | |
|---|---|
| `var x = …` | declare the box |
| `set x = …` | write it |
| `get x` | read it |

`get` binds tighter than `.` and `[]`, so it dereferences first and the rest
applies to the contents — `get cfg.width` is `(get cfg).width`, which is what a
cell holding a record wants. It is an error on anything that is not a `var`, so
`get` appearing in a body always means a cell.

### Capturing a reactive name that moves later

The other half of the same problem, though only for one kind of binding.

Because a function captures at its own position, a function reading a `let`
that is rebound below it sees the earlier binding. That is the **defined
behaviour**, not a mistake: the later `let` is a new binding, and the function
above it is meant to read the one that was in scope where it was written.
Nothing is reported.

A module-level `state` is different. Rebinding it with `=` does not create a
new binding — it writes the persisted slot (module scope is one path, so there
is exactly one), and the *next* run of the file initialises the name from that
slot. So a function above the write reads one run behind, every run:

```petal ignore
state scroll = 0

fn row_y(r)
  GRID_Y + (r - scroll) * ROW_H   // warning: reads `scroll` one run behind
end

scroll = scroll + 3
draw_row(row_y(2))                // uses the previous run's `scroll`
```

That is a **warning**, not an error — the program compiles and `petal check`
exits 0. Pass the value in when you want the current one:

```petal ignore
fn row_y(r, scroll)
  GRID_Y + (r - scroll) * ROW_H
end
```

A `state` that is never written after the function is fine to capture — the
overwhelmingly common case, and why constants, helpers and `fn`s all keep
reading exactly as before. Three shapes are deliberately left alone: `let`, as
above; an inline callback (`map(xs, fn(a) … end)`), which runs inside the
statement that created it, so nothing can move underneath it; and a binding
local to an enclosing *function*, since only module-level ones are checked (an
in-function `state` is a different slot per
[call path](#one-slot-per-call-path), and the lag rule is about the one
module-level slot a whole run shares).
`var`/`state var` are exempt too — a bare outer-cell read is already a hard
error there, and the `get` it demands is a live read that cannot lag.

Between the two rules, a bare name inside a function body is always a value
that cannot change under you, and anything live says `get`.

```petal
var score = 0
let doubled = map([1, 2, 3], fn(a)
  if a > 1 then set score += 10 end
  a * 2
end)
print(doubled, score)   // [2, 4, 6] 20
```

Each *evaluation* of the declaration makes a new box, so a factory hands out
independent counters:

```petal
fn counter()
  var c = 0
  let bump = fn()
    set c = get c + 1
    get c
  end
  bump
end
let a = counter()
let b = counter()
print(a(), a(), b())   // 1 2 1
```

Reading a `var` gives you its *contents*, never the box. Storing one in a
record, passing it to a function, or printing it all move the value as of that
moment; the only way to share a box is to capture it in a closure, which you
can see in the source.

```petal
var x = 1
let r = { a: x }
set x = 2
print(r, x)   // { a: 1 } 2
```

Prefer `let`. `var` is an escape hatch, and reaching for it costs you the
provenance queries (`petal show-provenance`, `petal show-slice`) that can
otherwise answer "what produced this value?".

### `config let` — declaring a tuning knob

A `config` modifier marks a `let` binding as the value a person is *meant* to
adjust:

```petal
config let offset = 10
let x = 20
draw_circle(x + offset, 40, 12)
```

The binding evaluates exactly as a plain `let`; what changes is how tools
treat it. Goal-based editing (docs/direct-manipulation.md) prefers rewriting
config bindings and leaves the rest of the program alone — dragging the circle
above edits `offset`, never `x` — and a live-editing host has an honest place
to render a slider per knob. `config` is contextual (only special immediately
before `let`), so it remains usable as an ordinary name, and it composes with
`export` as `export config let`. A mutable `var` cell cannot be `config`.

## Types

Petal has the following value types:

| Type | Examples |
|------|----------|
| `int` | `42`, `-3`, `0` |
| `float` | `3.14`, `-0.5`, `1.0` |
| `bool` | `true`, `false` |
| `string` | `"hello"`, `"world"` |
| `nil` | `nil` |
| `list` | `[1, 2, 3]` |
| `record` | `{name: "Alice", age: 30}` |
| `color` | `#ff8800`, `#f80` (desugars to record) |
| `enum` | `Some(42)`, `None` |
| a class | `Rect(0, 0, 8, 8)` (a record tagged with its [class](#classes--methods)) |

Use `type(value)` to get the type name as a string at runtime. For a class
instance that is the class's own name (`"Rect"`), not `"record"`.

## Type Annotations

Petal is dynamically typed, but you can *optionally* annotate every binding form
(`let`, `var`, `state`), function and lambda parameters, and a named function's
return type. Annotations are checked at compile time and are **advisory only**: a
mismatch is a warning, never an error, and annotations have no effect at runtime.
Run `petal check <file>` to see the warnings, or `petal check --strict` to make
them fail the exit code (see [CLI.md](CLI.md#check--validate-without-running)).

```petal
let count: int = 0
let name: string = "Petal"        // `str` is also accepted for `string`

fn area(r: float) -> float        // `:` before a param type, `->` before the return type
  3.14159 * r * r
end

fn greet(who: string)             // the return type is optional
  print("Hello, {who}!")
end

// Typed and un-annotated code mix freely — an omitted annotation is inferred
// where possible, otherwise treated as `any` (which suppresses checking).
fn scale(v, factor: float) -> float
  v * factor
end

let double = fn(n: int) -> n * 2  // lambda params take annotations too
```

A lambda's `->` already introduces its body, so a lambda has **no** return-type
annotation — write `fn(n: int) -> n * 2`, not `fn(n: int) -> int -> n * 2`.

**Cells and reactive bindings.** `var` and `state` take the same `: type` slot,
and it means more there than on a `let`:

```petal
var total: int = 0
set total = "oops"        // warning: `total` declared `int` but assigned `string`

state count: int = 0
state var seen: list = []
state(row.id) hovered: bool = false   // the annotation follows the name
```

A `var` is a heap cell and a `state` is a reactive binding, so their initializers
say nothing about what a *later* read observes — a `set` or the next frame can
replace the value from anywhere. Un-annotated, both read as `any`, and the
checker stays quiet about them. An annotation is what makes them checkable: it
types every read *and* constrains every write, wherever the write is.

The type names are the ones `type(value)` reports, written lowercase: `nil`,
`bool`, `int`, `float`, `string` (alias `str`), `list`, `record`, `function`,
`enum`, `vec2`, `f64_array`, `element`, `symbol`, `dual`, `handle`, `pending` —
plus two that no value ever reports: `any`, the dynamic escape hatch, and
`num`, meaning **`int` or `float`**. They are recognized only in type position,
so `int`, `float`, and `str` remain callable as the cast builtins everywhere
else.

Write `num` for a slot that genuinely takes either numeric width — the common
case, since arithmetic accepts both. It is what the built-in `Rect` declares
for its edges, which are ints in pixel layout and floats in sub-pixel layout:

```petal
fn scale_by(v: num, factor: num) -> num
  v * factor
end

scale_by(3, 2)        // ok
scale_by(3.5, 2)      // ok
scale_by("3", 2)      // warning: string is not assignable to num
```

The name of any [class](#classes--methods) — declared or built in — is a type
name too, anywhere in the file that declares it (a class declaration is
hoisted) and in any file that imports it. These names are reserved: a class may
not take one of the lowercase built-in type names above.

A type is a single bare name. There are no parameterized types (`list<int>`),
arrow types, structural record types, or user-defined aliases — `list` and
`record` are opaque, which is why a write through a field or index
(`set r.a = …`, `xs[0] = …`) is never checked against an element type.

**Assignability.** An `int` may be used where a `float` is expected (the same
promotion arithmetic already does), but the reverse is not allowed — there is no
implicit casting. Cross types explicitly with `int()`, `float()`, or `str()`:

```petal
let n: int = 3.9          // warning: float is not assignable to int
let n: int = int(3.9)     // ok — explicit cast, n is 3
let x: float = 5          // ok — int promotes to float
```

`num` widens but never narrows: an `int`, a `float` or a `dual` all satisfy a
`num` slot, and a `num` satisfies neither `int` nor `float` without an explicit
cast — otherwise accepting one would be the implicit truncation the rule above
forbids.

```petal
fn width(r: Rect) -> num
  r.w                     // ok — a rect edge is a num
end

fn pixels(n: int) n end
fn place(x: num)
  pixels(x)               // warning: num is not assignable to int
  pixels(int(x))          // ok — explicit cast
end
```

`any` on either side of a check is always compatible, so a value flowing in from
un-annotated (dynamic) code is trusted. Unrecognized type names are kept as
written and reported too — `let x: banana = 5` warns `unknown type name
\`banana\`` rather than failing to compile.

**Calls through a binding.** A parameter annotation travels with the function,
not just with its name, so calling one through a binding is checked the same way
a direct call is:

```petal
let double = fn(n: int) -> n * 2
double("hi")              // warning: argument 1 to `double`: expected `int`, found `string`

fn greet(who: string)
  print("Hello, {who}!")
end
let hello = greet
hello(7)                  // warning: argument 1 to `hello`: expected `string`, found `int`
```

Re-assigning the binding drops what was known about it — the slot holds a
different function now. A function that merely *arrives* as an argument
(`fn apply(f, x) f(x) end`) is unknown, and unknown callables are never checked.

**Argument counts.** A call that no declaration can accept is reported without
running the program, since Petal [overloads by arity](function-overloading.md)
and such a call cannot resolve at runtime:

```petal
fn f(a, b)
  a
end
f(1)            // warning: `f` expects 2 arguments, got 1

class Point
  x: int,
  y: int,
end
Point(1)        // warning: `Point` expects 2 arguments, got 1
```

Every declared arity counts as a candidate — with `fn f(a)` and `fn f(a, b)`
both declared, `f(1, 2, 3)` warns `` `f` expects 1 or 2 arguments, got 3 ``.
Methods are checked the same way, counted as the call site writes them (the
receiver is not one of them): with `fn Point.shift(p: Point, dx: int)`,
`p.shift()` warns `` method `Point.shift` expects 1 argument, got 0 ``. Builtins
take flexible argument counts and declare no signature, so they are never
checked.

Inference is deliberately shallow and local (literals and the signatures of
called functions); anything else infers `any` and so reports nothing. The pass
prefers missing a mismatch to inventing one.

## Arithmetic

```petal
let a = 10 + 3    // 13
let b = 10 - 3    // 7
let c = 10 * 3    // 30
let d = 10 / 3    // 3
let e = 10 % 3    // 1
let f = -a        // -13
```

Float arithmetic works the same way. Mixed int/float operations promote to float.

### Compound Assignment

```petal
let x = 10
x += 5   // x is now 15
x -= 3   // x is now 12
x *= 2   // x is now 24
x /= 4   // x is now 6
x %= 4   // x is now 2
```

## String Operations

### Concatenation

Use `++` to concatenate strings:

```petal
let greeting = "hello" ++ " " ++ "world"
```

### String Interpolation

Use `{}` inside strings to embed expressions:

```petal
let name = "Petal"
print("hello, {name}!")
print("2 + 2 = {2 + 2}")
```

A nested string inside a hole may be written bare or backslash-escaped; both
spellings mean the same thing:

```petal
print("{if name == "" then "anon" else name end}")
print("{if name == \"\" then \"anon\" else name end}")
```

Such a nested string has to close on the same line it opened on. Without that
rule a stray quote silently swallows the rest of the file, and the error surfaces
far from its cause.

For a literal brace, escape it or use a raw string:

```petal
print("\{" ++ name ++ "\}")   // {Petal}
print("""{""" ++ name ++ """}""")
```

A bare `"{"` is a parse error — the brace opens a hole, so the quote that was
meant to close the string opens a nested one instead.

### Raw (triple-quoted) strings

Use `"""..."""` for a raw, multi-line string. The contents are captured
verbatim: `{` and `}` are literal (no interpolation), backslashes are not
treated as escapes, and raw newlines are allowed. This is handy for embedding
source code or any text full of braces and quotes:

```petal
let src = """
    fn step(input)
        str(input) ++ "!"
    end
    step
"""
```

Ordinary double-quoted strings may also span multiple lines, but a `{` inside
them starts an interpolation hole — use a raw string when you want braces to be
literal. (A string nested *inside* a hole is the one exception: it must close on
the line it opened on.)

### String Builtins

```petal
len("hello")          // 5
split("a,b,c", ",")   // ["a", "b", "c"]
join(["a", "b"], ",")  // "a,b"
contains("hello", "ell")  // true
reverse("hello")       // "olleh"
slice("hello", 1, 3)   // "el"
```

## Comparison and Logical Operators

```petal
// Comparisons (return bool)
x == y    x != y
x < y     x <= y
x > y     x >= y

// Logical operators (short-circuit)
a && b    // true if both true
a || b    // true if either true
!a        // negation
```

## Control Flow

### If / Else

`if` is an expression that returns a value:

```petal
let x = 10
if x > 5 then
    print("big")
else
    print("small")
end

// As an expression
let label = if x > 5 then "big" else "small" end
```

Chain conditions with `elsif` — it continues the same `if` and shares the
single closing `end` (no nested blocks):

```petal
fn classify(n)
  if n < 0 then "negative"
  elsif n == 0 then "zero"
  else "positive"
  end
end

// Reads well for the multi-way branches common in drawer code:
let color = if line.kind == "hunk" then HUNK
  elsif line.kind == "add" then GREEN
  elsif line.kind == "del" then RED
  else CONTEXT
  end
```

Note the keyword is `elsif` (one word, Ruby-style), not `else if` — writing
`else if` opens a nested `if` that needs its own `end`.

### For Loops

Iterate over lists or ranges:

```petal
for item in [1, 2, 3] do
    print(item)
end

for i in range(0, 5) do
    print(i)
end
```

#### For loops as mapping expressions

Used in **value position** a `for` loop evaluates to a **list** built from the
last expression of each iteration. This turns a loop into a mapping. The rule is
simply *"the loop's value is used"*; in full, that is when the loop is

- assigned to a name (`let xs = for … end`),
- `return`ed,
- passed as an argument (`len(for … end)`, `print(for … end)`),
- placed as a list element (`[for … end, 9]`),
- **used as a record field value** (`{rows: for … end}`),
- interpolated into a string (`"{for … end}"`),
- or in a **tail position** — see below.

```petal
let squares = for i in range(1, 6) do
    i * i
end
// squares == [1, 4, 9, 16, 25]

fn doubled(xs)
    return for x in xs do x * 2 end
end
```

**Tail positions count as value positions.** A loop that ends a construct whose
own value is used collects just the same — no `return` or intermediate binding
needed. That covers a function body's **implicit return**, an **`if`/`else`
branch tail**, a **`match` arm tail**, and the body of an enclosing collecting
loop:

```petal
fn doubled(xs)
    for x in xs do x * 2 end        // implicit return: [2, 4, 6] for [1, 2, 3]
end

fn rows(n)
    if n > 0 then
        for i in range(0, n) do i end   // if-branch tail: [0, 1, 2] for n = 3
    else
        []
    end
end

fn tagged(n)
    match n
        when 0 -> []
        when m -> for i in range(0, m) do i + 100 end   // match-arm tail
    end
end

let grouped = {ids: for i in range(0, 3) do i end}      // record field: [0, 1, 2]
```

A `for` loop only produces a list when its value is actually used ("captured").
A **bare `for` statement** runs purely for its side effects and allocates no
list — so existing loops keep their zero-overhead behavior:

```petal
for i in range(0, 3) do
    print(i)          // side effects only, no list built
end
```

A side-effect loop that happens to end a function is in tail position, so it
does collect. Add a trailing `nil` when the caller has no use for the list and
the allocation is worth avoiding:

```petal
fn draw_all(items)
    for it in items do draw(it) end
    nil               // side effects only again
end
```

Inside a collecting loop:

- `continue` **filters**: that iteration contributes nothing to the list.
- `break` ends collection; the elements gathered so far are the result.

```petal
let odds = for i in range(0, 10) do
    if i % 2 == 0 then continue end
    i
end
// odds == [1, 3, 5, 7, 9]
```

Loops nest directly — the inner loop is in the outer body's tail position, so it
collects and each outer iteration contributes a row:

```petal
let grid = for row in range(0, 3) do
    for col in range(0, 3) do
        row * 10 + col
    end
end
// grid == [[0,1,2], [10,11,12], [20,21,22]]
```

`while` loops are statement-only: they have no collecting expression form.

### While Loops

```petal
let x = 0
while x < 10 do
    print(x)
    x += 1
end
```

### Break and Continue

`break` exits the loop; `continue` skips to the next iteration.

```petal
for i in range(0, 100) do
    if i == 5 then
        break       // exit the loop
    end
    if i % 2 == 0 then
        continue    // skip to next iteration
    end
    print(i)
end
```

## Functions

Functions are declared with `fn`. The last expression is the implicit return value:

```petal
fn add(a, b)
    a + b
end

print(add(2, 3))  // 5
```

Use `return` for early exit:

```petal
fn abs(x)
    if x < 0 then
        return -x
    end
    x
end
```

### Named Arguments

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

Overloads are still chosen by the *total* number of arguments (positional plus
named) — see [Function Overloading](function-overloading.md). Only once a
variant is chosen do the names pick out its parameter slots, so the count still
has to match a declared arity before any name is looked at, and a slot left
unfilled shows up as the ordinary arity error (`scale() expects 3 arguments,
got 2`). The two failures that are specific to names are an unknown parameter
and a slot filled twice:

```petal
scale(2, 10, nudge: 1)     // error: scale() has no parameter named 'nudge'
scale(2, value: 3, by: 1)  // error: scale() got multiple values for parameter 'value'
```

Wherever the callee is known before the program runs — a function or a
constructor named at the call site, or a binding that holds one — `petal check`
reports both of these as warnings too, whether or not the line ever executes,
and lists the detail the runtime error has no room for:

```
warning: scale() has no parameter named 'nudge' (parameters: 'value', 'by', 'offset')
warning: scale() got multiple values for parameter 'value' (argument 1 already fills it)
```

`petal check --strict` exits non-zero on them, so a bad name fails CI without
the branch ever being taken. The runtime error is still the only report for a
method call, for a callee this pass cannot see (a function passed in as a
parameter), and for a builtin.

Lambdas take named arguments too, since they have parameter names like any
other function:

```petal
let sub = fn(a, b) -> a - b
print(sub(b: 1, a: 10))  // 9
```

A method's receiver occupies the first parameter, so `p.shift(dx: 2)` names the
parameters after it — and naming the receiver's own parameter is the
double-bind error above (`Point.shift() got multiple values for parameter
'p'`). Builtins carry no parameter names at runtime, so they reject names
outright rather than guessing: `append(list, x: 1)` is
`builtin 'append' does not accept named arguments`.

### Recursion

```petal
fn factorial(n)
    if n <= 1 then 1
    else n * factorial(n - 1)
    end
end
```

### Declaration order: top-level `fn`s are hoisted

A top-level `fn` is usable **above** where it is written, so declaration order
inside a file does not matter and mutual recursion works — the table stakes for
parsers, tree-walkers and state machines:

```petal
fn is_even(n)
    if n == 0 then true else is_odd(n - 1) end
end

fn is_odd(n)
    if n == 0 then false else is_even(n - 1) end
end

print(is_even(10))  // true
```

The hoist is deliberately conservative, because moving a declaration must never
change what it captures. A top-level `fn` stays exactly where it is written when

- its body mentions something the file computes at run time (a top-level `let`,
  `var`, `state`, or an enum variant), or
- it shadows a name already in scope — so `let old_max = max` above `fn max`
  still reads the builtin.

For the one case that leaves — a top-level *call* to a declaration below it that
was not hoistable — the compiler reports it as "call to `h` before its
declaration" at compile time, rather than leaving a bare runtime `Cannot call
nil` at the call site. A reference from *inside* another function body is always
fine: that body runs after the whole file has.

### Lambdas

Anonymous functions use `fn` without a name:

```petal
let double = fn(x) -> x * 2
print(double(5))  // 10
```

A lambda body can also be an `end`-terminated block, and a lambda that takes
no arguments may leave the parameter list out altogether:

```petal
let describe = fn(x)
    print(x)
    x * 2
end

let answer = fn -> 42        // argless arrow lambda
let greet = fn print("hi") end
let nothing = fn end         // empty body, returns nil
```

Argless lambdas are the usual way to pass a callback that needs no arguments:

```petal
on_click(fn set_count(count + 1) end)
```

**Wart:** after `fn`, a leading `(` is *always* parsed as the parameter list,
so an argless lambda's body cannot begin with a parenthesized expression.
`fn (a + b) * 2 end` reads `(a + b)` as a parameter list and is an error.
Write `fn -> (a + b) * 2` instead.

### Closures

Functions capture variables from their enclosing scope:

```petal
fn make_adder(n)
    return fn(x) -> x + n
end

let add5 = make_adder(5)
print(add5(10))  // 15
```

### Pipe Operator

The pipe operator `|>` passes a value as the first argument to a function:

```petal
let result = [3, 1, 2] |> sort |> reverse
print(result)  // [3, 2, 1]
```

### Method Syntax

Dot notation desugars to a function call with the receiver as the first argument:

```petal
fn greet(person)
    print("hello, {person.name}!")
end

let alice = {name: "Alice"}
alice.greet()  // same as greet(alice)
```

This is the last of four things `value.name(...)` can resolve to; a callable
record field and a [class's methods](#resolution-order) are tried first.

### Rebind Operator

Prefixing a call argument with `@` assigns the call's result back to that
variable — shorthand for the common `x = f(x)` pattern that immutable values
produce:

```petal
let nums = [1, 2, 3]
append(@nums, 4)   // same as: nums = append(nums, 4)
print(nums)        // [1, 2, 3, 4]
```

`@` binds to the nearest enclosing call, and works on `let` bindings only — it
desugars to `x = f(x)`, so on a [`var`](#var-and-set) it is an error and you
write `set nums = append(nums, 4)`. See the
[Rebind Operator](syntax/rebind-operator.md) doc for nesting, statement-level scope,
and limits.

## Lists

```petal
let nums = [1, 2, 3]
print(nums[0])        // 1 (zero-indexed)
nums = append(nums, 4)   // append returns a new list — rebind to keep it
                         // (or use the @ rebind operator: append(@nums, 4))
print(len(nums))      // 4
```

Lists are immutable values: `append` produces a new list rather than changing
the original, so `let b = append(a, x)` leaves `a` untouched. Grow an
accumulator by rebinding the variable (`xs = append(xs, x)`).

**Commas are required** between elements — here and in every other
comma-separated construct (call arguments, parameters, record literals, enum
declarations, and the matching patterns). Neither whitespace nor a newline
separates elements: `[1 2]` is a parse error. A trailing comma before the
closing delimiter is always allowed, so a list may be written one element per
line:

```petal
let xs = [
    1,
    2,
    3,
]
print(len(xs))    // 3
```

See [Commas](syntax/commas.md) for the full rule.

### List Builtins

```petal
sort([3, 1, 2])                // [1, 2, 3]
reverse([1, 2, 3])             // [3, 2, 1]
slice([1, 2, 3, 4], 1, 3)     // [2, 3]
flat([[1, 2], [3, 4]])         // [1, 2, 3, 4]
contains([1, 2, 3], 2)         // true
enumerate(["a", "b"])           // [[0, "a"], [1, "b"]]
zip([1, 2], ["a", "b"])        // [[1, "a"], [2, "b"]]
```

### Higher-Order Functions

```petal
map([1, 2, 3], fn(x) -> x * 2)              // [2, 4, 6]
filter([1, 2, 3, 4], fn(x) -> x > 2)        // [3, 4]
reduce([1, 2, 3], 0, fn(acc, x) -> acc + x) // 6
```

## Records

Records are key-value structures with string keys:

```petal
let person = {name: "Alice", age: 30}
print(person.name)      // "Alice"
person.age = 31          // mutation
```

### Nested Records

```petal
let user = {
    name: "Bob",
    address: {
        city: "Portland",
        region: "OR"
    }
}
print(user.address.city)  // "Portland"
```

### Field assignment

You can assign to a field directly, including nested fields and fields of
records stored inside lists.

```petal
let p = {x: 1, y: 2}
p.x = 10                     // field assignment
p.y = p.y + 1

let pts = [{x: 0, y: 0}, {x: 0, y: 0}]
pts[0].x = 100               // through a list element

let user = {name: "Bob", address: {city: "Portland"}}
user.address.city = "Seattle" // nested field
```

This is a *rebind of the name*, exactly like `p = ...`: it produces an updated
record and points `p` at it. Records are values, so anything else holding the
old one keeps it, and the compiler can still trace where each value came from.

```petal
let r = {a: 1}
let s = r
r.a = 2
print(r, s)   // { a: 2 } { a: 1 }
```

The same is true across a call: a function that assigns to a field of its
parameter updates its own local binding, not the caller's record. To share one
record between writers, declare it [`var`](#var-and-set) and write it with
`set r.a = ...`.

### Spread

Use `...expr` inside a record literal to copy all fields from another record.
Fields that follow the spread override the copied values.

```petal
let defaults = {x: 0, y: 0, color: "gray"}
let moved = {...defaults, x: 100}    // {x: 100, y: 0, color: "gray"}
```

Spread and field assignment both produce an updated record — spread names the
fields it keeps, field assignment names the fields it changes. Use whichever
reads better at the call site.

### Ragged records — reading a field that may not be there

Reading a field a record does not carry is a **hard error**. That is deliberate:
a typo'd field name should fail where it is written, not silently read as nil.

```petal
let el = {tag: "p"}
print(el.fragment)     // error (No field 'fragment' on record)
```

Data that is legitimately ragged — decoded JSON, a partial style record — says
so with `??`. The nil-coalescing operator makes its **left side** tolerant: a
field (or `[key]`) the record does not carry reads as nil there, so the fallback
runs instead of the frame aborting.

```petal
print(el.fragment ?? "")        // "" — no error
print(el["fragment"] ?? "")     // "" — same for index syntax
```

The tolerance covers the whole access chain on the left of `??`, so a link
missing partway down is fine too:

```petal
let cfg = {}
print(cfg.window.width ?? 800)  // 800
```

It covers *only* an absent record key. A wrong-typed base and an out-of-bounds
list index are bugs, not ragged data, and stay hard errors on both sides of the
operator:

```petal
let n = 3
print(n.width ?? 800)           // error (Cannot access field 'width' on int)
print([1, 2][9] ?? 0)           // error (Index 9 out of bounds)
```

When there is no sensible fallback to write — the value is simply absent and the
caller will deal with it — `?.` asks for the tolerance on its own:

```petal
print(el?.fragment)             // nil — no error, no fallback needed
print(el?.fragment ?? "")       // "" — the two compose
print(el?.["fragment"])         // nil — the index spelling
```

One `?.` covers the rest of its chain, the way JavaScript's does, so a missing
link partway down does not then fail on the link written after it:

```petal
let cfg = {}
print(cfg?.window.width)        // nil
```

`?.` softens absence and nothing else — the same line the `??` rule draws:

```petal
print(3?.width)                 // error (Cannot access field 'width' on int)
print([1, 2]?.[9])              // error (Index 9 out of bounds)
```

It is a read, not a write: `a?.b = v` is not an assignment target.

When the key is computed, the prelude spells the same rule as a function —
`field(rec, key, fallback)`, plus `has_field(rec, key)` for the one question
`??` cannot answer (a key that is present but nil):

```petal
field({a: 1}, "zz", 7)          // 7
has_field({a: nil}, "a")        // true
field({a: nil}, "a", 7)         // 7 — nil coalesces
```

### Record Builtins

```petal
keys({a: 1, b: 2})     // ["a", "b"]
values({a: 1, b: 2})   // [1, 2]
```

## Color Literals

CSS-style hex color literals desugar into records with `r`, `g`, `b` (and `a`) fields.
Values are integers 0–255.

```petal
let red = #ff0000        // {r: 255, g: 0, b: 0}
let coral = #ff7f50      // {r: 255, g: 127, b: 80}
print(coral.r)           // 255
```

Four formats are supported:

| Format | Example | Expansion |
|--------|---------|-----------|
| `#rgb` | `#f80` | `{r: 255, g: 136, b: 0}` |
| `#rgba` | `#f80a` | `{r: 255, g: 136, b: 0, a: 170}` |
| `#rrggbb` | `#ff8800` | `{r: 255, g: 136, b: 0}` |
| `#rrggbbaa` | `#ff8800aa` | `{r: 255, g: 136, b: 0, a: 170}` |

In the short 3/4-digit forms, each digit is doubled (e.g. `f` → `ff` = 255).

## Enums

Enums define named variants, optionally with associated data:

```petal
enum Color
    Red,
    Green,
    Blue,
    Custom(r, g, b),
end

let c = Red
let pink = Custom(255, 192, 203)
```

## Classes & Methods

A `class` declares a **named record type**: fields with optional types, and a
constructor bound to the class's name.

```petal
class Rect
  x: int,
  y: int,
  w: int,
  h: int,
end

let r = Rect(0, 0, 100, 40)
print(r.x)        // 0 — field access, exactly as on a record
print(type(r))    // "Rect"
```

Fields are comma-separated, on one line or many, and follow the
[comma rule](syntax/commas.md) exactly as an `enum` body does — a newline is not
a separator, and a trailing comma before `end` is fine. Field annotations use
the same grammar as a parameter's, and an un-annotated field is `any`.

A `class` is a **top-level declaration**: it belongs at the top level of a file,
not inside a function, a loop or an `if`. Nesting one is an error
([below](#errors)). The declaration is also **hoisted** — the constructor and
the type name are both available throughout the file, above the `class` line as
well as below it — so declaration order within a file never matters.

A class name may not be a [built-in type name](#type-annotations): `class int`
and `class list` are errors. The built-in vocabulary wins in type position, so
such a class could never be named in an annotation — `x: int` would keep meaning
the primitive while `int(…)` built a record.

An instance **is a record** — it carries a tag naming its class, and nothing
else changes. `keys(r)`, `values(r)`, `r.x`, `r.x = 5` and printing all behave
as they do for `{x: …, y: …}`, and any function that takes a plain rect-shaped
record accepts one. Values stay immutable: `r.x = 5` produces a *new* instance
(still a `Rect`) rather than mutating this one. A spread, though, builds a plain
record — `{...r, label: "hi"}` is no longer that class's shape, so it loses the
tag.

### The class name as a type

The name works in every type position ([Type Annotations](#type-annotations)),
and checking follows the same warning-only rules:

```petal
fn center_x(r: Rect) -> int
  r.x + r.w / 2
end

center_x("nope")   // warning: argument 1 to `center_x`: expected `Rect`, found `string`
```

A field read is typed by its declaration, so with `class Point x: int end`,
`Point(4).x` is an `int`. An un-annotated field is `any` and reads as `any`. The
built-in `Rect`'s edges are declared [`num`](#type-annotations), since an edge
may be an `int` or a `float`. Two classes are never interchangeable, however
alike their fields. An instance *is* assignable to a `record` slot; a plain
record is not assignable to a class slot.

The declared fields are also the *only* ones the checker expects, so reading a
name the class does not declare is a warning:

```petal
fn label(r: Rect) -> string
  r.caption          // warning: class `Rect` has no field `caption`
end
```

A plain `record` or an un-annotated (`any`) value has no declared shape, so
neither warns. Method names are not fields, and never warn — `r.center_x()` is
resolved by [method dispatch](#resolution-order), not a field read.

### Methods

`fn <Class>.<name>(receiver, ...)` declares a method. The receiver is an
ordinary first parameter — the call site supplies it:

```petal
fn Rect.center_x(rect: Rect) -> int
  rect.x + rect.w / 2
end

fn Rect.shifted(rect: Rect, dx: int, dy: int) -> Rect
  Rect(rect.x + dx, rect.y + dy, rect.w, rect.h)
end

let r = Rect(0, 0, 100, 40)
print(r.center_x())            // 50
print(r.shifted(10, 0).center_x())   // 60
```

Methods may be declared on your own classes and on the built-in ones alike.
Like a function, a method becomes callable when its declaration runs, so declare
it before the top-level code that calls it. Two methods may share a name on one
class only if their arities differ (the same rule as
[function overloading](function-overloading.md)); the same name on *different*
classes is entirely independent.

A method's annotations are read at the call site, exactly like a function's: the
arguments you write are checked against the declared parameters, and the call
takes the declared return type.

```petal
fn Rect.scaled(rect: Rect, by: num) -> Rect
  Rect(rect.x, rect.y, rect.w * by, rect.h * by)
end

let r = Rect(0, 0, 10, 10)
r.scaled("2")                  // warning: argument 1 to `scaled`: expected `num`, found `string`
let s: string = r.scaled(2)    // warning: `s` declared `string` but assigned `Rect`
```

The receiver is not one of the arguments you write, so `argument 1` is the
first one in the parentheses.

This only happens when the call resolves to exactly one method — the same
condition as step 2 of [resolution order](#resolution-order) below. If a field
of that name could win, if the class declares no such method, or if the
receiver's type is unknown, the call stays `any` and nothing is checked, because
the declaration you can see may not be the code that runs.

The built-in `Rect` methods carry signatures too: `center_x`, `center_y`,
`right` and `bottom` return `num` (an int rect yields ints, a float rect
floats), while `inset` and `offset` return a `Rect`, so they chain.

### Resolution order

`value.name(args...)` tries, in order, and takes the first match:

1. **A callable record field** — `r.f()` where `f` is a field holding a
   function. Data beats declarations, and an instance is a record.
2. **A user-declared method** for the receiver's class — `fn Rect.area(...)`.
3. **A built-in method** of that class — `Rect.center_x` and friends. A user
   declaration therefore overrides a built-in method of the same name.
4. **The declaring slot's class**, when the label the receiver carries means
   nothing here — see [stale labels](#when-a-label-outlives-its-class) below.
   This is a last resort, reached only after 2 and 3 have found nothing.
5. **A global builtin**, with the receiver passed as its first argument — this
   is the [method syntax](#method-syntax) that makes `[1,2,3].len()` work.
   `p.str()` and `p.keys()` reach a class instance this way too, since an
   instance is a record.

Calling something none of these resolve reports the class by name:
`No method 'nope' on class Rect`. So does step 5 *failing* on a class
instance: `P(1).first()` is a call to a method that does not exist, not a call
to the global `first`, so it reports `No method 'first' on class P` rather than
the builtin's own complaint. That is what a live edit which deletes
`fn P.first` now reports.

An arity error counts the arguments you wrote. The receiver is supplied by the
call site, so a two-parameter `fn C.foo(c: C, n: int)` called as `C(1).foo()`
reports `C.foo() expects 1 argument, got 0` — the same wording a builtin uses.

### When the call is resolved at compile time

The order above is what happens *at runtime*, reading the class label the
receiver carries. But when the compiler can already tell which class the
receiver is, it skips the search and binds the call straight to that class's
method — the same thing a plain function call does. The receiver is pinned when
it is a constructor call, a `let` bound to one, or **any binding or parameter
carrying a class annotation**:

```petal
class C
  a: int,
end
fn C.value(c: C)
  c.a
end

let pinned = C(1)             // pinned: the initializer says so
state also: C = C(1)          // pinned: the annotation says so
fn takes_one(c: C)            // pinned: the parameter is annotated
  c.value()
end

state loose = C(1)            // not pinned — un-annotated, so `any`
fn takes_any(c)               // not pinned
  c.value()
end

print(pinned.value(), also.value(), takes_one(C(2)), loose.value(), takes_any(C(3)))
```

This never changes what a working program computes; it is the same method
either way. What it changes is *when the choice is made*, and two things follow
from that.

**Live editing.** A value in `state` outlives the edit that reshaped its class:
it keeps the fields it was built with and the label it was built under, because
a class instance is a plain record plus a name — never a link back to the code
that made it. Dispatching on that label means an old value can go looking for a
class the program no longer has. A pinned call instead runs the method the code
*now says*, so renaming a class or moving a method takes effect on the values
already live:

```petal
// before the edit
class C
  x: int,
end
fn C.value(c: C)
  c.x
end
state c: C = C(1)
print(c.value())        // 1
```

```petal
// after: the class is renamed and the method rewritten. `c` still holds the
// instance built above, labelled `C` — but the call is bound to `D.value`, so
// the edit lands on it.
class D
  x: int,
end
fn D.value(d: D)
  d.x + 100
end
state c: D = D(1)
print(c.value())        // 101, computed from the old value
```

Petal does **not** migrate state: a field the edit adds is not invented on an
instance built before it existed (`No field 'y' on class C`), and no value is
rewritten when a declaration changes. That is the same contract as changing a
state variable's type on reload.

### When a label outlives its class

Without the annotation the call above is not pinned, so it dispatches on the
label — and after the rename, `C` names nothing. Rather than dead-end there,
dispatch falls back to the class the *declaration* named (step 4 of the
[resolution order](#resolution-order)), and the edit lands anyway.

The fallback is deliberately a last resort, not a preference. It applies only
when the receiver's label is meaningless in the program now running: it names
no class here, or there is no label at all because the value predates the
class. A label naming a class that really exists always wins, so one binding
holding different classes over time keeps working:

```petal
class Circle
  r: int,
end
class Square
  s: int,
end
fn Circle.area(c: Circle)
  3 * c.r * c.r
end
fn Square.area(q: Square)
  q.s * q.s
end

state shape = Circle(2)
print(shape.area())        // 12 — dispatches on the label
shape = Square(3)
print(shape.area())        // 9  — still the label, not the declaration
```

So the two mechanisms answer different questions. An annotation says *this slot
is a C*, and the call is bound before it ever runs — predictable, and it gives
the dataflow tools an exact edge. The fallback says *nothing else could
answer*, and only rescues a value whose class has been edited out from under
it. Annotating is still worth doing; it just is not the difference between a
live edit working and failing any more.

**Dataflow.** A pinned call names its callee like any other call, so
`show-slice`, `show-provenance` and `show-graph` get the exact function. An
unpinned one is recovered as a *may*-edge to every method of that name, since
which one runs is not knowable until the receiver arrives.

Declaration order no longer decides this. Top-level declarations are
[hoisted](#declaration-order-top-level-fns-are-hoisted), methods included and in
source order, so a `c.bump()` written *above* its `fn C.bump` pins to that
method just as one written below it does.

### The built-in `Rect`

`Rect` is built into the language — no declaration and no import. Writing
`class Rect … end` yourself is allowed and *replaces* it for that program, the
same way a user binding shadows a builtin anywhere else. Its fields are
`x`, `y`, `w`, `h`, and it carries the geometry that layout code otherwise
rewrites by hand. See [Builtins.md](Builtins.md#built-in-classes) for the
methods.

```petal
let card = Rect(0, 0, 100, 40)
card.center_x()        // 50
card.right()           // 100  (x + w)
card.inset(5)          // Rect(5, 5, 90, 30)
card.offset(10, 10)    // Rect(10, 10, 100, 40)
```

An edge may be an `int` or a `float`, and stays the one it was given: there is
no implicit casting in Petal, so the constructor never truncates. A method's
result follows the same arithmetic the language does — `Rect(0, 0, 101, 40)`
centers at `50` (int division), `Rect(0.0, 0.0, 101.0, 40.0)` at `50.5`.

In `petal-ui`, `rect` is this same constructor exported under the prelude's own
name, so every rect an app already passes around gains the methods.

### Classes across files

Method dispatch spans the whole program rather than one file: a module can
declare `fn Rect.area(…)` and every file's rects gain it, and a file can extend
a class it imported.

The class *name* is an ordinary binding governed by `export`, and it is one
name covering both positions — exporting a class exports the constructor
`Point(…)` **and** the type `Point` in an annotation. A class its module does
not export is private in both. Two modules still may not declare the same class
name; the error names both files. See the
[Module System](module-system.md#methods-are-program-wide-a-class-name-follows-export).

### Errors

These are hard compile errors, not warnings — there is no reasonable code to
generate for any of them:

| Mistake | Message |
|---------|---------|
| `class Point` twice | `class \`Point\` is already declared` |
| `class Point` in two modules | ``class `Point` is already declared in `a.ptl`, so `b.ptl` may not declare it too`` |
| `class` inside a function or block | ``class `Point` must be declared at the top level of a file`` |
| `class int` (a built-in type name) | ``class `int` collides with the built-in type name `int`` `` |
| two fields named `x` | `duplicate field \`x\` in class \`Point\`` |
| `fn Nope.thing(...)` | `cannot declare a method on \`Nope\`: no class of that name` |
| two `fn Point.f(p)` | `method \`Point.f\` is already declared with 1 parameter` |
| `fn Point.f()` | `method \`Point.f\` declares no receiver parameter` |
| `fn Point.f(p: Other)` | `method \`Point.f\` declares its receiver \`p\` as \`Other\`, but a method on \`Point\` always receives an instance of \`Point\`` |

Each is reported at the field or declaration that is wrong, with the source
line and a caret under it.

The last two are the receiver rule. A method's first parameter *is* the
receiver, and the call site supplies it, so a method must declare one. `p.f(…)`
then only ever dispatches on a `Point`, so annotating that receiver with
anything a `Point` cannot fill describes a call that can never happen. `Point`,
`record`, `any` and no annotation at all are all fine.

### Not supported

There is no inheritance, no `self`/`this` (the receiver is a named parameter),
no private fields, no static methods, no constructor body, and no
`Rect{x: 0, …}` literal form — construct with the positional call. Classes are
a naming and dispatch feature over records, not an object system.

## Pattern Matching

The `match` expression tests a value against patterns:

```petal
fn describe(x)
    match x
        when 0 -> "zero"
        when 1 -> "one"
        when n -> "other: {n}"
    end
end
```

### Enum Patterns

```petal
enum Shape
    Circle(radius),
    Rect(w, h),
end

fn area(shape)
    match shape
        when Circle(r)  -> 3.14159 * r * r
        when Rect(w, h) -> w * h
    end
end
```

### List Destructuring

```petal
fn first(list)
    match list
        when [head, ...tail] -> head
        when []              -> nil
    end
end
```

### Guards

Guards add conditions to match arms:

```petal
fn classify(n)
    match n
        when x if x < 0   -> "negative"
        when 0             -> "zero"
        when x if x > 100 -> "big"
        when x             -> "small positive"
    end
end
```

## State

The `state` keyword declares a persistent variable: initialized once, then
keeping its value from one run of the program to the next, and across a
[hot reload](program-modification.md). It is how a script that re-runs every
frame remembers anything.

```petal
state hits = 0
hits += 1
print(hits)   // 1 on the first run, 2 on the second, ...
```

### One slot per call path

*Which* slot a declaration reads is decided by the declaration **and the path
that reached it**: the chain of callsites and loop iterations running from the
top of the program down to this execution. Reached the same way ⇒ the same
slot. Reached a different way ⇒ a different slot. That is React's `useState`
rule, with the call path standing in for the component instance.

The consequences are the whole point:

- **Two callsites of a helper hold two independent values.**
- **Recursion gets one slot per depth** — each recursive call extends the path.
- **A function called inside a `for` gets one slot per iteration**, so a list's
  positions key its rows automatically.
- **Two functions that declare the same state name never collide.** A
  declaration's identity is its name plus the enclosing module and function
  chain, so `count` in `fn a` and `count` in `fn b` are different declarations.
- **Top-level `state` is unaffected.** Module scope runs on the empty path, so a
  file-level declaration is exactly one slot, as it always was.

```petal
fn counter()
    state n = 0
    n += 1
    n
end

fn left()   counter() end
fn right()  counter() end

print(left(), right())   // 1 1 — two callsites, two slots
```

Within one run, a path is normally reached only once: reaching the same callsite
twice takes a loop or a recursive call, and both of those extend the path. So an
in-function counter counts *runs* (frames), not calls — the program above prints
`1 1`, then `2 2` on its second run. To count calls within a run, key the slot
explicitly or share one top-level cell (both below).

```petal
fn walk(n)
    state visits = 0
    visits += 1
    if n > 0 then walk(n - 1) end
    visits
end
print(walk(3))   // 1 — depths 3, 2, 1, 0 are four slots, each on its first visit
```

```petal
fn tick(label)
    state n = 0
    n += 1
    label ++ ":" ++ str(n)
end

for label in ["a", "b", "c"] do
    print(tick(label))   // a:1  b:1  c:1, then a:2  b:2  c:2 on the next run
end
```

Positional keying is React's un-keyed list behaviour, including its cost:
reorder the list and the values stay with the *positions*, not the items. When
it is the item's identity that matters, say so with a key.

### Keyed state — `state(key)`

`state(expr) name = …` keys the slot by the value of `expr` and **ignores the
call path entirely**. It is absolute: every execution of that declaration with
the same key value lands on the same slot, no matter which caller, how deep, or
which loop iteration it arrived from.

```petal
fn health(id, damage)
    state(id) hp = 100
    hp -= damage
    hp
end
print(health("goblin", 10), health("orc", 30), health("goblin", 5))  // 90 70 85
```

That makes `state(key)` the tool for anything whose identity outlives its
position: entities in a list that reorders, a tree node reached one level deeper
each frame, an input's repeat phase that two widgets are meant to share. Keys
are hashed, one slot per (declaration, key value), and a slot that goes
untouched for a whole run is reclaimed.

### Sharing on purpose

Because every caller is a different path, a helper cannot be used to launder a
shared value — funnelling reads and writes through one accessor function that
holds a single `state` gives each caller its own slot, not a shared one.
Deliberately shared state is a **top-level `state var`**: module scope is one
path, so there is exactly one cell, and `get`/`set` keep every live read and
write visible at the site that does it.

```petal
state var theme = "dark"

fn ui_theme()      get theme end
fn theme_set(t)    set theme = t end

theme_set("light")
print(ui_theme())   // light
```

The same trap has a quieter form: a **cache** behind an accessor. `state` only
runs its initializer on a slot's first touch, so wrapping an expensive build in
a function reads like a memo — but the memo is per path, so a caller inside a
loop rebuilds it on every iteration and never gets a hit.

```petal
fn char_table()
    state var t = build_table()   // once per callsite, once per iteration
    get t
end

state var char_table = build_table()   // once for the program — what you meant
```

Hoist the declaration to the top level when the cache is global, or key it with
`state(k)` when it varies by some argument (`state(ink) var rows =
build_rows(ink)` gives one build per ink, shared by every caller).

### Callbacks run at the builtin's callsite

`map`, `filter`, `reduce`, `forEach` and the other higher-order builtins call
your function from a single place — the builtin's own callsite — so a `state`
inside the callback has one slot shared by every element of that call:

```petal
fn seen(x)
    state n = 0
    n += 1
    n
end
print(map([10, 20, 30], seen))   // [1, 2, 3] — one shared slot, not three
```

Use a `for` loop when you want a slot per element, or key it: `state(x) n = 0`.

### `state var`

`state var` combines persistence with a [mutable cell](#var-and-set): the slot
holds the cell, so the value survives across runs and hot reloads *and* can be
written with `set` from inside another function or a callback.

```petal
state var hits = 0
set hits = hits + 1
print(hits)
```

The slot is path-keyed like any other, so a `state var` at the top level is one
shared cell while a `state var` inside a function is one cell per call path.
`state(key) var` works too — one cell per key, created on first touch. Reach for
`state var` only when a plain `state` cannot express the write; a `state` read
still carries its dataflow edges, and a cell read does not.

### State and hot reload

Every part of a slot's identity is derived from names, never from positions in
the file: the declaration from its name and enclosing module/function chain,
each callsite from the callee's spelling plus its ordinal among identically
spelled calls in that function. So editing elsewhere in the file — reordering
declarations, adding lines, reformatting — preserves state. Renaming a state
variable, renaming a callee, or inserting an *earlier* call to the same callee
changes an identity, and that slot (or that callsite's subtree of slots) starts
fresh. See
[program-modification.md](program-modification.md#state-preserving-hot-reload-transfer_state).

## JSX-like Elements

Petal supports a JSX-style element syntax for building tree-shaped data —
useful for DOM-like UIs in `petal-web` and diagram trees in `petal-diagram-canvas`.

```petal
let page = <div class="root">
    <h1>Hello, {name}</h1>
    <ul>
        <li>one</li>
        <li>two</li>
    </ul>
</div>
```

- Attributes are `name={expr}` or `name="literal"`.
- Text between tags is treated as a string child.
- Self-closing tags use `<Tag />`.
- `{expr}` embeds a Petal expression as a child.

Elements are runtime values; host embeddings (petal-web's renderer,
petal-diagram-canvas) walk the tree and produce DOM / canvas output.

## Function Overloading

Petal supports defining multiple functions with the same name but different
numbers of parameters. Dispatch happens at runtime by argument count:

```petal
fn greet()       print("hi") end
fn greet(name)   print("hi", name) end
fn greet(a, b)   print("hi", a, b) end
```

See [function-overloading.md](function-overloading.md) for the full rules.

## Assertions

```petal
assert(x > 0, "x must be positive")
assert_eq(total, expected)
```

`assert` aborts with `assertion failed: <msg>` plus the source location when
its condition is false. `assert_eq` reports both operand values on failure.
Both are built-ins — no import needed.

## Automatic Differentiation

Petal has built-in support for forward-mode automatic differentiation using dual numbers:

```petal
// Create a dual number: dual(value, derivative)
let x = dual(3.0, 1.0)  // x = 3, dx/dx = 1

// Arithmetic propagates derivatives automatically
let y = x * x + 2.0 * x  // y = x^2 + 2x

print(value_of(y))  // 15.0  (3^2 + 2*3)
print(deriv_of(y))  // 8.0   (2*3 + 2 = dy/dx at x=3)
```

Math builtins like `sqrt`, `abs`, `floor`, `ceil`, and `round` also support dual numbers.
