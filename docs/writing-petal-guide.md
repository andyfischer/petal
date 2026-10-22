# Writing Petal — a guide for programmers new to the language

This is a *how to write it* guide, not a reference. It assumes you already
program in something (JavaScript, Python, Rust, Lua) and want to be productive
in Petal in an afternoon. It covers the shape of a Petal program, the handful of
rules that differ from what you are used to, and — the part most guides skip —
how to use the `petal` tooling to answer questions about your own code instead
of guessing.

For the exhaustive rules, see the [Language Guide](language-guide.md),
[Builtins](Builtins.md) and [CLI Reference](CLI.md). This guide links into them
rather than repeating them.

---

## 1. Setup, and the loop you will actually use

[Getting Started](Getting_Started.md#install) has the one-line install of the
prebuilt `petal` binary. In a checkout of this repository you can build it
instead:

```bash
make build                       # binary at core/target/debug/petal
./tools/run-petal.ts run x.ptl  # wrapper: rebuilds if Rust source is newer
```

Hello world, and the one-liner form you will use constantly:

```bash
petal run hello.ptl
petal run -e 'print("hello, world!")'
petal hello.ptl                  # `run` is the default command
```

The inner loop that works well:

```bash
petal check app.ptl    # fast: compiles + type-checks, never runs. Do this first.
petal run app.ptl      # then run it
```

`check` is much cheaper than `run` and catches syntax errors, misspelled names,
arity errors, type mismatches and a few lints. Reach for it after every edit; `run` only when you
want output. More on the tooling in [§10](#10-the-tooling).

---

## 2. The 60-second shape of a program

A `.ptl` file is a sequence of statements executed top to bottom. There is no
`main`. Top-level `fn`, `class` and `enum` declarations are hoisted, so
declaration order inside a file does not matter for them.

```petal
class Task
  title: string,
  done: bool,
end

fn Task.label(t: Task) -> string
  let mark = if t.done then "x" else " " end
  "[{mark}] {t.title}"
end

let tasks = [
  Task("write guide", true),
  Task("test snippets", false),
]

for t in tasks do
  print(t.label())
end
```

```
[x] write guide
[ ] test snippets
```

Things to notice, because each is a rule you will meet again:

- **`end` closes every block.** `if … then … end`, `for … do … end`,
  `fn … end`. No braces, no significant indentation.
- **The last expression is the return value.** `Task.label` has no `return`.
  An explicit `return expr` exists for early exit; a bare `return` returns
  `nil`. A function declared `-> nil` is the exception: it has no implicit
  return ([Implicit Return Values](syntax/implicit-return-values.md)).
- **`{}` inside a string interpolates**, like a JS template literal but with the
  plain double quote: `"[{mark}] {t.title}"`.
- **Type annotations are optional and advisory.** `title: string` and
  `-> string` are checked at compile time and reported as *warnings*; they have
  no runtime effect. You can leave every one of them off. (The exception is
  `-> nil`, which means "this function returns nothing" and is compiled that
  way.)
- **`t.label()` is a method call**, and a method is just a function whose first
  parameter is the receiver. There is no `self`.
- **Commas are required** between list elements, and a trailing comma is fine.
- **Recursion works, including mutual recursion**, because top-level `fn`s are
  hoisted: a flood fill or a tree walk needs no forward declaration. A
  top-level `let` of literals (`let NODE_W = 140`) can be read by a hoisted
  `fn`; a `fn` that reads a value computed at run time cannot be called from
  above it, and `petal check` warns when one is. The rules are in
  [Declaration order](syntax/declaration-order.md).

---

## 3. Bindings: `let`, `var`, `state`

This is the part of Petal that has no direct equivalent in most languages, and
it is worth ten minutes up front. There are three binding forms and they answer
three different questions.

### `let` — the default, and a dataflow edge

```petal
let x = 10
x = 20          // rebinds the name; not a write to a slot
```

`x = 20` does not overwrite a cell — it *rebinds the name to a new value*. The
compiler therefore knows exactly which earlier value every read came from, which
is what makes the dataflow tools in [§10](#10-the-tooling) work. Use `let` unless
you have a specific reason not to.

Because it is a rebind, an `=` inside a function targeting a name bound outside
it is an **error**, not a silent shadow:

```
Error: `n` is bound outside this function; this assignment creates a local
shadow and does not modify `n`. Use `let` for a new local, return the value,
or — if it really must be mutable — declare it `var n = ...` and write it
with `set n = ...`
```

**A rebind inside a loop or an `if` carries out of it.** This is the rule that
decides how most Petal code is written, so learn it before you reach for
anything else: within one function, `=` reaches the enclosing scope, so an
ordinary accumulator needs no special machinery.

```petal
fn tally(xs)
  let total = 0
  for x in xs do
    total = total + x      // carries to the next iteration, and out of the loop
  end
  total                    // 6 for [1, 2, 3]
end

// Threading a value through a loop works the same way — this is how you write
// a simulation step, and why a *bare* `for` is not a dead end.
let flock = [1, 2, 3]
for i in range(0, 3) do
  flock = map(flock, fn(x) -> x * 2)
end
print(flock)               // [8, 16, 24]
```

The boundary is the **function**, not the block. `=` crossing into a nested
`fn` is the error above; `=` crossing into a `for`, `while`, `if` or `match` arm
in the same function is just a rebind.

### `const` — a top-level table that must stay constant

Write a top-level table, and anything derived from one, as `const`:

```petal
fn tile(name, cost)
  {name: name, cost: cost}
end

const TILES = [tile("grass", 1), tile("road", 0)]
const COSTS = map(TILES, fn(t) -> t.cost)
print(COSTS)
```

A `const` is a `let` that fails to compile if its value could ever differ
between two runs of the same source: if it reads `state` or a `var`, the
screen, the clock, `random()`, or calls a function that does, or one that
prints. The error names the chain (`TILES calls tile`, `tile calls print`),
so a debugging `print` left in a constructor is caught instead of quietly
turning a table into per-frame work. It is top level only, never assigned,
and holds data, not functions. Scalars (`let NODE_W = 140`) gain nothing from
it; use it where the value is a table or is computed. The full rule is in
[`const`](language-guide.md#const--a-value-that-must-be-constant).

**Do not park a constant table in `state`** to avoid rebuilding it. A `state`
cell keeps its first value across hot reloads, so the table stops following
your edits. `petal lint --rules-include=state-as-constant` reports these, and
with `--fix` rewrites them to `const`.

### `var` / `set` / `get` — the mutable escape hatch

Because of that carry rule, a plain accumulator does **not** need `var`. What
needs `var` is a write from inside a *nested function* — a callback, a closure,
or a recursive helper — which is exactly the case `=` refuses:

```petal
fn count_matching(xs, pred)
  var hits = 0
  forEach(xs, fn(x)
    if pred(x) then set hits += 1 end   // a plain `hits = …` here is an error
  end)
  get hits
end
```

Declare it `var`, and say so at every use:

| | |
|---|---|
| `var x = …` | declare the box |
| `set x = …` | write it |
| `get x` | read it — **required** when the read is inside a function other than the declaring scope |

`get` is required across a function boundary because the two readings differ: a
plain name is captured *by value where the function is written*, and `get` reads
the box *now*. Petal makes you pick, rather than making it depend on a
declaration hundreds of lines away. Forget it and the error tells you exactly
this:

```
Error: `n` is a `var` declared outside this function; write `get n` to read the
cell's current value.
```

Prefer `let`. Every `var` is a place the dataflow story goes dark, and
`petal graph` loses precision there. [`var`: mutable cells](syntax/var.md) has
the complete rules for [`var` and `set`](syntax/var.md#declaring-and-writing-a-cell)
and [`get`](syntax/var.md#reading-a-cell-get), and explains how cells work
underneath.
A `var` only earns its keep when a nested `fn` or lambda reads or writes it.
An accumulator that stays in one function is a `let` rebind, even across
loops and branches:

```petal
let fx = 30
fx += 40
fx += 40
print(fx)   // 110
```

`petal lint` makes this rewrite for you (`var` → `let`, dropping `set`/`get`)
wherever it is safe.

The other direction is a decision, so it is a command you run on one binding:
`petal apply-change convert-to-var app.ptl --target step/vx` turns that `let`
into a `var` (or a `state` into a `state var`), puts `set` on every write and
`get` on every read inside a nested function. See
[`petal apply-change`](#petal-apply-change--refactors-that-change-behaviour).

### `state` — persistence across runs

`state` declares a slot that is initialized once and keeps its value across
subsequent runs of the program and across hot reloads. This is what makes a
script that re-runs every frame able to remember anything. This section is
the short version; [Inline state](features/inline-state.md) is the reference.

```petal
state hits = 0
hits += 1
print(hits)    // 1 on the first run, 2 on the second, ...
```

A host that runs the script every frame keeps the slots between runs. `petal
run` is a single run, so it prints `1` every time and warns that the script
declares state; `petal run --state-storage hits.json app.ptl` keeps the slots
in a file between invocations.

The rule that surprises people: **which slot you get depends on the call path**
— the chain of callsites and loop iterations that reached the declaration. Same
path ⇒ same slot. This is React's `useState`, with the call path standing in for
the component instance.

```petal
fn counter()
  state n = 0
  n += 1
  n
end
fn left()   counter() end
fn right()  counter() end
print(left(), right())   // 1 1 — two callsites, two independent slots
```

Consequences worth memorizing:

- A helper **cannot** be used to launder shared state — each caller gets its own
  slot. For genuinely shared state use a **top-level `state var`**, where module
  scope is one path so there is exactly one cell.
- A `state` inside a `for` gets one slot per iteration — positional keying, with
  React's un-keyed-list cost when the list reorders. Give the loop a key,
  `for t in tasks key t.id do`, and each item's state (in the body and in
  every function it calls) follows the item instead. The key is relative to
  the loop, must be unique within one run of it, and a removed item's slots
  are dropped with no cleanup code.
- State that is shared on purpose lives in a top-level `state var`, a `Map`
  keyed by id when there is one value per entity; remove an entry when its
  entity goes away. A pure value computed from a key is a `memo(key)`.
  `state(id)` is deprecated.
- A slot that a run does not reach is dropped when that run ends. A `state`
  behind an `if` that was not taken, or one in a loop iteration for an item that
  is no longer in the list, starts from its initializer the next time it is
  reached.
  To keep a value through runs that do not use it, declare it at the top
  level, outside any condition
  ([How long a slot lives](features/inline-state.md#how-long-a-slot-lives)).

```petal
state var total = 0                          // one cell for the whole program

fn accumulate(v)
  set total = get total + v
  get total
end
```

*Every* function reads a module-level `var` with `get`, plain helpers called
from the top level as much as callbacks. Only module scope itself reads the
cell bare, and a bare read inside a function is a compile error.

```petal
state var hp = Map()                         // one entry per entity id

fn health(id, damage)
  set hp[id] = (get hp[id] ?? 100) - damage
  get hp[id]
end
```

`examples/console/state.ptl` and `examples/console/reactive_ui.ptl` are worth
reading once; they are the canonical treatment of this.

---

## 4. Values are immutable

Lists, records and class instances are values. Nothing mutates in place.

```petal
let nums = [1, 2, 3]
nums = append(nums, 4)      // append returns a NEW list — capture it
print(nums)                 // [1, 2, 3, 4]

let r = {a: 1}
let s = r
r.a = 2
print(r, s)                 // { a: 2 } { a: 1 }
```

`r.a = 2` is a *rebind of `r`* to an updated record, exactly like `r = …`. So a
function that assigns to a field of its parameter updates its own local binding,
not the caller's record. To share one record between writers, declare it `var`
and write it with `set r.a = …`.

### Indexing, and writing to a grid

`xs[i]` reads; `xs[i] = v` writes — and the write is a **rebind of `xs`**, on
exactly the same terms as `r.a = 2`. It nests, which is what makes a 2D board
practical:

```petal
let row = [1, 2, 3]
row[0] = 99                 // row is now [99, 2, 3]

let grid = for r in range(0, 3) do
  for c in range(0, 3) do 0 end
end                         // a 3x3 of zeros — nested collecting loops
grid[1][2] = 5              // one cell, by row then column
```

Since it is a rebind, the same function-boundary rule applies: a helper cannot
write a grid that its caller holds. When several functions — or a recursive one,
like a flood fill — must all write one board, that is the case `var` exists for,
and `set` takes index targets too:

```petal
var board = for r in range(0, 4) do
  for c in range(0, 4) do 0 end
end

fn fill(r, c)
  if r < 0 || r > 3 || c < 0 || c > 3 then return nil end
  if get board[r][c] == 1 then return nil end
  set board[r][c] = 1                    // `set` writes an index or a field
  fill(r + 1, c)
  fill(r, c + 1)
  nil
end
```

Note `get board[r][c]`: `get` binds tighter than `[]` and `.`, so it
dereferences the cell first and indexes the contents — `(get board)[r][c]`,
which is what you want.

### Helpers that write shared data

That flood fill is the general pattern for mutable data that more than one
function writes: the data goes in a cell, and every function that can see the
cell writes it with `set` and reads it with `get`. Where the cell is declared
decides which functions can see it:

- **Data that lives for one call** goes in local `var`s, with the helpers
  nested beside them as closures. The function stays a pure function of its
  arguments, and only the helpers say `get`.
- **Data that outlives the call** goes in a top-level `var` or, in an app that
  re-runs every frame, a `state var`, written by top-level functions. A plain
  `state` will not do: a function cannot write it with `=`.

Writes into a cell's list or record are in place, so moving a loop body into a
helper does not turn each write into a copy. Both shapes, the habits that keep
writes in place (including flat parallel lists for hot loops), and the worked
example in `examples/games/physics-playground` are in
[Mutable state shared by several functions](syntax/var.md#mutable-state-shared-by-several-functions).

The classic newcomer bug is dropping the result of a pure call. The compiler
lints it:

```
warning: result of `append` is discarded, so this call does nothing — `append`
returns a new value and never mutates its argument. Capture it, e.g.
`xs = append(xs, …)`.
```

The `@` **rebind operator** is shorthand for `x = f(x)`:

```petal
let nums = [1, 2, 3]
append(@nums, 4)      // same as: nums = append(nums, 4)
```

Use it where it reads well; it only works on `let` bindings.

---

## 5. Numbers and strings — the four things that bite

None of these produce an error. They produce a wrong answer, quietly, which is
why they are worth a page this early.

**`/` between two ints truncates.** There is no separate integer-division
operator; `/` is integer division when both sides are ints, and float division
as soon as either side is a float. It truncates *toward zero*, so it is not
`floor` on negatives. `%` is a remainder that follows the sign of the left
operand — not Python's always-positive modulo, so it does not wrap a negative
index for you.

```petal
print(7 / 2)          // 3     — not 3.5
print(-7 / 2)         // -3    — toward zero, not -4
print(7.0 / 2)        // 3.5   — one float is enough
print(float(7) / 2)   // 3.5   — how to force it
print(-1 % 5)         // -1    — not 4
print((-1 % 5 + 5) % 5)   // 4 — the wrap you probably meant
```

**Ints overflow loudly.** `9223372036854775807 + 1` aborts the run with
`Integer overflow when trying to add` rather than wrapping. Hand-rolling a
linear-congruential PRNG is the usual way to meet this; keep the modulus small
or work in floats.

**`floor`, `ceil` and `round` return floats** when given one, so an index has to
be cast: `xs[int(floor(t))]`, not `xs[floor(t)]`. They are int-preserving in the
other direction — `floor(7 / 2)` is the int `3`.

**Strings concatenate with `++`, never `+`.** `"a" + "b"` is a hard error (a
good one — it names `++` in the message). `++` also stringifies a number on
either side, and interpolation is usually nicer than either:

```petal
print("n=" ++ 5)          // n=5
print("n={5}")            // n=5   — prefer this
print(str(5) ++ "!")      // 5!    — `str` converts explicitly
```

**Every `{` in a string literal opens an interpolation hole**, so hand-written
JSON or a brace-bearing format string fails with
`Expected string part in interpolation, got Colon`. Escape the brace: `"\{"`.

```petal
print("\{\"a\": 1}")     // {"a": 1}
```

(For real JSON, `import json` gives you `json_stringify` and `json_parse`; see
[§9](#9-the-builtins-you-will-actually-use).)

**`nil` compares with `==` and `!=`**, and every other value is `!= nil`. So
`last(xs) == nil` and `if x != nil then … end` both work; `??` is the
shorthand for the common "or a default" case, and `?.` for reading a field of
something that may be nil.

**Repeat a string with `repeat(s, n)`**, which every ASCII-rendering program
wants (there is no `"-" * 20`):

```petal
print(repeat("-", 20))
print("[" ++ repeat("#", 3) ++ repeat(".", 7) ++ "]")   // [###.......]
```

**Do not build a long string by appending in a loop.** `out = out ++ piece`
makes a new string each time, copying everything written so far, so `n` appends
cost on the order of `n²` bytes — unnoticeable for a 40-column row, seconds for
a 1 MB report. Collect the pieces in a list and `join` them once:

```petal
let rows = for i in range(0, 3) do "row {i}" end
print(join(rows, "\n"))
```

`petal lint` rewrites the plain repeat loop (`for … do out = out ++ s end`) to
`repeat` for you. `pad_start(s, width)` / `pad_end` right- and left-align a
column, and `trim(s)`, `starts_with(s, prefix)` and `ends_with(s, suffix)` cover
the usual line parsing — between them most table and board rendering is a
one-liner.

**The operator set is small, and the boolean ones are symbols.** There is no
`not` / `and` / `or`. Those parse as identifiers, and the error says which
symbol to write: ``Expected ',' between arguments — Petal spells logical not
as `!` ``.

| | |
|---|---|
| arithmetic | `+  -  *  /  %` (and `+=  -=  *=  /=  %=`) |
| string | `++` (concatenate, and `++=`), `"{expr}"` (interpolate) |
| comparison | `==  !=  <  <=  >  >=` |
| boolean | `&&  \|\|  !` — short-circuiting |
| absent fields | `??` (fallback, and `??=`), `?.` (optional read) |
| other | `\|>` (pipe into first argument), `@` (rebind) |

**Put spaces around `<`.** Petal has a JSX-like element syntax, so a `<` with no
space after it opens a tag: `a<b` fails with `Expected ',' between arguments`,
while `a < b` compares. `>` is unambiguous and needs no space, which makes this
one asymmetric and easy to misread — space both, always.

---

## 6. Control flow you should know before you write anything

### `elsif`, not `else if`

```petal
fn classify(n)
  if n < 0 then "negative"
  elsif n == 0 then "zero"
  else "positive"
  end
end
```

`else if` opens a *nested* `if` that needs its own `end`. One `end` per chain
with `elsif`.

### `for` in value position builds a list

A bare `for` statement runs for side effects and allocates nothing. A `for`
whose value is *used* — assigned, returned, passed as an argument, used as a
record field, or in tail position — collects the last expression of each
iteration into a list.

```petal
let squares = for i in range(1, 6) do i * i end   // [1, 4, 9, 16, 25]

let tasks = [{title: "a", done: true}, {title: "b", done: false}]
let titles = for t in tasks do
  if t.done then continue end                     // `continue` filters
  t.title
end
```

Inside a collecting loop `continue` filters the iteration out and `break` ends
collection with what was gathered so far. Nested loops give you nested lists.
Both keywords also work with their ordinary meanings in a plain side-effect
`for` and in a `while`, including from inside a nested loop.

The gotcha: a side-effect loop at the *end of a function body* is in tail
position, so it collects. Declare the function `-> nil` if you don't want the
list — that turns the implicit return off, and the loop allocates nothing:

```petal
fn draw_all(items) -> nil
  for it in items do draw(it) end
end
```

`petal suggest` points these out, and tells `-> nil` from `-> list` by whether
any caller uses the result. Every position that does and does not capture a
loop is listed in [Implicit Return Values](syntax/implicit-return-values.md).

`while` is statement-only — no collecting form. A `for … end` that starts a
statement cannot be piped where it stands (a line `for x in xs do x end |>
len()` is a parse error, `Unexpected token: '|>'`); in value position it can
(`let n = for x in xs do x end |> len()`).

One thing to keep straight: a collecting loop cannot thread an accumulator — its
value is the list of its iterations, not a running total. When you want to carry
a value forward, use a **bare** `for` and rebind, as in
[§3](#3-bindings-let-var-state).

The one place tail position does *not* apply is the top level of a file: a
trailing `for` there runs for side effects and collects nothing, so the
discarded-result lint still fires on `for i in xs do append(a, i) end` written at
module scope — which is the bug it exists to catch.

### `match`

```petal
fn describe(n)
  match n
    when 0 -> "zero"
    when x if x < 0 -> "negative"
    when x -> "positive: {x}"
  end
end
```

Patterns cover literals, bindings, enum variants (`when Shape.circle(r) ->`), list
destructuring (`when [head, ...tail] ->`), alternatives (`when "a" | "b" ->`) and
guards (`when x if x > 100 ->`).

An arm's body is a statement list, like an `if` branch: write the statements on
the lines under `->`, and the last expression is the arm's value. `break`,
`continue` and `return` work in it as they do in an `if`.

```petal
fn apply(cmd, pos)
  match cmd
    when "left" | "right" ->
      let dx = if cmd == "left" then -1 else 1 end
      pos + dx
    when "home" -> 0
    when _ -> pos
  end
end
```

The catch-all is `when _ ->` (or `when name ->` if you want the value bound).
There is no `else` arm — `else -> …` is a parse error.

Leave the catch-all out and a value no arm matches is a runtime error, not a
`nil`. When every arm is a variant of one enum, leaving a variant out is a
compile error instead, so prefer no catch-all there: adding a variant to the
enum then points at every match that has to learn about it.

---

## 7. Data: records, classes, enums, and absent fields

```petal
let person = {name: "Alice", age: 30}
let moved  = {...person, age: 31}     // spread; later fields win
```

There are **no tuples** and no destructuring `let`: `let [a, b] = pair` is a
parse error (list patterns exist only inside `match`). A function that returns
two things returns a record, and the caller reads its fields — this is the
idiom, not a workaround:

```petal
fn slide(row)
  let shifted = filter(row, fn(x) -> x != 0)
  {row: shifted, gained: len(row) - len(shifted)}
end

let res = slide([2, 0, 0, 4])
print(res.row, res.gained)    // [2, 4] 2
```

**Reading a field a record does not carry is a hard error** — a typo'd field
name fails where it is written rather than reading as nil. Data that is
legitimately ragged (decoded JSON, a partial style record) says so:

```petal
let cfg = {}
print(cfg.window.width ?? 800)   // 800 — `??` makes its whole left side tolerant
print(cfg?.window.width)         // nil — `?.` when there's no fallback to write
```

Both soften *absent keys only*. A wrong-typed base (`3.width`) and an
out-of-bounds index stay hard errors, which is what you want.

`class` names a record shape, gives it a constructor and a type name, and lets
you hang methods on it — but an instance is still a plain record. No
inheritance, no `self`, no static methods, no private fields.

One caveat follows from that, and it is easy to trip over: **a spread drops the
class tag**. `{...c, radius: 3}` is a plain `record`, so it no longer dispatches
`c`'s methods (`No method 'area' on type record`). Field assignment keeps the
tag — `c.radius = 3` still yields a `Circle` — so prefer it, or rebuild through
the constructor.

```petal
class Circle
  radius: num,
end

fn Circle.area(c: Circle) -> num
  3.14159 * c.radius * c.radius
end

print(Circle(2).area())   // 12.56636
```

`enum` declares variants, with or without payloads, and pairs with `match`.
A variant is a member of its enum, in expressions and in patterns:

```petal
enum Shape
  circle(radius),
  rect(w, h),
end

fn area(s)
  match s
    when Shape.circle(r) -> 3.14159 * r * r
    when Shape.rect(w, h) -> w * h
  end
end

print(area(Shape.rect(2, 3)))   // 6
```

Reach for an enum whenever a value is one of a fixed set of names — a stage,
a kind, a command. Compared with a bare string, a misspelt variant does not
compile, and a `match` over the variants is checked to cover them all.

A variant can also carry constant data, which replaces a lookup table kept
beside the enum, and an enum lists and looks up its payload-free variants:

```petal
enum Kind
  parkway(shape: "route", lanes: 4),
  park(shape: "area", lanes: 0),
end

print(Kind.park.shape)           // area
print(Kind.park.name)            // park
print(Kind.all)                  // [Kind.parkway, Kind.park]
print(Kind.from("parkway"))      // Kind.parkway
print(Kind.from("lake"))         // nil
```

Payload fields are read by name as well (`Shape.rect(2, 3).w`). See
[Enums](language-guide.md#enums).

---

## 8. Idioms that make Petal code read like Petal

**A collecting `for` is your `map`.** `for` in value position builds a list
(§6), and with `if … then new else old end` in the body it is the whole
"replace one element" idiom — three lines, no index bookkeeping:

```petal
fn put_doc(ds, id, nd)
  for d in ds do
    if d.id == id then nd else d end
  end
end
```

**Pipe into the first argument.**

```petal
let tasks = [{title: "a", done: true}, {title: "b", done: false}]
let done_count = tasks |> filter(fn(t) -> t.done) |> len()
print(done_count)             // 1
```

**Lambdas are `fn(args) -> expr`, with no `end`.** The `->` introduces a
single expression body, which is why a lambda has no return-type annotation —
and why `filter(xs, fn(b) -> b.kind == "h" end)` fails with
`Expected ',' between arguments`: every named `fn … end` primes the habit, but
the arrow form takes no terminator. A multi-statement lambda is the block form
`fn(args) … end` (no arrow), and the parameter list can be left off when there
are no parameters: `fn 42 end`.

**Arguments can be passed by name**: `f(x: 1)`, in any order, after any
positional ones. Reach for it when a call is a row of same-typed literals —
`pill(x: 8, y: 4, w: 120, h: 24, label: "Save")` reads at a glance where
`pill(8, 4, 120, 24, "Save")` does not — and for class constructors with more
than two or three fields. Names bind to the callee's parameter slots directly;
no record is built, so it costs nothing a positional call doesn't beyond the
name lookup. The names are the callee's own: your `fn`s, lambdas, methods,
constructors and imported functions use the names they declare, and **builtins
use the names in the [Builtins Reference](Builtins.md)**
(`clamp(v, lo: 0, hi: 1)`, `xs.slice(start: 1, end: 3)`). The `ui` package's
draw calls take several shapes, and the names pick the shape:
`draw_circle(center: p, radius: 6, c: red)`; see the
[signature table](../core-libs/petal-ui/docs/components.md#draw-primitives--fills-and-strokes).
A wrong name is an error `petal check` catches. The binding rules and the
errors are in [Named Arguments](syntax/function-overloading.md#named-arguments).

**Parameters can have defaults**: `fn button(label, w = 120, h = 24, on = false)`,
called as `button("Save")` or `button("Save", on: true)`. Use one where most
callers pass the same value, instead of a second overload that only forwards.
The default is an *expression that runs on every call that omits the argument*
— not a value computed once — so `fn add(x, into = [])` gets a fresh list each
time and `fn stamp(at = time())` reads the clock each time, and it may use the
parameters before it (`fn box(w, h = w)`). Defaulted parameters go last;
passing `nil` is passing a value, not asking for the default. See
[Default Parameter Values](syntax/function-overloading.md#default-parameter-values).

**Method syntax reaches builtins, not your own functions.** `value.name(args)`
falls back to a **builtin** with the receiver as the first argument, which is
why `[1,2,3].len()` and `r.keys()` work. It does *not* fall back to a global you
declared: with `fn steer(b, k)` in scope, `b.steer(2)` fails with
`No method 'steer' on class B` (or `on type record`) — write `steer(b, 2)`, or
declare it as a method, `fn Boid.steer(b: Boid, k)`.

**Annotate where it buys you something.** Annotations are warnings-only, so they
cost nothing at runtime and are worth adding on public function signatures, on
`var` and `state` (where they are the *only* thing that makes those checkable),
and on parameters you want method calls pinned on. Use `num` for a slot that
takes an int or a float — that is most arithmetic.

The vocabulary is closed, and every name is a **single bare word**. The ones
you will write are `int`, `float`, `num` (int *or* float), `bool`, `string`
(alias `str`), `list`, `record`, `map`, `function`, `nil`, `vec2`, `vec3` and `any`,
plus the name of any class or enum; [Type Annotations](language-guide.md#type-annotations)
lists the rest. (`k: Kind` is a variant of the enum `Kind`: passing another
enum's variant warns, and so does reading a field no `Kind` has.) There are no
parameterized types: `list<int>` and record shapes are not expressible, so
`list` and `record` are opaque and their elements are never checked. An
unknown name warns (`unknown type name \`banana\``) rather than failing. An
`int` satisfies a `float` slot; the reverse needs an explicit `int(x)`.

**Mark your tuning knobs with `config let`.** It evaluates exactly like a `let`;
what changes is that the goal-based editing tools and live-editing hosts prefer
to rewrite *those* bindings and leave the rest of the program alone
([`config let`](syntax/config-let.md)).

```petal
config let offset = 10
```

**Split files with `import`; declarations are private until marked `pub`.**

```petal ignore
// shapes.ptl
pub class Circle
  radius: num,
end
pub fn Circle.area(c: Circle) -> num
  3.14159 * c.radius * c.radius
end

// app.ptl
import ./shapes                // every export bare, plus the qualifier `shapes`
print(Circle(2).area())        // or: import ./shapes: Circle  /  import ./shapes as s
```

`import ./shapes` is `shapes.ptl` next to the importing file; a path with no
`./` or `../` in front names a *package* (`import bloom`, `import creative`).
A package is bundled with the runtime, or is a dependency listed in the
project's `petal.toml` ([Packages](packages.md)). The rest — what happens
when two modules export the same name, what `pub` covers, how modules share
state — is in the [Module System](module-system.md).

**Wrap long expressions by ending the line with the operator, not starting the
next one with it.** This is the guide's one genuine footgun, because getting it
wrong is silent. Most operators (`+`, `|>`, `&&`, `??`) continue a line from
either end — but `-` does not, since a leading `-` is a valid unary minus
starting a fresh statement. So this compiles and returns the wrong number.
`petal check` warns on the `-` line (and `check --strict` fails on it), but
`petal run` goes ahead:

```petal
fn score(a, b, c)
  1.0 * a
  - 2.0 * b
  - 3.0 * c      // only THIS line is the return value
end
print(score(1, 2, 3))   // -9.0, not -12.0
```

Every earlier line becomes a discarded statement. Put the operator at the end of
the line and it means what it looks like:

```petal
fn score(a, b, c)
  1.0 * a -
  2.0 * b -
  3.0 * c
end
print(score(1, 2, 3))   // -12.0
```

A weighted sum written down the page is exactly where this bites. When in doubt,
wrap the whole expression in parentheses, or check the value with
`petal run --observe`.

---

## 9. The builtins you will actually use

Most of these are in scope with no import. The ones that are not say which
package to import, and `petal check` names the import when you forget it
(``unknown function `lerp` … add `import creative` ``). This is
the working set, not the full list; [Builtins.md](Builtins.md) has the rest with
exact signatures. Reach for this table before you guess a name, because a wrong
guess costs a compile round-trip (`unknown function \`flatten\``).

| | |
|---|---|
| **Output** | `print(a, b, …)` — space-separated, newline at the end |
| **Length** | `len(x)` — list or string. A string counts characters (code points), as do `slice`, `s[i]` and `index_of`; `byte_len` for the UTF-8 size |
| **Build a list** | `range(a, b)` — `a`..`b-1`; `range(a, b, step)` counts by `step`, down when it is negative (`range(5, 0, -1)` is 5..1). `append(xs, v)`, `drop_last(xs)`, `last(xs)` |
| **Read a list** | `xs[i]`, `slice(xs, start, end)` (end-exclusive, negatives allowed), `contains(xs, v)`, `index_of(xs, v)` (`-1` if absent) |
| **Reshape** | `reverse(xs)`, `sort(xs)`, `sort_by(xs, fn(x) -> x.key)`, `sort(xs, fn(a, b) -> a - b)`, `flat(xs)` (**`flat`**, not `flatten`; one level), `zip(a, b)`, `enumerate(xs)` |
| **Higher-order** | `map(xs, f)`, `filter(xs, f)`, `reduce(xs, init, f)`, `forEach(xs, f)` — the list comes **first**. `f` may be a builtin: `map(words, upper)` |
| **Reduce and select** | `sum`, `product`, `mean`, `minimum`, `maximum`, `any(xs, p)`, `all`, `count`, `find`, `first`, `take(xs, n)`, `drop`, `min_by(xs, key)`, `max_by`, `top_k(xs, k, key)`, `argmin(xs)`, `argmax`, `group_by(xs, key)`, `flat_map(xs, f)`, `fill(n, v)` |
| **Format** | `fixed(x, places)`, `commas(n)`, `pad_start(v, w)`, `pad_end`, `format("%,.2f", x)`, `plural(n, "car")`, `list_and(xs)` |
| **Records** | `keys(r)`, `values(r)`, `has_field(r, k)`, `field(r, k, fallback)`, `remove(r, k)` |
| **Strings** | `++`, `join(xs, sep)`, `split(s, sep)`, `repeat(s, n)`, `trim(s)`, `starts_with(s, p)`, `ends_with(s, p)`, `upper`/`lower`, `pad_start(s, w)`, `char_at(s, i)`, `chars(s)`, `ord(c)`, `chr(n)` |
| **Numbers** | `abs`, `min(a, b)`, `max(a, b)` — **two arguments, not a list** — `floor`, `ceil`, `round(x)`, `round(x, places)`, `sqrt`, `pow`, `sign`, `clamp(v, lo, hi)`, `safe_div(a, b)` (`nil` instead of aborting on a zero divisor) |
| **Interpolate** (`import creative`) | `lerp(a, b, t)` (numbers, vectors, colors, number lists, records field by field), `map_range(v, a, b, c, d)`, `smoothstep(e0, e1, x)`, `distance(…)`, `mag(…)`, `choose(xs)` |
| **Trig** | `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2(y, x)`, `hypot(x, y)`, `pi()`, `radians`, `degrees` |
| **Convert** | `str(v)`, `int(v)`, `float(v)`, `parse_int(s)`, `parse_float(s)`, `type(v)`. `parse_*` return `nil` on bad text, so `parse_float(s) ?? 0.0` reads user input with a default |
| **Random** | `random()` (in [0, 1)), `random(hi)`, `random(lo, hi)`, `random_int(lo, hi)` (half-open) |
| **Stateless hash** | `hash_int(a, b?, c?, d?)` (0 to 2^53 − 1), `hash01(…)` (in [0, 1)), `hash_str(text, salt?)` — the same in every run |
| **Tables** | `interp(table, x)` — piecewise linear through `[[x, y], …]`, clamped at the ends |
| **Noise** (`import noise`) | `noise(x[, y[, z]])`, `noise_seed(n)` |
| **Vectors, color, JSON** | `import vec2` (`vec2`, `vec3`, `normalize`, `dot`, …), `import color` (`hsv`, `hsl`, `color_lerp`), `import json` (`json_parse`, `json_stringify`) |
| **Checks** | `assert(cond, msg)`, `assert_eq(actual, expected)` stop the program. `expect(cond, msg)`, `expect_eq(actual, expected)`, `expect_near(actual, expected, tolerance)` record the failure and carry on: use them inside a `test "name" do … end` block, which `petal test` runs and `petal run` does not ([testing.md](testing.md)) |

Three of these are shaped differently than you will expect:

- `min` and `max` take exactly **two** arguments. For the extent of a list,
  `minimum(xs)` and `maximum(xs)`; for the element with the smallest key,
  `min_by(xs, key)`.
- `noise` is centered on **0** and runs roughly −1..1, not 0..1. Terrain and
  heightmap code wants `(noise(x, y) + 1.0) / 2.0`, or `map_range`.
- Do not hand-roll a sort. `sort_by(rows, fn(r) -> r.score, "desc")` sorts by
  a key (a **number or a string** — anything else is an error) and is stable,
  so chaining two `sort_by` calls sorts by two keys. `sort` takes an **optional
  comparator** as a second argument (`sort(xs, fn(a, b) -> b - a)`), returning
  a number or a bool (`true` = `a` first). Without one it orders numbers and
  strings, and leaves records alone.

### Determinism without a command-line flag

`petal run --seed 42` pins `random`, `random_int` and `choose`, and
`noise_seed(n)` pins `noise` from inside the program. When a program must be
byte-identical under a bare `petal run f.ptl` with no flags, do not write a
generator: `hash_int(a, b?, c?, d?)` is a stateless hash of its arguments, the
same in every run and on every machine, so "the i-th pick of n for this seed"
is one call and needs no `var` to carry.

```petal
let SEED = 20260830
fn pick(i: int, limit: int) -> int
  hash_int(SEED, i) % limit
end
print(pick(0, 6), pick(1, 6), pick(2, 6))   // 1 2 1
```

`hash01` is the same hash as a float in `[0, 1)`, and `hash_str(text)` hashes
an id or a name. See [Builtins.md](Builtins.md#hash_inta-b-c-d--hash01a-b-c-d).

### Before you write a helper

Every row here is something a large Petal program wrote for itself, two to
four times over, while the language already had it. Read the left column
before you type `fn`. `petal lint` reports a function that reuses one of
these names (`shadows-builtin`) or only forwards to one
(`reimplements-builtin`).

| You are about to write | It exists |
|---|---|
| a loop that adds up a list, or multiplies it | `sum(xs)`, `product(xs)`, `mean(xs)` |
| a loop for the smallest or largest of a list | `minimum(xs)`, `maximum(xs)` (`min` / `max` take two numbers) |
| a "best so far" loop: the element with the lowest cost, the nearest, the richest | `min_by(xs, key)`, `max_by(xs, key)`; the index is `argmin(xs)` / `argmax(xs)` |
| `sort_by(xs, key, "desc")` and then the first few | `top_k(xs, k, key)` |
| `len(filter(xs, p)) > 0`, or a loop that sets a flag | `any(xs, p)`, `all(xs, p)`, `count(xs, p)` |
| a loop that returns the first match | `find(xs, p)` |
| a record of lists built with `append(field(by, k, []), x)` | `group_by(xs, key)` |
| `flat(for x in xs do … end)` | `flat_map(xs, f)` |
| `for i in range(0, n) do 0.0 end`, `fn zeros(n)` | `fill(n, 0.0)` |
| a `var` and a `set r[k] = v` only to give a record a field whose name is data | `{...r, [k]: v}` |
| `set xs[i] = get xs[i] + v`, `set r = {...get r, n: get r.n + 1}` | `set xs[i] += v`, `set r.n += 1` (also `-=`, `*=`, `/=`, `%=`, `++=`, `??=`) |
| `if r == nil then d else field(r, k, d) end`, a `gf(r, k, d)` helper | `r?.k ?? d` (`?.` reads through nil and through a missing field); `field(r, k, d)` when the key is computed; `has_field(r, k)` |
| a string of digits built a character at a time for `1,234,567` | `commas(n)`, `commas(x, 2)` |
| `str(round(x, 1))` and a patch for the missing zero | `fixed(x, 1)` — and it goes in a hole: `"{fixed(x, 1)} km"` |
| right-aligning a column with a loop of spaces | `pad_start(v, width)`, `pad_end(v, width)`, `format("%8.2f", x)` |
| `if n == 1 then "1 car" else "{n} cars" end` | `plural(n, "car")`, `plural(n, "city", "cities")` |
| `join(slice(xs, 0, n - 1), ", ") ++ " and " ++ xs[n - 1]` | `list_and(xs)`, `list_and(xs, "or")` |
| a `curve(table, x)` that walks `[x, y]` pairs | `interp(table, x)` |
| fifteen fields interpolated one at a time | `lerp(a, b, t)` on the two records (`import creative`) |
| mixing two colors channel by channel | `lerp(c1, c2, t)` or `color_lerp(c1, c2, t)` (`import color`) |
| `index_of(ALPHABET, c)` for a character's number | `ord(c)`, and `chr(n)` back |
| a multiply-and-modulo generator in a `var`, a string hashed with a character table | `hash_int(seed, i)`, `hash01(seed, i)`, `hash_str(text)` |
| a second parameter list of the same function with fewer arguments | a default: `fn button(label, w = 120)`; a call names what it passes: `button("Save", w: 80)` |
| a `{name, fn, params}` table and an `if name == …` chain to call the right one | `f(...args)` with a record (by name) or a list (by position), and `fn_info(f).params` for the schema |
| `fn check(name, ok)` with a counter and a printed "OK" line | `test "name" do … end` with `expect`, `expect_eq`, `expect_near`; `petal test` runs them. `assert` for a condition that must stop the program |
| breaking a paragraph into lines that fit a width (petal-ui) | `text_wrap(text, style, max_width)` |
| trimming a label to fit with an ellipsis (petal-ui) | `text_ellipsize(text, style, max_width)` |

The exact signatures are in [Builtins.md](Builtins.md); the two text
functions are in the [UI reference](ui-reference.md).

---

## 10. The tooling

Petal's compiler is unusually willing to answer questions about your program.
Using it beats reading your own code back to yourself.

### `petal check` — your fastest feedback

```bash
petal check app.ptl              # compile + type-check, don't run. exit 1 on any error
petal check --strict app.ptl     # warnings fail it too — use in CI
petal check --json app.ptl       # {"ok": true, "warnings": [...]} or a structured error
```

`check` sorts what it finds into errors and warnings. An **error** is a line
that fails whenever it runs, and `check` exits 1 on it: a call to, or read of,
a name nothing defines, a call whose argument count no overload accepts, a
named argument no parameter has. `petal run` starts with the same check and
does not run a program that has one.

**Warnings** are advice, and exit 0 unless you pass `--strict`: a mismatched
annotation, an unknown type name, a discarded pure call, a call to a function
that could not be hoisted above it. The complete lists are in
[Errors and warnings](CLI.md#errors-and-warnings).

A name is defined by a declaration in the file, by an import, or by being a
core builtin; no host is consulted. So a script for a graphical host starts
with `import ui`, and it checks clean under the plain CLI even though only that
host can run it. `petal fix-imports app.ptl` adds the import lines a script is
missing. [Which names exist](CLI.md#which-names-exist) says where `check`
finds each host's package.

With `--json`, every error carries a `phase` naming the stage that rejected
the program, and an `errors[]` array with *every* diagnostic rather than only
the last ([Error phases](CLI.md#error-phases)).

### `petal fmt` and `petal lint` — layout and spelling, mechanically

```bash
petal fmt app.ptl            # rewrite in the canonical layout (whitespace only)
petal fmt --check app.ptl    # CI mode: write nothing, exit 1 if it would change
petal lint app.ptl           # report code that has a better spelling; exit 1 if any
petal lint --fix app.ptl     # apply the fixes in place
```

`fmt` has one layout and no options: 2-space indentation and consistent
spacing, with strings, comments and hand-made alignment left alone. `lint`
runs named rules, each with a fix: a `var` that no nested function shares
becomes a `let`, `x = x + e` becomes `x += e`, a cast of a value already of
that type is dropped, an `if`/`elsif` chain over literals becomes a `match`.
`petal lint --rules` lists them, and `--fix --verify` proves a rewrite against
the compiled IR before writing it.

The rules also know the spellings that a builtin or an operator already
covers, and fix them where the rewrite is provably the same program
([CLI.md](CLI.md#lint--rules-with-fixes) has the table):

| Written | Lint says | Rule |
|---|---|---|
| `float(i) * 0.5` (an int beside a float) | `i * 0.5`: the arithmetic converts it | `no-redundant-cast` |
| `set xs[i] = get xs[i] + d` | `set xs[i] += d` | `prefer-compound-assign` |
| `set T = {...get T, rev: get T.rev + 1}` | `set T.rev += 1` | `prefer-field-set` |
| `len(filter(xs, p)) > 0` | `any(xs, p)` | `prefer-any` |
| `0 - n` | `-n` (`0.0 - x` is reported only: `-0.0` is not `0.0`) | `neg-zero-sub` |
| `let dd = dist` and then only `dd` | read `dist` | `dead-alias` |
| `fn sum(xs) …`, `fn gf(r, k, d) if r == nil then d else field(r, k, d) end` | the builtin is already there | `shadows-builtin`, `reimplements-builtin` |

`/` on two ints truncates. You do not need `float()` on every operand to
guard against it: one float operand makes the arithmetic float, and `petal
check` warns about a literal `1 / 2`.

One rule is off unless you name it: `petal lint
--rules-include=state-as-constant` reports a top-level `state` that nothing
assigns and whose initializer is constant, and with `--fix` rewrites it to
`const`. That is right for a table and wrong for a cell your host writes by
name; put `// petal-lint-ignore state-as-constant` on such a line. See
[`fmt`](CLI.md#fmt--canonical-layout) and [`lint`](CLI.md#lint--rules-with-fixes).

### `petal suggest` — safe refactors, on request

```bash
petal suggest app.ptl                             # report every kind, write nothing
petal suggest --only named-args --apply app.ptl   # name the arguments of long calls
petal suggest --only types --apply app.ptl        # write the implied type annotations
```

Suggests changes that make a file say more. Three kinds are rewrites: **type
annotations** the program already implies, **named arguments** for calls that
pass three or more by position (`draw_rect(0, 0, 320, 48, PANEL)` becomes
`draw_rect(x: 0, y: 0, w: 320, h: 48, c: PANEL)`), and **return types** for a
function that ends in a loop (`-> nil` or `-> list`, see
[§6](#for-in-value-position-builds-a-list)). Nothing is written without
`--apply`, and `--apply` proves each change first; it leaves a call alone
rather than guess. The fourth kind, **advice**, is a comment such as "this
looks like a hand-written insertion sort — use `sort` or `sort_by`". It has no
fix attached; read it and decide. What counts as evidence and what each proof
checks are in [CLI.md](CLI.md#suggest--suggest-safe-refactors-for-a-file).

### `petal apply-change` — refactors that change behaviour

```bash
petal apply-change convert-to-var app.ptl --target /score --dry-run   # show the diff
petal apply-change convert-to-var app.ptl --target /score             # write it
petal apply-change convert-to-var physics.ptl --target step/vx        # a local, by path
```

`lint` and `suggest` keep the program the same. `apply-change` is for the
refactor that does not: you have decided a binding should be a mutable cell,
usually because the compiler said "`score` is bound outside this function",
and the change has to be made at every mention. `convert-to-var` rewrites the
declaration (`let` → `var`, `state` → `state var`), every `=` write on it
(→ `set`), and every read inside a nested function (→ `get`). Functions that
mention the binding now see the live value instead of the one captured where
they were written.

`--target` is a path of enclosing declarations ending in the binding: `/score`
for a module-level one, `step/vx` for a local of `step`. Get it wrong and the
error lists the paths it could have meant. Nothing is written unless every
touched file compiles afterwards. Target paths, `pub` bindings and the
refusals are in
[CLI.md](CLI.md#apply-change--refactors-that-change-behaviour).

### `petal run --observe` — what was everything set to?

The debugging tool to reach for first. It dumps the last value bound to every
named variable after the run — **including when the program errors**, which is
usually the run you care about.

```bash
$ petal run --observe -e 'let a = 2
let b = a * 10
print(b)'
20

Observed values (2):
  a = 2
  b = 20
```

Names are function-qualified (`list_row.sel` vs a top-level `sel`), and it's one
slot per binding with last-write-wins — so a loop temp reports its final
iteration.

### `petal explain` — why does this value look like that?

Runs the program with tracing, then walks *backward* through the dataflow graph
from a term, printing every recorded value along the chain:

```bash
$ petal explain --term b -e 'let a = 2
let b = a * 10
print(b)'
20
Explain t141 (b):
  Provenance chain:
    => t141 b [line 2, column 9] = 20
     . t139 - [line 2, column 9] = 2
     . t140 - [line 2, column 13] = 10
     . t138 a [line 1, column 9] = 2
```

`--term` takes a variable name, a term id (`72`), or `t72`. The ids are the
compiler's own numbering and change as the compiler does; the names and
positions are what to read.

### Static dataflow queries

These do *not* run the program:

| Command | Question |
|---|---|
| `petal graph --term x f.ptl` | What does this value depend on? |
| `petal graph --direction forward --term x f.ptl` | What downstream values does this influence? |
| `petal graph --term a --term b f.ptl` | The subgraph connecting several targets |
| `petal show-graph f.ptl \| dot -Tpng -o g.png` | Draw the whole dataflow graph |

All of them stop at a `var` cell and report it as a *frontier* entry naming the
cell, its declaration and every `set` that could have supplied the value — one
more reason to prefer `let`
([Cells and the frontier](CLI.md#cells-and-the-frontier)).

### Pipeline dumps

```bash
petal show-tokens -e 'let x = 1'      # lexer
petal show-ast    -e 'let x = 1 + 2'  # parser
petal show-ir     -e 'let x = 1 + 2'  # term graph
petal show-bytecode f.ptl             # VM lowering
petal show-static f.ptl               # which top-level declarations are static
```

All take `--json`. You mostly need these when a construct isn't parsing the way
you expected — `show-ast` settles precedence and continuation arguments in
seconds.

### Determinism, tracing, and the rest

- `petal run --seed 42 f.ptl` (or `PETAL_SEED=42`) makes `random`, `random_int`
  and `choose` replay identically — essential for reproducing a bug.
- `petal run --trace` streams per-term events to stderr;
  `--record-trace out.json` saves the buffer for offline analysis.
- `petal pending-report f.ptl` answers "why is this region blank?" for async
  resources.
- `petal run --profile f.ptl` prints an instruction/builtin histogram.
- `petal bench f.ptl --fn step` reports what one call of `step` costs:
  instructions, milliseconds, allocations and copies per call, with the
  optimizer on and off.
- `petal ir-equal a.ptl b.ptl` asks whether two files are the *same program*,
  ignoring spans and layout — the check behind refactor verification.

### Editors

`petal lsp` is a language server, and
[`editor-support/`](../editor-support/README.md) has the syntax highlighting.
[Getting Started](Getting_Started.md#editor-support) says how to set both up.

### MCP tools, if you use an AI assistant

`tools/petal-mcp.ts` exposes the same capabilities as tools: `TestSnippet`,
`CheckSnippet`, `ExplainTerm`, `ShowStage` (tokens / AST / IR / bytecode),
`PendingReport`, `TraceEmits`, `ProposeEdit`. The server builds the
binary on first use. See [dev/mcp-server.md](dev/mcp-server.md).

---

## 11. A debugging recipe

When a program does the wrong thing:

1. **`petal check --strict f.ptl`** — clear the errors and warnings first. An
   arity error or a type warning is frequently the actual bug, reported before
   you ran anything.
2. **`petal run --observe f.ptl`** — look at what everything actually held. The
   dump survives a crash, so this works on failing runs too.
3. **`petal explain --term <the wrong value> f.ptl`** — walk back to where the
   value came from, with real recorded numbers at every step.
4. **`petal graph --term <it>`** — if the chain crosses a `var`, the
   frontier block names every `set` that could be responsible.
5. **`petal run --seed N`** if randomness is involved, so step 2–4 look at the
   same run every time.

Errors themselves are already unusually specific — most of the ones you'll hit
tell you the fix in the message. A sample:

| You wrote | Petal says |
|---|---|
| `[1 2]` | ``Expected ',' between list elements`` |
| `set a = 2` on a `let` | ``` `a` is not a `var`; use `a = ...`, or declare it `var a = ...` ``` |
| `n = n + 1` inside a fn, `n` outside | ``` `n` is bound outside this function; this assignment creates a local shadow ``` |
| bare `n` inside a fn, `n` is a `var` | ``` write `get n` to read the cell's current value ``` |
| `el.title` where there is no `title` | ``No field 'title' on record``, plus the source of `el` |
| `f(1)` where every `f` takes 2 | ``` `f` expects 2 arguments, got 1 ``` — from `check`, before you run |
| a line starting with `-` under the line it should continue | ``` a line that starts with `-` is a new statement (a negation), not a continuation of the line above ``` — from `check` |
| `n + 1` on a line of its own | ``the value of this expression is discarded, so the line has no effect`` — from `check` |
| `state split = 0`, then `split(text, ",")` | ``` `split` here is the `state split` declared on line 1, which shadows the builtin `split` ``` — from `check` |
| `red` where `enum Color` declares `red` | ``` undefined variable `red` — did you mean Color.red? ``` — from `check` |
| `Color.rde` | ``enum `Color` has no variant `rde` — did you mean Color.red?`` — a compile error |
| a `match` over `Kind` variants with no arm for `Kind.lane` | ``this match does not cover every variant of `Kind`: no arm for `Kind.lane` `` — a compile error |
| a `match` where no arm matches the value | ``No matching pattern for value: "tin"`` — at run time; add `when _ ->` if that is expected |
| `when Shape.rect(w)` for `rect(w, h)` | ``the pattern `Shape.rect(…)` has 1 field, but Shape.rect has 2 payload fields (w, h), so its pattern is `Shape.rect(_, _)` `` — a compile error; write `Shape.rect(w, _)` |

---

## 12. Where to go next

| If you want… | Read |
|---|---|
| The full rules for anything above | [Language Guide](language-guide.md) |
| A compact map of the surface syntax | [Syntax Overview](syntax/overview.md) |
| Every builtin, with signatures | [Builtins](Builtins.md) |
| Every CLI flag and JSON schema | [CLI Reference](CLI.md) |
| `state` in full: call paths, keys, lifetime | [Inline state](features/inline-state.md) |
| What a hot reload keeps and rebuilds | [Hot Reload](features/hot-reload.md) |
| How the debugging tools fit together | [Debugging & Agent Visibility](dev/debugging-visibility.md) |
| Runnable programs to imitate | [`examples/console/`](../examples/README.md) |
| Multi-file programs | [Module System](module-system.md) |
| Dependencies, `petal.toml`, the package commands | [Packages](packages.md) |
| Building an app or a game on Petal | [Building Apps](usage/building-apps.md) |
| Why the language is shaped this way | [README](../README.md#goals) |

The single most useful next step is to read `examples/console/` end to end —
each file is a commented treatment of one feature, and they are all covered by
the test suite, so they are always current.
