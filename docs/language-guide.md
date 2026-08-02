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
    set c = c + 1
    c
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

The type names are exactly the ones `type(value)` reports (plus `any`), written
lowercase: `any`, `nil`, `bool`, `int`, `float`, `string` (alias `str`), `list`,
`record`, `function`, `enum`, `vec2`, `f64_array`, `element`, `symbol`, `dual`,
`handle`, `pending`. They are recognized only in type position, so `int`,
`float`, and `str` remain callable as the cast builtins everywhere else.

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
literal.

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

Used in **value position** — assigned to a name, `return`ed, passed as an
argument, or placed as a list element — a `for` loop evaluates to a **list**
built from the last expression of each iteration. This turns a loop into a
mapping:

```petal
let squares = for i in range(1, 6) do
    i * i
end
// squares == [1, 4, 9, 16, 25]

fn doubled(xs)
    return for x in xs do x * 2 end
end
```

A `for` loop only produces a list when its value is actually used ("captured").
A **bare `for` statement** runs purely for its side effects and allocates no
list — so existing loops keep their zero-overhead behavior:

```petal
for i in range(0, 3) do
    print(i)          // side effects only, no list built
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

To build a nested list, **bind the inner loop** so its value is captured, then
make it the body's last expression:

```petal
let grid = for row in range(0, 3) do
    let cells = for col in range(0, 3) do
        row * 10 + col
    end
    cells
end
// grid == [[0,1,2], [10,11,12], [20,21,22]]
```

An inner loop written as a bare statement is *not* captured, so each outer
iteration would collect `nil` — bind it (as above) when you want a nested list.

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

### Recursion

```petal
fn factorial(n)
    if n <= 1 then 1
    else n * factorial(n - 1)
    end
end
```

### Lambdas

Anonymous functions use `fn` without a name:

```petal
let double = fn(x) -> x * 2
print(double(5))  // 10
```

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
`Point(4).x` is an `int`. An un-annotated field is `any` and reads as `any` —
which is what the built-in `Rect`'s fields are, since an edge may be an `int` or
a `float`. Two classes are never interchangeable, however alike their fields. An instance
*is* assignable to a `record` slot; a plain record is not assignable to a class
slot.

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
instance: `P(1).get()` is a call to a method that does not exist, not a call to
the global `get`, so it reports `No method 'get' on class P` rather than the
builtin's own complaint. That is what a live edit which deletes `fn P.get`
now reports.

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
fn C.get(c: C)
  c.a
end

let pinned = C(1)             // pinned: the initializer says so
state also: C = C(1)          // pinned: the annotation says so
fn takes_one(c: C)            // pinned: the parameter is annotated
  c.get()
end

state loose = C(1)            // not pinned — un-annotated, so `any`
fn takes_any(c)               // not pinned
  c.get()
end

print(pinned.get(), also.get(), takes_one(C(2)), loose.get(), takes_any(C(3)))
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
fn C.get(c: C)
  c.x
end
state c: C = C(1)
print(c.get())        // 1
```

```petal
// after: the class is renamed and the method rewritten. `c` still holds the
// instance built above, labelled `C` — but the call is bound to `D.get`, so
// the edit lands on it.
class D
  x: int,
end
fn D.get(d: D)
  d.x + 100
end
state c: D = D(1)
print(c.get())        // 101, computed from the old value
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

One thing does not change: nothing in Petal hoists, so a call above its
`fn Class.method` is never pinned to it — it keeps the runtime search, which
finds the method by the time the call actually runs.

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

The `state` keyword declares persistent variables that survive across function calls.
State is initialized once and retains its value on subsequent calls:

```petal
fn counter()
    state count = 0
    count += 1
    count
end

print(counter())  // 1
print(counter())  // 2
print(counter())  // 3
```

State enables patterns like accumulators, caches, and reactive components:

```petal
fn running_average(value)
    state total = 0
    state count = 0
    total += value
    count += 1
    total / count
end
```

State is preserved during hot reload — if you edit and save a file while it's running,
existing state values carry over to the new code.

A `state` declaration can also be **keyed** — `state(key)` gives each distinct
key its own slot, so one declaration site holds independent state per entity:

```petal
fn health(id, damage)
    state(id) hp = 100
    hp -= damage
    hp
end
print(health("goblin", 10), health("orc", 30), health("goblin", 5))  // 90 70 85
```

Keys are hashed with the declaration's control-flow position, and a slot that
goes untouched for a run is reclaimed.

### `state var`

`state var` combines persistence with a [mutable cell](#var-and-set): the slot
holds the cell, so the value survives across calls and hot reloads *and* can be
written with `set` from inside another function or a callback.

```petal
state var hits = 0
set hits = hits + 1
print(hits)
```

`state(key) var` works too — one cell per key, created on first touch. Reach for
`state var` only when a plain `state` cannot express the write; a `state` read
still carries its dataflow edges, and a cell read does not.

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
