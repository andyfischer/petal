# Petal Syntax Overview

A compact map of Petal's surface syntax: every lexical form, statement, and
expression the parser accepts, each with the page that explains it. It is an
index, not a tutorial. For walkthroughs and worked examples see the
[Language Guide](../language-guide.md).

Petal is a hybrid functional/imperative language. **Almost everything is an
expression** — `if`, `match`, `for`, and blocks all evaluate to a value — and
the last expression of a block or function body is its implicit result
([Implicit Return Values](implicit-return-values.md)).

## Lexical structure

### Comments

Line comments only, introduced by `//` and running to end of line. There is no
block-comment form.

```petal
let x = 1   // trailing comment
// whole-line comment
```

### Identifiers and keywords

Identifiers are letters, digits and underscores, not starting with a digit. A
leading `_` has no special meaning (visibility across files is controlled by
`pub`; see the [Module System](../module-system.md#exporting)). By convention
it marks an unused or internal name. A lone `_` is the wildcard pattern in
`match`.

Reserved keywords:

```
let  const  var  set  get  fn  if  else  elsif  then  for  in  while  match
when  do  end  return  break  continue  state  enum  import  pub  export
true  false  nil
```

`export` is the deprecated spelling of `pub`; it stays reserved and keeps
working.

`as` (in `import ui as u`), `class` (in `class Rect … end`), `config` (in
`config let x = …`), `native` (in `native fn sqrt(x: num) -> float`) and
`test` (in `test "name" do … end`) are **contextual**: recognised only in
those positions, not globally reserved. `class` in particular stays usable as
an ordinary identifier and as the JSX attribute `<div class="…">`.

A reserved keyword is still usable as a **member name**, where nothing else
could be meant: `{when: 3, end: 10}`, `r.when`, and the same names in record
patterns.

### Literals

| Kind | Examples | Doc |
|------|----------|-----|
| int | `42`, `-3`, `0` | [Types](../language-guide.md#types) |
| float | `3.14`, `-0.5`, `1.0`, `.001` (leading zero optional), `1e9`, `1.5e-3`, `2E+4` (scientific) | |
| bool | `true`, `false` | |
| nil | `nil` | |
| string | `"hello"`, `"line\n"`, `"2 + 2 = {2 + 2}"` (interpolation) | [String Operations](../language-guide.md#string-operations) |
| raw string | `"""...multi-line, no escapes or interpolation..."""` | [Raw strings](../language-guide.md#raw-triple-quoted-strings) |
| color | `#f80`, `#ff8800`, `#f80a`, `#ff8800aa` (desugar to `{r,g,b[,a]}` records) | [Color Literals](../language-guide.md#color-literals) |
| list | `[1, 2, 3]` | [Lists](../language-guide.md#lists) |
| record | `{name: "Alice", age: 30}` | [Records](../language-guide.md#records) |
| enum variant | `Color.red`, `Color.custom(255, 0, 0)` | [Enums](../language-guide.md#enums) |

In an ordinary string `{expr}` is an interpolation hole, so a literal brace is
written `"\{"` or put in a raw string. See
[String Interpolation](../language-guide.md#string-interpolation).

### Separators

Statements are separated by newlines; a `;` also separates two statements on
one line. A binary operator at the end of a line, or at the start of the next
one, continues the expression ([Line Continuation](line-continuation.md)).

**Commas are required** between adjacent elements of every comma-separated
construct. Whitespace and newlines are not separators (`[1 2]` is a parse
error); a trailing comma is always allowed. See [Commas](commas.md).

## Operators

Listed loosest to tightest binding (the parser's precedence ladder):

| Level | Operators | Notes |
|-------|-----------|-------|
| pipe | `\|>` | `x \|> f` ≡ `f(x)` (value becomes first arg) |
| logical or | `\|\|` | short-circuit |
| logical and | `&&` | short-circuit |
| nil-coalescing | `??` | `a ?? b` → `b` when `a` is `nil` or an absent record field; RHS short-circuits |
| equality | `==` `!=` | by value; see [Comparison](../language-guide.md#comparison-and-logical-operators) |
| comparison | `<` `<=` `>` `>=` | |
| concat | `++` | strings and lists |
| additive | `+` `-` | scalar; also broadcasts a scalar over a list (`[1,2,3] + 10`) |
| multiplicative | `*` `/` `%` | scalar; `*` and `/` also broadcast a scalar over a list |
| unary | `-` `!` | negation, logical not |
| postfix | `f(...)` `x[i]` `a.b` `a?.b` `a?.[i]` | call, index, field access, optional access |

`??` binds tighter than comparison but looser than `++`, so `count ?? 0 > 5`
parses as `(count ?? 0) > 5`. On its left side, and after `?.`, a missing
record field is nil rather than an error. See
[Ragged records](../language-guide.md#ragged-records--reading-a-field-that-may-not-be-there).

**Assignment** is a statement, not an operator: `x = e`, plus the compound forms
`+=` `-=` `*=` `/=` `%=` `++=` `??=`
([Compound Assignment](../language-guide.md#compound-assignment)). Assignment
targets may be a variable, an index (`xs[0] = v`), or a field (`p.x = v`,
including nested `a.b.c = v`; see
[Field assignment](../language-guide.md#field-assignment)). `set x = e` takes
the same target and compound forms; which keyword a name accepts is fixed by
its declaration (see [`var`, `set` and `get`](#var-set-and-get)).

### Sugar that desugars to calls

| Form | Desugars to | Doc |
|------|-------------|-----|
| `x \|> f(a)` | `f(x, a)` | [Pipe Operator](../language-guide.md#pipe-operator) |
| `obj.method(a)` | `method(obj, a)` | [Method Syntax](../language-guide.md#method-syntax), [resolution order](../language-guide.md#resolution-order) |
| `f(@x)` | `x = f(x, ...)` | [Rebind Operator](rebind-operator.md) |
| `#ff8800` | `{r: 255, g: 136, b: 0}` | [Color Literals](../language-guide.md#color-literals) |

## Statements

A program is a sequence of statements. `import` statements, if any, must come
first.

### `import`

Only valid before any other statement in a file:

```petal ignore
import ui                    // plain: every export bare, plus ui.button(...)
import ui as u               // the same, with the qualifier renamed: u.button(...)
import ui: button, clicked   // selective: only button(...) and clicked
import ./level               // a file beside this one
import ../shared/rng         // a file above it
import bloom/menu            // a module inside a package
pub import ./menu            // re-export every export of the module
```

A path is relative (`./`, or one or more `../`, then `/`-joined identifiers)
or starts with a package name. See the [Module System](../module-system.md)
for name resolution and exports, and [Packages](../packages.md) for
`petal.toml` and dependencies.

### `pub`

`pub` before a top-level declaration makes it visible to importing files:
`pub fn`, `pub let`, `pub const`, `pub config let`, `pub var`, `pub state`,
`pub enum`, `pub class`, and `pub import` to re-export. Everything else is
private to its file. See [Exporting](../module-system.md#exporting).

### `let` and assignment

```petal
let x = 10
let name: string = "Petal"   // optional type annotation (see Types)
x = 20                       // reassignment
x += 5                       // compound assignment
config let offset = 4        // a declared tuning knob
const TILES = [1, 2, 3]      // a `let` that must be static (top level only)
```

`let` declares a dataflow binding and `=` rebinds it
([Variables](../language-guide.md#variables)). An `=` inside a function may
not target a name bound outside it
([Cross-function assignment](var.md#cross-function-assignment)).

`config let` marks a binding as a value that tools and hosts are meant to
edit. It combines with `pub` and applies only to `let`. See
[`config let`](config-let.md).

`const NAME = expr` is a `let` with a compile-time check that the value
depends on nothing dynamic. It is top level only, is never assigned, and
combines with `pub` but not with `config`. See
[`const`](../language-guide.md#const--a-value-that-must-be-constant).

### `var`, `set` and `get`

```petal
var count = 0
for i in [1, 2, 3] do
    set count = count + i     // also `set count += i`
end

fn describe()
    "count: {get count}"      // `get` is required inside a function
end
print(describe())             // count: 6
```

`var` declares a mutable cell instead of a dataflow binding, `set` is the only
way to write one, and `get` reads one across a function boundary. `=` on a
`var` and `set` on a `let` are both compile errors. `set` takes a name, a
field or an index as its target, and the compound forms. See
[`var`: mutable cells](var.md).

### `state`

```petal
fn counter()
    state count = 0           // initialised once, kept across runs
    count += 1
    count
end

fn hit(id)
    state(id) var hp = 100    // keyed by value, and a mutable cell
    set hp = hp - 10
    hp
end
```

`state` declares a binding that keeps its value from one run of the program
to the next and across a hot reload. A slot is keyed by the call path that
reached the declaration, or by an explicit key with `state(key)`; `state var`
makes the slot a cell. See [Inline state](../features/inline-state.md).

### `fn` (function declaration)

```petal
fn add(a, b)
    a + b
end

fn abs(x: int) -> int        // optional param/return type annotations
    if x < 0 then return -x end
    x
end

fn box(w, h = w, label: str = "box") "{label} {w}x{h}" end   // defaults

fn show_all(xs) -> nil       // no implicit return
    for x in xs do print(x) end
end
```

| Feature | Doc |
|---------|-----|
| the last expression is the return value; `return` exits early; `-> nil` turns the implicit return off | [Implicit Return Values](implicit-return-values.md) |
| several declarations of one name with different parameter counts | [Overloading](function-overloading.md#overloading-by-argument-count) |
| `name = expr` parameter defaults, evaluated per call | [Default parameter values](function-overloading.md#default-parameter-values) |
| top-level `fn`s are usable above their declaration | [Declaration order](../language-guide.md#declaration-order-top-level-fns-are-hoisted) |
| `fn Class.name(receiver, …)` declares a method | [Methods](../language-guide.md#methods) |
| `native fn name(params) -> type` declares a function the host implements (no body) | [Native interfaces](../usage/ffi.md#native-interfaces-native-fn) |

### `class` (declaration)

```petal
class Point
    x: int,
    y: int,
end

fn Point.shifted(p: Point, dx: int, dy: int) -> Point
    Point(p.x + dx, p.y + dy)
end

let p = Point(1, 2).shifted(10, 0)   // an instance is a record with a class tag
```

A named record type: comma-separated fields with optional annotations. The
class name is a positional constructor and a type name. A `class` is top-level
only and hoisted. `Rect` (fields `x`, `y`, `w`, `h`) is built in. See
[Classes & Methods](../language-guide.md#classes--methods).

### `enum` (declaration)

```petal
enum Shape
    circle(radius, sides: 0),
    rect(w, h, sides: 4),
    unit(sides: 4),
end

print(Shape.circle(2).radius)    // 2
print(Shape.unit.sides)          // 4
print(Shape.unit.name)           // unit
print(Shape.all)                 // [Shape.unit]
print(Shape.from("unit"))        // Shape.unit
```

Named variants, written through the enum (`Shape.unit`, `Shape.circle(2)`).
Inside a variant's parens a bare identifier is a positional payload field and
`key: expr` is constant data. An `enum` is top-level only and hoisted, and its
name is a type. See [Enums](../language-guide.md#enums).

### `test` (test block)

A block `petal test` runs and an ordinary run does not. Top level only; the
name is a plain string, unique within the file. The body is a function body,
checked with `expect`, `expect_eq` and `expect_near`, which record a failure
and carry on. See [Testing](../testing.md).

```petal
test "addition" do
    expect_eq(1 + 1, 2)
    expect_near(0.1 + 0.2, 0.3, 0.000001)
end
```

### `break` / `continue` / `return`

Loop control and early function exit
([Break and Continue](../language-guide.md#break-and-continue)). Inside a
value-producing `for`, `continue` leaves the element out and `break` ends
collection early.

## Expressions

### Blocks

A block is a statement sequence delimited by a construct's keywords (e.g.
`then … end`, `do … end`). It evaluates to its last expression.

### `if` / `elsif` / `else`

An expression. `then` introduces each branch; a single `end` closes the whole
chain. `elsif` (one word, not `else if`) continues the same `if`. See
[If / Else](../language-guide.md#if--else).

```petal
let label = if x > 5 then "big" else "small" end

let color = if line.kind == "add" then GREEN
    elsif line.kind == "del" then RED
    else CONTEXT
    end
```

### `match` (pattern matching)

Tests a subject against `when` arms in order and evaluates the first whose
pattern matches. After `->` an arm's body is a statement list that runs to the
next `when` or the match's `end`. See
[Pattern Matching](../language-guide.md#pattern-matching).

```petal
enum Shape
    circle(radius),
    rect(w, h),
    unit,
end

let shape = Shape.rect(2, 3)

let area = match shape
    when Shape.circle(r)  -> 3 * r * r
    when Shape.rect(w, h) ->
        let a = w * h
        print("rect of {a}")    // rect of 6
        a
    when Shape.unit -> 1
end
print(area)                     // 6
```

| Pattern | Example | Matches |
|---------|---------|---------|
| wildcard | `_` | anything (no binding) |
| variable | `n` | anything, binds `n` (a bare identifier is always a binding, never a variant) |
| literal | `0`, `-1`, `"hi"`, `true`, `nil` | that value, as `==` would |
| enum variant | `Shape.unit`, `Shape.circle(r)`, `mod.Shape.rect(w, h)` | that variant; one sub-pattern per payload field, `_` to ignore one |
| list | `[head, ...tail]`, `[]`, `[a, b]` | list shape, `...rest` captures the tail |
| record | `{x: a, y: b}` | record with those keys, binding values |
| alternatives | `"a" \| "b"`, `Shape.unit \| Shape.circle(_)` | what any alternative matches; each binds the same names |

A **guard** adds a condition: `when x if x < 0 -> "negative"`. An arm must
have a body (`-> nil` for one that does nothing), and there is no `else` arm:
the arm for everything else is `when _ ->`. The older `when pattern do … end`
arm is still accepted. A match over the variants of one enum must
[cover every variant](../language-guide.md#exhaustiveness), and a match in
which [no arm matches](../language-guide.md#when-no-arm-matches) is a runtime
error, not `nil`.

### `for` (loop / mapping expression)

```petal
for item in [1, 2, 3] do print(item) end   // statement: side effects only

let squares = for i in range(1, 6) do i * i end
// squares == [1, 4, 9, 16, 25]
```

Iterates over a list or range with `in … do … end`. As a statement it runs for
side effects only. In value position it collects the last expression of each
iteration into a list. See [Loops](implicit-return-values.md#loops) for what
counts as value position.

### `while` (loop)

Statement-only; there is no value-collecting `while` form
([While Loops](../language-guide.md#while-loops)):

```petal
while x < 10 do
    x += 1
end
```

### Lambdas

```petal
let double = fn(x) -> x * 2          // arrow body
let describe = fn(x)                 // block body
    print(x)
    x * 2
end
let answer = fn -> 42                // no parameters: the list may be dropped
let greet = fn print("hi") end
```

Anonymous functions capture their enclosing scope and have no return-type
annotation. After `fn`, a leading `(` is always read as the parameter list.
See [Lambdas](../language-guide.md#lambdas) and
[Closures](../language-guide.md#closures).

### Calls and named arguments

```petal
fn scale(value, by, offset) value * by + offset end

print(scale(2, 10, 1))                      // 21
print(scale(2, by: 10, offset: 1))          // 21
print(scale(by: 10, offset: 1, value: 2))   // 21
```

An argument may be written `name: value`; positional arguments come first.
Builtins take names too. See
[Named arguments](function-overloading.md#named-arguments), and the
[Builtins Reference](../Builtins.md) for each builtin's parameter names.

The last argument may be a spread, `f(...args)`, which calls `f` with
arguments that are data: a list supplies them by position, a record by
parameter name (defaults honoured; an unknown or missing name is the error the
written call would give). Any callable takes one, builtins included. See
[Calling with a list or record of arguments](../language-guide.md#calling-with-a-list-or-record-of-arguments).

```petal
fn box(w, h = w) "{w}x{h}" end

print(box(...[3, 4]))    // 3x4
print(box(...{w: 5}))    // 5x5
print(box(2, ...{h: 9})) // 2x9
```

### Collection and access forms

```petal
[1, 2, 3]              // list literal
xs[0]                  // index (zero-based; negative counts from the end)
{name: "Alice"}        // record literal
{...defaults, x: 100}  // record spread (later fields override)
person.name            // field access
person?.name           // optional access: nil when the field is absent
```

See [Lists](../language-guide.md#lists), [Records](../language-guide.md#records)
and [Spread](../language-guide.md#spread).

### JSX-like elements

```petal
let page = <div class="root">
    <h1>Hello, {name}</h1>
    <ul><li>one</li><li>two</li></ul>
</div>
```

Builds a tree-shaped `element` value for a host to render. Attributes are
`name={expr}` or `name="literal"`; `{expr}` embeds a child expression; text
between tags is a string child; `<Tag />` self-closes. See
[JSX-like Elements](../language-guide.md#jsx-like-elements).

## Types (optional annotations)

```petal
let n: int = 0
var total: float = 0.0
state count: int = 0
state(id) var seen: list = []      // the annotation follows the name, before `=`

fn scale(v: float, k: float) -> float
    v * k
end

let double = fn(n: int) -> n * 2   // lambda params, but no lambda return type
```

`:` annotates a binding (`let`, `var`, or `state`), a parameter or a class
field; `->` annotates a named function's return type. Annotations are optional
and checked by `petal check` as warnings, with no effect at runtime except
`-> nil`, which turns off the function's
[implicit return](implicit-return-values.md#--nil-turns-the-implicit-return-off).

A type is a single bare name: `int`, `float`, `num` (either of the two),
`bool`, `string` (alias `str`), `list`, `record`, `function`, `enum`, `nil`,
`any`, host types such as `vec2`, `vec3`, `f64_array`, `element`, `symbol`,
`dual`, `handle`, `pending`, and the name of any class or enum in scope. There
are no parameterized, arrow, or structural forms. See
[Type Annotations](../language-guide.md#type-annotations) for the
assignability rules.

## See also

- [Language Guide](../language-guide.md) — the full tour with worked examples.
- [`var`: mutable cells](var.md) — `var`, `set`, `get`.
- [Function overloading, named arguments and default parameters](function-overloading.md).
- [Implicit Return Values](implicit-return-values.md) — function tails, collecting loops, `-> nil`.
- [Commas](commas.md) — where commas are required, and how `-` is disambiguated.
- [Line Continuation](line-continuation.md) — breaking a long expression across lines.
- [Rebind Operator](rebind-operator.md) — the `@` rebind operator.
- [Inline state](../features/inline-state.md) — the `state` keyword.
- [Module System](../module-system.md) — `import`, exports, resolution.
- [Builtins Reference](../Builtins.md) — built-in functions.
