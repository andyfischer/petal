# Petal Language Guide

This is a user-facing reference for the Petal programming language. It covers all syntax,
types, operators, and features with examples.

For built-in functions (math, collections, color, vectors, autodiff, etc.) see
[Builtins.md](Builtins.md). For the `petal` CLI and the IR JSON schema, see
[CLI.md](CLI.md). For the design philosophy behind the language, see
[goals.md](goals.md).

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

Use `type(value)` to get the type name as a string at runtime.

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
    fn(x) -> x + n
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

### Rebind Operator

Prefixing a call argument with `@` assigns the call's result back to that
variable — shorthand for the common `x = f(x)` pattern that immutable values
produce:

```petal
let nums = [1, 2, 3]
append(@nums, 4)   // same as: nums = append(nums, 4)
print(nums)        // [1, 2, 3, 4]
```

`@var` binds to the nearest enclosing call. See the
[Rebind Operator](rebind-operator.md) doc for nesting, statement-level scope,
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
        state: "OR"
    }
}
print(user.address.city)  // "Portland"
```

### Mutation

Records are mutable. You can assign to a field directly, including nested
fields and fields of records stored inside lists.

```petal
let p = {x: 1, y: 2}
p.x = 10                     // direct field mutation
p.y = p.y + 1

let pts = [{x: 0, y: 0}, {x: 0, y: 0}]
pts[0].x = 100               // mutation inside a list

let user = {name: "Bob", address: {city: "Portland"}}
user.address.city = "Seattle" // nested field mutation
```

### Spread

Use `...expr` inside a record literal to copy all fields from another record.
Fields that follow the spread override the copied values.

```petal
let defaults = {x: 0, y: 0, color: "gray"}
let moved = {...defaults, x: 100}    // {x: 100, y: 0, color: "gray"}
```

Spread creates a new record; mutation modifies in place. Use whichever fits
the call site — spread for values you want to keep immutable, mutation for
loops that update the same object each iteration.

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
    Red
    Green
    Blue
    Custom(r, g, b)
end

let c = Red
let pink = Custom(255, 192, 203)
```

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
    Circle(radius)
    Rect(w, h)
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

See [Function_Overloading.md](Function_Overloading.md) for the full rules.

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
