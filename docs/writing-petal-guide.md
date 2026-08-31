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

Install a prebuilt binary:

```bash
curl -fsSL https://petal-lang.org/install.sh | sh    # installs to ~/.petal/bin
```

Or build from source in this repo:

```bash
make build                       # binary at rust/target/debug/petal
./ts/bin/run-petal.ts run x.ptl  # wrapper: rebuilds if Rust source is newer
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

`check` is much cheaper than `run` and catches syntax errors, arity errors, type
mismatches and a few lints. Reach for it after every edit; `run` only when you
want output. More on the tooling in [§8](#8-the-tooling).

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
- **`{}` inside a string interpolates**, like a JS template literal but with the
  plain double quote: `"[{mark}] {t.title}"`.
- **Type annotations are optional and advisory.** `title: string` and
  `-> string` are checked at compile time and reported as *warnings*; they have
  no runtime effect. You can leave every one of them off.
- **`t.label()` is a method call**, and a method is just a function whose first
  parameter is the receiver. There is no `self`.
- **Commas are required** between list elements, and a trailing comma is fine.

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
is what makes the dataflow tools in [§8](#8-the-tooling) work. Use `let` unless
you have a specific reason not to.

Because it is a rebind, an `=` inside a function targeting a name bound outside
it is an **error**, not a silent shadow:

```
Error: `n` is bound outside this function; this assignment creates a local
shadow and does not modify `n`. Use `let` for a new local, return the value,
or — if it really must be mutable — declare it `var n = ...`
```

### `var` / `set` / `get` — the mutable escape hatch

When you genuinely need one slot several places write to, declare it `var`, and
say so at every use:

```petal
fn tally(xs)
  var total = 0
  for x in xs do
    set total = total + x
  end
  total
end
print(tally([1, 2, 3]))   // 6
```

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
`show-provenance` / `show-slice` lose precision there.

### `state` — persistence across runs

`state` declares a slot that is initialized once and keeps its value across
subsequent runs of the program and across hot reloads. This is what makes a
script that re-runs every frame able to remember anything.

```petal
state hits = 0
hits += 1
print(hits)    // 1 on the first run, 2 on the second, …
```

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
  React's un-keyed-list cost when the list reorders.
- When identity matters more than position, key it: `state(id) hp = 100` ignores
  the call path entirely and gives one slot per key value.

```petal
state var total = 0                          // one cell for the whole program

fn accumulate(v)
  set total = get total + v
  get total
end

fn health(id, damage)
  state(id) hp = 100                         // one cell per entity id
  hp -= damage
  hp
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

## 5. Control flow you should know before you write anything

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

let titles = for t in tasks do
  if t.done then continue end                     // `continue` filters
  t.title
end
```

Inside a collecting loop `continue` filters the iteration out and `break` ends
collection with what was gathered so far. Nested loops give you nested lists.

The gotcha: a side-effect loop at the *end of a function body* is in tail
position, so it collects. Add a trailing `nil` if you don't want the list:

```petal
fn draw_all(items)
  for it in items do draw(it) end
  nil
end
```

`while` is statement-only — no collecting form.

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

Patterns cover literals, bindings, enum variants (`when Circle(r) ->`), list
destructuring (`when [head, ...tail] ->`) and guards (`when x if x > 100 ->`).
An arm whose body is several statements uses `do … end` instead of `->`.

---

## 6. Data: records, classes, enums, and absent fields

```petal
let person = {name: "Alice", age: 30}
let moved  = {...person, age: 31}     // spread; later fields win
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

```petal
class Circle
  radius: num,
end

fn Circle.area(c: Circle) -> num
  3.14159 * c.radius * c.radius
end

print(Circle(2).area())   // 12.56636
```

`enum` declares variants, with or without payloads, and pairs with `match`:

```petal
enum Shape
  Circle(radius),
  Rect(w, h),
end
```

---

## 7. Idioms that make Petal code read like Petal

**Pipe into the first argument.**

```petal
let done_count = tasks |> filter(fn(t) -> t.done) |> len()
```

**Lambdas are `fn(args) -> expr`** — the `->` introduces the body, which is why
a lambda has no return-type annotation.

**Method syntax works on anything**, because `value.name(args)` falls back to
calling a global with the receiver as the first argument: `[1,2,3].len()`.

**Annotate where it buys you something.** Annotations are warnings-only, so they
cost nothing at runtime and are worth adding on public function signatures, on
`var` and `state` (where they are the *only* thing that makes those checkable),
and on parameters you want method calls pinned on. Use `num` for a slot that
takes an int or a float — that is most arithmetic.

**Mark your tuning knobs with `config let`.** It evaluates exactly like a `let`;
what changes is that the goal-based editing tools and live-editing hosts prefer
to rewrite *those* bindings and leave the rest of the program alone.

```petal
config let offset = 10
```

**Split files with `import`; declarations are private until `export`ed.**

```petal
// shapes.ptl
export class Circle
  radius: num,
end
export fn Circle.area(c: Circle) -> num
  3.14159 * c.radius * c.radius
end

// app.ptl
import shapes: Circle          // or: import shapes  /  import shapes as s
print(Circle(2).area())
```

