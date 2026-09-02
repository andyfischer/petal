# Petal Syntax Overview

A compact map of Petal's surface syntax: every lexical form, statement, and
expression the parser accepts. It is a reference, not a tutorial. For
walkthroughs and worked examples see the
[Language Guide](../language-guide.md).

Petal is a hybrid functional/imperative language. **Almost everything is an
expression** — `if`, `match`, `for`, and blocks all evaluate to a value — and
the last expression of a block or function body is its implicit result.

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
`export`; see the [Module System](../module-system.md)). By convention it marks
an unused or internal name. A lone `_` is the wildcard pattern in `match`.

Reserved keywords:

```
let  var  set  get  fn  if  else  elsif  then  for  in  while  match  when  do
end  return  break  continue  state  enum  import  export  true  false  nil
```

`as` (in `import ui as u`), `class` (in `class Rect … end`), and `config` (in
`config let x = …`) are **contextual** — recognised only in those positions,
not globally reserved. `class` in particular stays usable as an ordinary
identifier and as the JSX attribute `<div class="…">`.

A reserved keyword is still usable as a **member name**, where nothing else
could be meant: `{when: 3, end: 10}`, `r.when`, and the same names in record
patterns.

### Literals

| Kind | Examples |
|------|----------|
| int | `42`, `-3`, `0` |
| float | `3.14`, `-0.5`, `1.0`, `.001` (leading zero optional), `1e9`, `1.5e-3`, `2E+4` (scientific) |
| bool | `true`, `false` |
| nil | `nil` |
| string | `"hello"`, `"line\n"` |
| raw string | `"""...multi-line, no escapes or interpolation..."""` |
| color | `#f80`, `#ff8800`, `#f80a`, `#ff8800aa` (desugar to `{r,g,b[,a]}` records) |
| list | `[1, 2, 3]` |
| record | `{name: "Alice", age: 30}` |
| enum variant | `Red`, `Custom(255, 0, 0)` |

**String interpolation.** Ordinary double-quoted strings interpolate `{expr}`
holes: `"2 + 2 = {2 + 2}"`. Triple-quoted **raw** strings capture their contents
verbatim — `{`/`}` are literal, backslashes are not escapes, and newlines are
allowed — which makes them ideal for embedding source or brace-heavy text.

To put a literal brace in an ordinary string, escape it (`"\{"`, `"\}"`) or
use a raw string (`"""{"""`). A bare `"{"` is an error, because the brace
opens a hole. A string opened inside a hole must close on the same line.

Inside a hole, a nested string may be written bare or backslash-escaped; the
two spellings mean the same thing:

```
"{if t then "a" else "b" end}"
"{if t then \"a\" else \"b\" end}"
```

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
| equality | `==` `!=` | |
| comparison | `<` `<=` `>` `>=` | |
| concat | `++` | string concatenation |
| additive | `+` `-` | scalar; also broadcasts a scalar over a list (`[1,2,3] + 10`) |
| multiplicative | `*` `/` `%` | scalar; `*` and `/` also broadcast a scalar over a list |
| unary | `-` `!` | negation, logical not |
| postfix | `f(...)` `x[i]` `a.b` `a?.b` `a?.[i]` | call, index, field access, optional access |

`??` binds tighter than comparison but looser than `++`, so `count ?? 0 > 5`
parses as `(count ?? 0) > 5`.

