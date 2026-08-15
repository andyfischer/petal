# Petal Syntax Overview

A compact map of Petal's surface syntax: every lexical form, statement, and
expression the parser accepts. It is a reference, not a tutorial — for prose
walkthroughs and worked examples see the
[Language Guide](../language-guide.md). Two syntactic topics have their own
deep dives: [Commas](commas.md) and the
[Module System](../module-system.md).

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

Identifiers are letters/digits/underscore, not starting with a digit. A leading
`_` has no special meaning — module visibility is governed entirely by `export`
(see [Module System](../module-system.md)); by convention it marks an
intentionally-unused or internal name. A lone `_` is the wildcard pattern in
`match`.

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

To put a *literal* brace in an ordinary string, escape it (`"\{"`, `"\}"`) or
use a raw string (`"""{"""`). A bare `"{"` is rejected: the brace opens a hole,
and the quote meant to close the string would open a nested one instead. A
string opened inside a hole must also close on the same line, so a stray quote
cannot swallow the rest of the file and blame some innocent character hundreds
of lines further down.

Inside a hole, a nested string may be written bare or backslash-escaped — the
two spellings lex identically, in string holes and in JSX holes alike:

```
"{if t then "a" else "b" end}"
"{if t then \"a\" else \"b\" end}"
```

**Commas are required.** In every comma-separated construct (list literals, call
arguments, function parameters, record literals, enum declarations, and the
matching patterns) adjacent elements must be separated by a comma. Whitespace and
newlines are not separators — `[1 2]` is a parse error — while a trailing comma
before the closing delimiter is always allowed. See [Commas](commas.md).

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

The left side of `??` reads records *tolerantly*: a field or `[key]` the record
does not carry is nil there rather than an error, and that holds for the whole
access chain (`cfg.window.width ?? 800`). `a?.b` (and its index spelling
`a?.[i]`) asks for that same tolerance without a fallback, and — like
JavaScript's — short-circuits the rest of its chain, so `cfg?.window.width` is
nil when `window` is absent. Everywhere else a missing field stays a hard error,
as do a wrong-typed base (`3.x`) and an out-of-bounds list index, on either side
of the operator and under `?.`. See
[Ragged records](../language-guide.md#ragged-records--reading-a-field-that-may-not-be-there).

**Assignment** is a statement, not an operator: `x = e`, plus the compound forms
`+=` `-=` `*=` `/=` `%=`. Assignment targets may be a variable, an index
(`xs[0] = v`), or a field (`p.x = v`, including nested `a.b.c = v`). `set x = e`
takes the same target and compound forms; which keyword a name accepts is fixed
by its declaration (see [`var` and `set`](#var-and-set)).

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

The contextual `config` modifier marks a binding as the value direct
manipulation should edit (docs/direct-manipulation.md). It composes with
`export` (`export config let`) and is rejected on `var`.

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

A `var` binds a box, so a function or closure that mentions the name writes the
*same* box — the one thing `=` cannot express, since an `=` inside a function
only shadows. Reading a `var` yields its contents; no expression evaluates to
the box itself, so the only way to share one is closure capture.

Inside a function that read is written `get`, and the keyword is required
there:

```petal
var hits = 0
fn describe()
    "hits: {get hits}"
end
set hits = 2
print(describe())         // hits: 2
```

An ordinary binding is captured *by value at the point the function is
written*, while a cell is read *now*; `get` is what tells the two apart at the
read instead of at a distant declaration. It binds tighter than `.` and `[]`
(`get cfg.w` is `(get cfg).w`) and is an error on anything that is not a `var`.
The matching rule for ordinary bindings: a function may not capture a module
binding that is **rebound below it** — pass it as a parameter instead.

`set` never declares: `set` on an unknown name is an error. Targets may be a
name, a field, or an index (`set r.a = 1`, `set xs[0] = 1`). `@` is a `let`-only
rebind and is rejected on a `var`.

Prefer `let` — a `var` read has no dataflow edge behind it, so provenance
queries stop there. See the [Language Guide](../language-guide.md#var-and-set).

### `state`

Persistent variables that are initialised once and survive across calls (and
across hot reloads). The key to Petal's control-flow-keyed state model:

```petal
fn counter()
    state count = 0
    count += 1
    count
end
```

`state var` combines the two — a cell that persists — and `state(key) var`
gives each key its own cell:

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

A named record type: comma-separated fields with optional annotations (the same
[comma rule](commas.md) as every other list). The class name binds to a positional constructor and
becomes a usable type name; `fn <Class>.<name>(receiver, …)` declares a method
on it. `Rect` (fields `x`, `y`, `w`, `h`) is built in. See the
[Language Guide](../language-guide.md#classes--methods).

A `class` is **top-level only** — declaring one inside a function or block is an
error — and it is **hoisted**, so the constructor and the type name are both
live throughout the file. Its name may not be a built-in type name (`class int`
is an error), and it is visible to other files only when `export`ed.

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

Iterates over a list or range with `in … do … end`. A **bare** `for` statement
runs for side effects only and allocates nothing. Used in **value position**
(assigned, returned, passed as an argument, a list element, or sitting in a
**tail position** — the last statement of a function body, of a value-position
`if` branch, or of a collecting loop's body) the same loop becomes a **mapping**
that collects the last expression of each iteration into a list:

```petal
for item in [1, 2, 3] do print(item) end   // statement: side effects only

let squares = for i in range(1, 6) do i * i end
// squares == [1, 4, 9, 16, 25]
```

Inside a collecting loop, `continue` filters (contributes nothing) and `break`
ends collection, yielding what was gathered so far. Loops nest directly: an
inner loop in the outer body's tail position collects, giving a list of lists.

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

Type annotations are **optional** and **advisory** (they drive warnings, not
runtime casts). A `:` annotates a binding — `let`, `var`, or `state` — or a
parameter; an `->` annotates a *named* function's return type:

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

Recognised type names: `int`, `float`, `bool`, `string` (alias `str`), `list`,
`record`, `function`, `enum`, `nil`, `any`, plus host/runtime types such as
`vec2`, `f64_array`, `element`, `symbol`, `dual`, `handle`, `pending`, and the
name of any [class](#class-declaration) in scope — one this file declares or
imports, or a built-in one such as `Rect`. A type is
a single bare name — there are no parameterized (`list<int>`), arrow, or
structural forms. An unknown name is kept, not rejected, and reported as an
`unknown type name` warning. Type names are **contextual**, not reserved, so
`int` / `float` / `str` remain callable as the cast builtins everywhere else. At
runtime, `type(value)` returns a value's type name as a string.

See the [Language Guide](../language-guide.md#type-annotations) for
assignability rules and `petal check`.

## See also

- [Language Guide](../language-guide.md) — the full tour with worked examples.
- [Commas](commas.md) — where commas are required, and how `-` is disambiguated.
- [Line Continuation](line-continuation.md) — breaking a long expression across lines.
- [Module System](../module-system.md) — `import`, exports, resolution, hot reload.
- [Function Overloading](../function-overloading.md) — multi-arity dispatch.
- [Rebind Operator](rebind-operator.md) — the `@` in-out argument operator.
- [Builtins Reference](../Builtins.md) — built-in functions.