Imports must come before any other statement. Methods are program-wide — declare
`fn Rect.area(…)` in one module and every file's rects gain it — but the class
*name* follows `export`.

**Wrap long expressions** by ending a line with a binary operator, or starting
the next one with it (`+`, `|>`, `&&`, `??`, … but not `-` or `<`).

---

## 8. The tooling

Petal's compiler is unusually willing to answer questions about your program.
Using it beats reading your own code back to yourself.

### `petal check` — your fastest feedback

```bash
petal check app.ptl              # compile + type-check, don't run. exit 0/1
petal check --strict app.ptl     # warnings become a non-zero exit — use in CI
petal check --json app.ptl       # {"ok": true, "warnings": [...]} or a structured error
```

Warnings are non-fatal by design: mismatched annotations, arity errors, unknown
type names, a discarded pure call, a function capturing a module `state` that is
rebound below it. `--strict` is what you want in CI.

Every `--json` error carries a `phase` (`lex` / `parse` / `module` / `compile` /
`lower` / `runtime`) telling you exactly which stage rejected the program, and
an `errors[]` array with *every* diagnostic rather than only the last.

### `petal lint` — formatting, mechanically

```bash
petal lint app.ptl           # report; exit 1 if changes needed
petal lint-fix app.ptl       # rewrite in place (same as `lint --fix`)
petal lint --check app.ptl   # CI mode, silent on success
```

Two passes: 2-space re-indentation (comments, string contents and JSX text are
preserved byte for byte) and deletion of identity casts like `int(n)` where `n`
is already an `int`. Add `--verify` to make it prove the rewrite is IR-equal
before writing anything.

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
Explain t119 (b):
  Provenance chain:
    => t119 b [line 2, column 9] = 20
     . t117 - [line 2, column 9] = 2
     . t118 - [line 2, column 13] = 10
     . t116 a [line 1, column 9] = 2
```

`--term` takes a variable name, a term id (`72`), or `t72`.

### Static dataflow queries

These do *not* run the program:

| Command | Question |
|---|---|
| `petal show-provenance --term x f.ptl` | What does this value depend on? |
| `petal show-dependents --term x f.ptl` | What downstream values does this influence? |
| `petal show-slice --term a --term b f.ptl` | The subgraph connecting several targets |
| `petal show-graph f.ptl \| dot -Tpng -o g.png` | Draw the whole dataflow graph |

All of them stop at a `var` cell and report it as a *frontier* entry naming the
cell, its declaration and every `set` that could have supplied the value — one
more reason to prefer `let`.

### Pipeline dumps

```bash
petal show-tokens -e 'let x = 1'      # lexer
petal show-ast    -e 'let x = 1 + 2'  # parser
petal show-ir     -e 'let x = 1 + 2'  # term graph
petal show-bytecode f.ptl             # VM lowering
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
- `petal ir-equal a.ptl b.ptl` asks whether two files are the *same program*,
  ignoring spans and layout — the check behind refactor verification.

### Editors

`petal lsp` speaks LSP over stdio: diagnostics on open/change, hover,
go-to-definition and completion. Point your editor's LSP client at it.
Syntax highlighting lives in [`editor-support/`](../editor-support/README.md) —
a tree-sitter grammar (Neovim, Helix, Zed, Emacs) and a classic-Vim syntax file.

### MCP tools, if you use an AI assistant

`ts/tools/petal-mcp.ts` exposes the same capabilities as tools: `TestSnippet`,
`CheckSnippet`, `ExplainTerm`, `ShowIR`, `ShowBytecode`, `ShowAST`,
`ShowTokens`, `PendingReport`, `TraceEmits`, `ProposeEdit`. They build the
binary automatically. See [dev/mcp-server.md](dev/mcp-server.md).

---

## 9. A debugging recipe

When a program does the wrong thing:

1. **`petal check --strict f.ptl`** — clear the warnings first. An arity or type
   warning is frequently the actual bug, reported before you ran anything.
2. **`petal run --observe f.ptl`** — look at what everything actually held. The
   dump survives a crash, so this works on failing runs too.
3. **`petal explain --term <the wrong value> f.ptl`** — walk back to where the
   value came from, with real recorded numbers at every step.
4. **`petal show-provenance --term <it>`** — if the chain crosses a `var`, the
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

---

## 10. Where to go next

| If you want… | Read |
|---|---|
| The full rules for anything above | [Language Guide](language-guide.md) |
| A compact map of the surface syntax | [Syntax Overview](syntax/overview.md) |
| Every builtin, with signatures | [Builtins](Builtins.md) |
| Every CLI flag and JSON schema | [CLI Reference](CLI.md) |
| Runnable programs to imitate | [`examples/console/`](../examples/README.md) |
| Multi-file programs | [Module System](module-system.md) |
| Building an app or a game on Petal | [Building Apps](building-apps.md) |
| Why the language is shaped this way | [Goals](dev/goals.md) |

The single most useful next step is to read `examples/console/` end to end —
each file is a commented treatment of one feature, and they are all covered by
the test suite, so they are always current.