On the left side of `??`, a missing record field is nil rather than an error,
for the whole access chain (`cfg.window.width ?? 800`). `a?.b` (and `a?.[i]`)
gives the same tolerance without a fallback and short-circuits the rest of the
chain, so `cfg?.window.width` is nil when `window` is absent. Everywhere else
a missing field is an error. See
[Ragged records](../language-guide.md#ragged-records--reading-a-field-that-may-not-be-there).

**Assignment** is a statement, not an operator: `x = e`, plus the compound forms
`+=` `-=` `*=` `/=` `%=`. Assignment targets may be a variable, an index
(`xs[0] = v`), or a field (`p.x = v`, including nested `a.b.c = v`). `set x = e`
takes the same target and compound forms; which keyword a name accepts is fixed
by its declaration (see [`var`, `set` and `get`](#var-set-and-get)).

### Sugar that desugars to calls

| Form | Desugars to | Doc |
|------|-------------|-----|
| `x \|> f(a)` | `f(x, a)` | pipe |
| `obj.method(a)` | `method(obj, a)` | method syntax |
| `f(@x)` | `x = f(x, ...)` | [Rebind Operator](rebind-operator.md) |
| `#ff8800` | `{r: 255, g: 136, b: 0}` | color literal |

## Statements

A program is a sequence of statements separated by newlines. `import`
statements, if any, must come first.

### `import`

Only valid before any other statement in a file:

```petal ignore
import ui                    // qualified:  ui.button(...)
import ui: button, clicked   // selective:  button(...)
import ui as u               // alias:      u.button(...)
```

See the [Module System](../module-system.md) for resolution, exports, and hot
reload.

### `let` and assignment

```petal
let x = 10
let name: string = "Petal"   // optional type annotation (see Types)
x = 20                       // reassignment
x += 5                       // compound assignment
config let offset = 4        // a declared tuning knob (see the Language Guide)
```

`config` marks a binding as a value that direct manipulation may edit (see
[direct-manipulation.md](../direct-manipulation.md)). It combines with
`export` (`export config let`) and is not allowed on `var`.

### `var`, `set` and `get`

`var` declares a mutable cell instead of a dataflow binding, `set` is the only
way to write one, and `get` is how you read one across a function boundary. The
write keywords are disjoint: `=` on a `var` and `set` on a `let` are both
compile errors.

```petal
var count = 0
for i in [1, 2, 3] do
    set count = count + i     // also `set count += i`
end
print(count)                  // 6
```

A `var` is a box. A function or closure that writes the name writes the same
box, which plain `=` cannot express (an `=` inside a function only shadows).
Reading a `var` yields its contents.

Inside a function, reading an outer `var` is written `get`, and the keyword is
required:

```petal
var hits = 0
fn describe()
    "hits: {get hits}"
end
set hits = 2
print(describe())         // hits: 2
```

An ordinary binding is captured by value when the function is written; a cell
is read now. `get` makes that difference visible at the read. It binds tighter
than `.` and `[]` (`get cfg.w` is `(get cfg).w`) and is an error on anything
that is not a `var`. The matching rule for ordinary bindings: a function may
not capture a module binding that is rebound below it; pass it as a parameter
instead.

`set` never declares: `set` on an unknown name is an error. Targets may be a
name, a field, or an index (`set r.a = 1`, `set xs[0] = 1`). `@` is a `let`-only
rebind and is rejected on a `var`.

Prefer `let` where you can. See the
[Language Guide](../language-guide.md#var-and-set).

### `state`

Persistent variables that are initialised once and survive across runs (and
across hot reloads). Each slot is keyed by the call path that reached the
declaration, so each callsite of a helper, each recursion depth, and each
iteration of a caller's `for` holds its own value. A top-level `state` has one
slot. See [State](../language-guide.md#state) in the Language Guide.

```petal
fn counter()
    state count = 0
    count += 1
    count
end
```

`state(key)` keys by the value instead: same key, same slot, whoever asks.
`state var` combines persistence with a mutable cell:

```petal
fn hit(id)
    state(id) var hp = 100
    set hp = hp - 10
    hp
end
```

### `fn` (function declaration)

The last expression is the implicit return; `return` exits early. Functions may
be overloaded by arity (see [Function Overloading](../function-overloading.md)).

```petal
fn add(a, b)
    a + b
end

fn abs(x: int) -> int        // optional param/return type annotations
    if x < 0 then return -x end
    x
end
```

### `class` (declaration)

A named record type: comma-separated fields with optional annotations. The
class name is a positional constructor and a type name;
`fn <Class>.<name>(receiver, …)` declares a method on it. `Rect` (fields `x`,
`y`, `w`, `h`) is built in. See the
[Language Guide](../language-guide.md#classes--methods).

A `class` is top-level only and hoisted, so the constructor and type name are
usable anywhere in the file. Its name may not be a built-in type name, and it
is visible to other files only when `export`ed.

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

### `enum` (declaration)

Named variants, optionally carrying positional data:

```petal
enum Shape
    Circle(radius),
    Rect(w, h),
    Unit,
end
```

### `break` / `continue` / `return`

Loop control and early function exit. Inside a value-producing `for` (below),
`continue` filters the current element and `break` ends collection early.

## Expressions

### Blocks

A block is a newline-separated statement sequence delimited by a construct's
keywords (e.g. `then … end`, `do … end`). It evaluates to its last expression.

### `if` / `elsif` / `else`

An expression. `then` introduces each branch; a single `end` closes the whole
chain. `elsif` (one word, Ruby-style — not `else if`) continues the same `if`:

```petal
let label = if x > 5 then "big" else "small" end

let color = if line.kind == "add" then GREEN
    elsif line.kind == "del" then RED
    else CONTEXT
    end
```

### `match` (pattern matching)

Petal has a full pattern-matching `match` expression. It tests a subject against
`when` arms in order and evaluates the first whose pattern matches. Each arm body
is **either** a single-expression `-> expr` **or** a multi-statement `do … end`
block (the two forms are alternatives — do not combine them):

```petal
match shape
    when Circle(r)  -> 3.14159 * r * r
    when Rect(w, h) -> w * h
    when _ do
        log("unknown")
        0
    end
end
```

**Patterns** the parser accepts:

| Pattern | Example | Matches |
|---------|---------|---------|
| wildcard | `_` | anything (no binding) |
| variable | `n` | anything, binds `n` |
| literal | `0`, `-1`, `"hi"`, `true`, `nil` | that exact value |
| enum variant | `Circle(r)`, `Rect(w, h)` | that variant, binding fields |
| list | `[head, ...tail]`, `[]`, `[a, b]` | list shape, `...rest` captures the tail |
| record | `{x: a, y: b}` | record with those keys, binding values |

**Guards** add a boolean condition with `if`:

```petal
match n
    when x if x < 0 -> "negative"
    when 0          -> "zero"
    when x          -> "positive"
end
```

### `for` (loop / mapping expression)

Iterates over a list or range with `in … do … end`. A bare `for` statement
runs for side effects only. In value position (assigned, returned, passed as
an argument, or the last expression of a function body or branch) the same
loop collects the last expression of each iteration into a list:

```petal
for item in [1, 2, 3] do print(item) end   // statement: side effects only

let squares = for i in range(1, 6) do i * i end
// squares == [1, 4, 9, 16, 25]
```

Inside a collecting loop, `continue` skips the element and `break` ends
collection with what was gathered so far. Nested loops give a list of lists.

### `while` (loop)

Statement-only — there is no value-collecting `while` form:

```petal
while x < 10 do
    x += 1
end
```

### Lambdas

Anonymous functions use `fn` with no name and an `->` body. They capture their
enclosing scope (closures) and have no return-type annotation:

```petal
let double = fn(x) -> x * 2
let add = fn(a, b) -> a + b
```

A lambda that takes no arguments may drop the parameter list entirely, with
either an `->` body or an `end`-terminated block:

```petal
let answer = fn -> 42
let greet = fn print("hi") end
let nothing = fn end          // returns nil
on_click(fn set_count(count + 1) end)
```

After `fn`, a leading `(` is always read as the parameter list, so an argless
lambda's body cannot start with a parenthesized expression: `fn (a + b) * 2
end` fails. Write `fn -> (a + b) * 2` instead.

### Calls and named arguments

An argument may be written `name: value`, binding it to the parameter of that
name instead of to its position. Every positional argument comes first; once an
argument is named, every later one must be named too (going back is a parse
error):

```petal
fn scale(value, by, offset) value * by + offset end

print(scale(2, by: 10, offset: 1))          // 21
print(scale(by: 10, offset: 1, value: 2))   // 21
```

Overloads are chosen by the total argument count (positional plus named);
names then bind to the chosen variant's parameters. Naming a parameter that
does not exist, or giving one twice, is an error — and a warning from
`petal check` first, wherever the callee is known statically. Builtins do not
accept named arguments. See
[Named Arguments](../language-guide.md#named-arguments).

### Collection and access forms

```petal
[1, 2, 3]              // list literal
xs[0]                  // index (zero-based)
{name: "Alice"}        // record literal
{...defaults, x: 100}  // record spread (later fields override)
person.name            // field access
```

### JSX-like elements

A JSX-style syntax builds tree-shaped runtime values (used by `petal-web` and
`petal-diagram-canvas`):

```petal
let page = <div class="root">
    <h1>Hello, {name}</h1>
    <ul><li>one</li><li>two</li></ul>
</div>
```

Attributes are `name={expr}` or `name="literal"`; `{expr}` embeds a child
expression; text between tags is a string child; `<Tag />` self-closes.

## Types (optional annotations)

Type annotations are optional and advisory: they produce warnings, not
runtime casts. `:` annotates a binding (`let`, `var`, or `state`) or a
parameter; `->` annotates a named function's return type:

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

A lambda's `->` already introduces its body, so lambdas take parameter
annotations only.

Recognised type names: `int`, `float`, `num` (either of the two), `bool`,
`string` (alias `str`), `list`, `record`, `function`, `enum`, `nil`, `any`,
host types such as `vec2`, `f64_array`, `element`, `symbol`, `dual`, `handle`,
`pending`, and the name of any [class](#class-declaration) in scope. A type is
a single bare name; there are no parameterized (`list<int>`), arrow, or
structural forms. An unknown name gives an `unknown type name` warning. Type
names are not reserved, so `int`, `float` and `str` remain callable as cast
builtins. At runtime, `type(value)` returns a value's type name as a string.

See the [Language Guide](../language-guide.md#type-annotations) for
assignability rules and `petal check`.

## See also

- [Language Guide](../language-guide.md) — the full tour with worked examples.
- [Commas](commas.md) — where commas are required, and how `-` is disambiguated.
- [Line Continuation](line-continuation.md) — breaking a long expression across lines.
- [Module System](../module-system.md) — `import`, exports, resolution.
- [Function Overloading](../function-overloading.md) — multi-arity dispatch.
- [Rebind Operator](rebind-operator.md) — the `@` rebind operator.
- [Builtins Reference](../Builtins.md) — built-in functions.
