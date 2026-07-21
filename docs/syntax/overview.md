# Petal Syntax Overview

A compact map of Petal's surface syntax: every lexical form, statement, and
expression the parser accepts. It is a reference, not a tutorial — for prose
walkthroughs and worked examples see the
[Language Guide](../Language_Guide.md). Two syntactic topics have their own
deep dives: [Optional Commas](optional-commas.md) and the
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
`_` marks a name as module-private (see [Module System](../module-system.md)),
and a lone `_` is the wildcard pattern in `match`.

Reserved keywords:

```
let  fn  if  else  elsif  then  for  in  while  match  when  do  end
return  break  continue  state  enum  import  true  false  nil
```

`as` (in `import ui as u`) is **contextual** — recognised only after `import`,
not globally reserved.

### Literals

| Kind | Examples |
|------|----------|
| int | `42`, `-3`, `0` |
| float | `3.14`, `-0.5`, `1.0`, `.001` (leading zero optional) |
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

**Optional commas.** In every comma-separated construct (list literals, call
arguments, function parameters, record literals, enum declarations, and the
matching patterns) commas are optional; elements may be separated by newlines or
plain whitespace. This is the intended form for dense numeric grids like
`[0 0 1 0]` or `color(0 1 2)`. The rules — especially the spacing-aware `-` — are
subtle; see [Optional Commas](optional-commas.md).

## Operators

Listed loosest to tightest binding (the parser's precedence ladder):

| Level | Operators | Notes |
|-------|-----------|-------|
| pipe | `\|>` | `x \|> f` ≡ `f(x)` (value becomes first arg) |
| logical or | `\|\|` | short-circuit |
| logical and | `&&` | short-circuit |
| nil-coalescing | `??` | `a ?? b` → `b` only when `a` is `nil`; RHS short-circuits |
| equality | `==` `!=` | |
| comparison | `<` `<=` `>` `>=` | |
| concat | `++` | string concatenation |
| additive | `+` `-` | scalar; also broadcasts a scalar over a list (`[1,2,3] + 10`) |
| multiplicative | `*` `/` `%` | scalar; `*` and `/` also broadcast a scalar over a list |
| unary | `-` `!` | negation, logical not |
| postfix | `f(...)` `x[i]` `a.b` | call, index, field access |

`??` binds tighter than comparison but looser than `++`, so `count ?? 0 > 5`
parses as `(count ?? 0) > 5`.

**Assignment** is a statement, not an operator: `x = e`, plus the compound forms
`+=` `-=` `*=` `/=` `%=`. Assignment targets may be a variable, an index
(`xs[0] = v`), or a field (`p.x = v`, including nested `a.b.c = v`).

### Sugar that desugars to calls

| Form | Desugars to | Doc |
|------|-------------|-----|
| `x \|> f(a)` | `f(x, a)` | pipe |
| `obj.method(a)` | `method(obj, a)` | method syntax |
| `f(@x)` | `x = f(x, ...)` | [Rebind Operator](../rebind-operator.md) |
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
```

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

### `fn` (function declaration)

The last expression is the implicit return; `return` exits early. Functions may
be overloaded by arity (see [Function Overloading](../Function_Overloading.md)).

```petal
fn add(a, b)
    a + b
end

fn abs(x: int) -> int        // optional param/return type annotations
    if x < 0 then return -x end
    x
end
```

### `enum` (declaration)

Named variants, optionally carrying positional data:

```petal
enum Shape
    Circle(radius)
    Rect(w, h)
    Unit
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
(assigned, returned, passed as an argument, or as a list element) the same loop
becomes a **mapping** that collects the last expression of each iteration into a
list:

```petal
for item in [1, 2, 3] do print(item) end   // statement: side effects only

let squares = for i in range(1, 6) do i * i end
// squares == [1, 4, 9, 16, 25]
```

Inside a collecting loop, `continue` filters (contributes nothing) and `break`
ends collection, yielding what was gathered so far. To nest, **bind** the inner
loop so its list value is captured as the body's last expression.

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

Type annotations are **optional** and currently **advisory** (they drive
warnings, not runtime casts). A `:` annotates a `let` binding or a parameter; an
`->` annotates a function's return:

```petal
let n: int = 0
fn scale(v: float, k: float) -> float
    v * k
end
```

Recognised type names: `int`, `float`, `bool`, `string` (alias `str`), `list`,
`record`, `function`, `enum`, `nil`, `any`, plus host/runtime types such as
`vec2`, `f64_array`, `element`, `symbol`, `dual`, `handle`, `pending`. Unknown
names are ignored rather than rejected. At runtime, `type(value)` returns a
value's type name as a string.

## See also

- [Language Guide](../Language_Guide.md) — the full tour with worked examples.
- [Optional Commas](optional-commas.md) — comma-less lists and the spacing-aware `-`.
- [Module System](../module-system.md) — `import`, exports, resolution, hot reload.
- [Function Overloading](../Function_Overloading.md) — multi-arity dispatch.
- [Rebind Operator](../rebind-operator.md) — the `@` in-out argument operator.
- [Builtins Reference](../Builtins.md) — built-in functions.
