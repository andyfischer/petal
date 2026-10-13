# Static values and `const`

A **static value** is a value that is a function of the source text alone: it
reads no `state`, no input, no clock and no host data, so every run of the
program builds the same thing. Petal builds such a value once, keeps it
between runs, and builds it again when an edit changes the source it depends
on. `const` is the declaration that says a value must be static.

```petal
fn tile(name, cost)
  {name: name, cost: cost}
end

const TILES = [tile("grass", 1), tile("road", 0), tile("water", 9)]
const COSTS = map(TILES, fn(t) -> t.cost)

state frame = 0
frame += 1
print(frame, COSTS)
```

Petal hosts run a script again and again: once per frame in a game or a UI,
once per edit while live coding. Run three times on one stack, the program
above calls `tile` three times in total, not nine: the first run builds
`TILES` and `COSTS`, and the next two read the kept values and jump over the
code that builds them.

This page is the reference for static values as a script author and a host
embedder see them. How the classifier, the cache and the reload are
implemented is in [internals/static-values.md](../internals/static-values.md).

- [What is static](#what-is-static)
- [`const`](#const)
- [Built once, kept between runs](#built-once-kept-between-runs)
- [Following edits on hot reload](#following-edits-on-hot-reload)
- [Values that cannot be kept](#values-that-cannot-be-kept)
- [Host natives: constant or not](#host-natives-constant-or-not)
- [Tools](#tools)
- [`const`, `let` or `state`](#const-let-or-state)

## What is static

A value is static when it is built only from:

- literals, and lists and records of static parts;
- operators on static operands;
- other static declarations: a `const`, a `let` that is itself static, a
  `config let`, an enum's constant data;
- calls of constant builtins (`len`, `map`, `sort`, `sqrt`, the string,
  collection, colour, vector and formatting builtins) with static arguments;
- calls of **static-pure** functions with static arguments. A function is
  static-pure when its body reads no `state`, no `var` but its own, and no
  dynamic native, prints nothing, and calls only static-pure functions.
  `tile` above is one.

Everything else is dynamic: `state` and `var` and anything that reads them,
`random()`, `print`, input, the clock, and every host native the host has not
declared constant. A few rules decide the cases that are less obvious:

| Rule | Example |
|---|---|
| A name that is assigned again is dynamic at every definition. | `xs[i] = v`, `xs = append(xs, v)`, a rebind inside a top-level `if` or loop. |
| A loop, a branch or a `match` is static only as a whole: its inputs and everything inside it. | `let D = if N > 2 then live else 0 end` is dynamic whichever way `N` points. |
| A value that would hold a function is dynamic. | `let HANDLERS = {open: open_file}` |
| A function argument is static only for `map`, `filter`, `reduce`, `sort` and `sort_by`, and only if the function is static-pure. | `apply(double, 2)` is dynamic; `map(XS, double)` is not. |
| A method call is static only if every method of that name is. | One `fn Square.area` that reads `state` makes every `.area()` dynamic. |
| One arity of an overloaded function that is not static-pure makes the whole name not static-pure. | |

The answer never depends on the value of a literal, only on the shape of the
program and the names in it. Editing a number cannot turn a static value
dynamic.

[`petal show-static`](#tools) prints the answer for every top-level
declaration of a file, with the reason for each dynamic one.

## `const`

`const NAME = expr` is a `let` whose value must be static. It is evaluated
exactly as the `let` would be. What it adds is a check at compile time: a
`const` that is not static is an error, with the chain of reasons down to the
term that makes it dynamic.

```petal
state clicks = 0

fn label(n)
  "clicked {n}"
end

const TITLE = label(clicks)
```

```text
error: `TITLE` is declared `const` but is not constant
  = TITLE reads clicks at line 7
  = clicks is state, which changes between runs at line 1
 --> [line 7, column 15]
  |
7 | const TITLE = label(clicks)
  |               ^^^^^^^^^^^^^
```

`petal check` exits non-zero on it and `petal run` does not start. A host
that reloads a script while it is being edited reports the error and keeps
running, with the binding behaving as the `let` it otherwise is.

A plain `let` that happens to be static is kept between runs in the same way.
But it silently stops being kept the day someone adds a `print` to a
constructor or reads the screen size in a helper. `const` turns that into an
error at the declaration instead of a slowdown nobody notices.

The rules:

- **Top level only.** A `const` inside a function, a loop or a branch is an
  error: only a top-level declaration is one value for the life of the
  program. Use `let` there.

  ```text
  error: `const X` must be declared at the top level of a file, not inside a function, loop or block; use `let` here
  ```

- **Never assigned.** `A = …`, `A[i] = …` and `A.f = …` on a `const` are the
  same error as above, with the reason `A is assigned again (a value that is
  mutated is never kept)`.
- **Data, not a function.** `const f = fn(x) -> x` is an error (``is
  declared `const` but is a function, not a constant value; declare it with
  `fn` or `let` ``), and so is a list or record that holds a function.
- **`pub const`** exports it like any binding.
- **`config const` does not exist.** A `config let` of literals is already
  static, and `config` is about who edits the value, not about what it
  depends on.
- **Hoisting** is as for `let`: a `const` whose initializer is literals and
  arithmetic is in place before the file's first statement, so a hoisted
  function that reads it can be called from above
  ([Declaration order](../language-guide.md#declaration-order-top-level-fns-are-hoisted)).
- `const` is a keyword, so it cannot name a binding. It can still be a record
  key or a field (`{const: 1}`, `r.const`), like every keyword.
- **Build it without `append`.** A value that may be written in place is
  never kept between runs, and `append` may write into the list it is
  handed. So in `const ALL = append(for id in IDS do … end, EXTRA)` the
  comprehension runs on every frame although `ALL` is constant, and the
  compile warns:

  ```
  warning: `ALL` is declared `const`, but part of what builds it is built
  again on every run: the value written on line 3 is handed to `append`, …
  ```

  `concat(for … end, [EXTRA])` and `(for … end) ++ [EXTRA]` build a new list
  and are kept whole. The same warning names a `const` that is itself handed
  to `append` at top level (`const U = append(T, x)` warns about `T`).

A `const` built from a host's natives is checked by the host, not by `petal
check`: see [Host natives](#host-natives-constant-or-not).

## Built once, kept between runs

A host that runs the script every frame builds each static value on the first
run after a load and keeps it on the stack. Later runs jump over the code
that builds it. Nothing changes in what the program does: it prints, returns,
raises and stores what it would with every value rebuilt on every run.

What is kept:

- **Top-level values** that code running on every run reads: a `const`, a
  static `let`, a static table written inside a call, an enum's constant
  data.
- **Values built inside function bodies** that read nothing of the call: the
  `{kind: "none", ml: 0}` a scanner returns for "nothing here", a colour
  literal built once per cell of a heat map. The first call that reaches the
  code builds the value and every later call loads it. A value that reads a
  parameter, a loop variable, a `state` or a `var` is a value of the call and
  is not kept.

What is not, although it is static:

- **A value that is written to.** `let by_sec = [0, 0, 0]` followed by
  `by_sec[i] = v` is rebuilt on every run, as is a list handed to `append`
  under another name. `show-static` marks the second case `! not kept`.
- **A value nothing reads.** An unused `let BAD = XS[7]` still raises at its
  line, on every run. A top-level `assert` runs on every run too.
- **Numbers and other scalars standing alone.** Loading one costs what the
  check would.

**A kept value cannot be changed.** A kept table handed to a function that
writes to its parameter, or edited through an alias, is copied by that write.
The kept value stays as it was built.

```petal
fn grow(xs)
  xs[0] = 99
  xs
end

const BASE = [1, 2, 3]
print(BASE, grow(BASE))   // [1, 2, 3] [99, 2, 3]
```

**The one thing a program can tell** is `frozen`, which answers about the
heap object and not the value. A kept table passed to `freeze` is frozen once
and is still frozen at the top of the next run. Rebuilt on every run, it was
a new unfrozen table each time. The same holds for a value kept inside a
function: after `freeze(mk())`, `frozen(mk())` is true.

**Where it is on.** Values are kept under the `fast` and `replay`
[run policies](../internals/run-policies.md), which is what a frame-loop
host runs (petal-sdl, Garden, an embedding engine). They are not kept under
`baseline` and `explain`, or while a host has the per-term trace on. A plain
`petal run` runs the script once and keeps nothing, since there is no second
run to keep a value for. `petal run --runs <n>` runs it `n` times on one
stack the way a host does, with values kept:

```text
$ petal run --runs 3 tiles.ptl
1 [1, 0, 9]
2 [1, 0, 9]
3 [1, 0, 9]
```

`--policy fast-static` turns the cache off, and `--policy fast-fnstatic`
turns off only the values kept inside function bodies. A host does the same
with `Env::set_static_cache(false)` (`pb_vm_set_static_cache` from C). Output
is the same either way, so a difference between the two is a bug in Petal.
What it saves is measured in
[dev/performance.md](../dev/performance.md#static-values).

## Following edits on hot reload

A static value follows its source. A [hot reload](hot-reload.md) keeps the
values the edit cannot have changed, and the next run rebuilds the rest. This
is the difference from a table parked in `state`, which is built once and
never again.

A value is **rebuilt** when the edit touched:

- the statement that declares it;
- anything it depends on: a table it is built from, a function it calls
  (a literal edited inside a function body changes the function), every
  method of a name it calls, an enum whose data it reads;
- what one of the names it uses means. A new import, a declaration that
  shadows a builtin or a module's export, an added overload and an added
  method all rebuild the values that could now resolve differently.

Everything else is **kept**. Editing a draw function, a comment, or one table
out of forty rebuilds at most what reads the edited thing. `petal show-static
--json` lists each declaration's dependencies under `depends_on`.

A rebuilt value that comes out the same as it was keeps the old heap value.
An edit to a helper function that leaves a table unchanged costs the rebuild
and changes nothing else.

Every value is rebuilt, as if nothing were followed, when the diff cannot say
what changed: top-level statements changed order, the set of source files
changed (an import added or removed), a source file could not be read, or the
new text is an edit the diff does not classify.

Three limits:

- Values kept inside function bodies are not followed. Any reload that
  changes a value or recompiles empties all of them, and each is built again
  by the next call that reaches it. A reload that only moves text (a comment,
  whitespace) leaves them.
- Only the stack the reload was asked of keeps anything. Every other stack of
  the program, and a fork, rebuilds its static values on its next run.
- A reload does not change which objects are shared. A rebuilt value holds
  the kept values it is built from themselves (`let C = [B, 1]`, with `B`
  kept and `C` rebuilt, holds the kept `B`, so `freeze(B)` is seen through
  `C[0]`), and a value is kept only together with the values it was built
  from: when `B` is rebuilt, `C` is too, whatever the edit was. A table
  frozen before the reload stays frozen when it is kept, and is a new,
  unfrozen one when it is rebuilt.

The unit of rebuilding is the value. Each value that something outside its
own statement reads is built and stored on its own, so a rebuilt table leaves
the tables written around it kept. What is rebuilt together is what cannot be
told apart: a table that only one other value reads and that value, and the
values one statement binds.

### The reload report

A host prints what happened after each reload. The SDL host and Garden print:

```text
[hot-reload] constants: 2 rebuilt (TILES, WIDTHS), 41 kept
[hot-reload] stale seed: state grid was initialized from TILES, which changed; it keeps its value
```

Nothing is printed for a reload that reached no constant and left no stale
seed. When everything was rebuilt the line ends with the reason (`(all of
them: …)`).

A **stale seed** is a top-level `state` that kept its value although its
initializer reads something the edit changed, or was itself edited. That is
how `state` has always behaved: an initializer runs once. The line makes it
visible when a table reloads and a `state` built from it does not. If the
`state` should have followed, it is a constant: see
[`const`, `let` or `state`](#const-let-or-state).

From Rust the report is `ReloadReport::statics` (counts, the rebuilt names,
the stale seeds) and `ReloadReport::static_lines()` gives the lines above.
From C it is under `statics` in `pb_vm_last_reload_json`
([embedding-c.md](../usage/embedding-c.md)). The report is absent when the
program does not run with values kept (the `baseline` and `explain`
policies, or a host with the per-term trace on, such as a Garden panel being
traced by the Petal IDE) or has not run yet.

`Env::set_static_follow(false)` (`pb_vm_set_static_follow`) makes every
reload rebuild every static value.

### Dragging a value

A host that sets a literal continuously (a slider drag, through
`Env::set_config_value`) rebuilds the static values built from it on each
tick. A value that is slow to build waits until the drag pauses; the dragged
value itself never waits. The times, the report line and the host calls are
in [hot-reload.md](hot-reload.md#a-slow-table-under-a-drag).

## Values that cannot be kept

A value that holds a function, a `var` cell, a host handle or a pending
result is never kept, however it was built. The code that builds it runs on
every run, which is correct and slow, and the host is told once per load (and
once more after a reload):

```text
warning: TABLE (app.ptl:12) holds a pending result, so it is rebuilt on every run
warning: const TABLE (app.ptl:12) holds a pending result, which cannot be kept between runs, so it is rebuilt on every run
warning: a constant in lookup (app.ptl:30) holds a host handle, so it is rebuilt on every call
```

The second form is for a `const`, whose author asked for a constant. The
third is for a value inside a function body.

In practice this means a host native that was declared constant and returns
a handle or a pending result. A script cannot get there by itself: a value
that holds a function or reads a `var` is already dynamic, and a `const` of
one is a compile error.

`petal run` and the SDL host print the warnings. An embedding host collects
them with `Env::take_static_warnings(stack)` after a run, or reads them from
`pb_vm_static_values_json` in C; the cache prints nothing by itself.
`Env::static_values(stack)` lists every slot and what it holds.

## Host natives: constant or not

A native function is dynamic unless its host registered it as a **constant
function**: the same arguments give the same result for as long as the
program lives, whatever the host is doing. Only then is a call of it with
static arguments static.

```rust
// A lookup into a table compiled into the host: constant.
env.register_native("tile_code", native_tile_code, NativeEffects::CONST);
// Pure for the memo, but the host's fonts decide the answer: not constant.
env.register_native("font", native_font, NativeEffects::PURE);
```

`CONST` is opt-in and separate from `PURE`. A native that answers from
anything the host loads, configures or simulates (fonts, themes, assets, a
seed, the clock, a handle) is not constant. A wrong `CONST` is a stale value
that stays until an edit reaches the declaration that holds it; no frame
recomputes it. `Env::clear_static_values(stack)`
(`pb_vm_clear_static_values`) drops everything a stack holds, for a host that
changed something such a native answers from. The checklist for deciding and
the API are in [ffi.md, Constant natives](../usage/ffi.md#constant-natives);
from C the flag is `PB_FX_CONST`
([embedding-c.md](../usage/embedding-c.md)).

**A `const` built from host natives is checked by the host.** `petal check`,
`petal lint` and the language server run without any host, so they do not
know which of its natives are constant. For a `const` whose only obstacle is
such a native they report nothing, and `show-static` says `declared const:
cannot be verified without its host's natives`. The host reports the error
when it loads the script, if the value is not constant there. A `const` that
is dynamic for a reason no host could change (it reads `state`, say) is an
error everywhere.

A host should therefore print the compile's diagnostics after a load and
after a reload: `Env::error_diagnostics_text(pid)` in Rust (or
`diagnostics_text`, which includes the warnings), `pb_vm_diagnostics_text`
from C. The program still loads and runs, with that
binding built on every run as a `let` is.

## Tools

**`petal show-static file.ptl`** lists every top-level declaration as static,
a function (static-pure or not) or dynamic, says who reads each static one,
and gives the chain of reasons for each dynamic one. It compiles the program
and never runs it. A `state` whose initializer is static says so. `--json`
adds each declaration's dependencies.
([CLI.md](../CLI.md#show-static--static-declarations))

```text
$ petal show-static tiles.ptl
function  fn tile      line 1    static-pure
static    const TILES  line 5    read only by static code (COSTS)
static    const COSTS  line 6    read by dynamic code (line 10)
dynamic   state frame  line 8    its initializer is static
```

**`petal run --static-audit file.ptl`** holds that answer against a run. While
each static declaration is being evaluated it watches everything a static
value may not depend on (host bindings, host data, output, effects, the
random stream, `state`, a native not registered constant), reports each one
touched, and exits with status 1 if there was one. A finding is nearly always
a native registered `CONST` that is not one. `petal-ui-run` takes the same
flag for a UI script.
([CLI.md](../CLI.md#checking-the-answer---static-audit))

**`petal run --static-only file.ptl`** evaluates only the static top-level
statements and skips the rest, under the same audit. Static code calls only
constant natives, so this builds the constant tables of a game or a panel
from the plain CLI, without its host. With `--observe` it prints the values
it built.

**`PETAL_STATIC_CHECK=1`** in the environment makes every `Env` of the
process check the values kept inside function bodies instead of trusting
them: the code runs on every call and what it builds is compared with what is
kept. A difference prints `static check failure: …` on stderr and is a bug in
Petal. A value that differs only because the program froze it prints `static
check note: …`. `Env::set_static_check(true)` does the same for one `Env`.

**`petal lint --rules-include=state-as-constant`** reports a plain top-level
`state` that nothing assigns and whose initializer is static, and `--fix`
rewrites it as a `const`
([inline-state.md](inline-state.md#state-is-not-a-constant)).

**The language server** shows the classification on hover (`static, kept
between runs`, `dynamic:` with the first reason, `function, static-pure`) and
reports a `const` that is not constant as an error.

**The reload report** names the constants a reload rebuilt and the stale
seeds it left ([above](#the-reload-report)).

## `const`, `let` or `state`

| Declare it | When | Built | After an edit to its source |
|---|---|---|---|
| `const` | The value is a table, a palette, a lookup, a set of rules: something that depends only on what is written in the file, and should stay that way. | Once, then kept. | Rebuilt. |
| `let` | Anything else that is computed. A `let` that happens to be static is kept like a `const`, with no error if it stops being static. | Every run, unless it is static. | Rebuilt if it was kept; otherwise it is built every run anyway. |
| `state` | The value changes while the program runs and must be remembered: a score, a cursor, a hover timer. | Once, then it holds whatever was last assigned. | Not rebuilt: an existing slot keeps its value. |

Use `const` for a table you would not want to become slow by accident. Use
`let` for small values and for anything that depends on the frame. Use
[`state`](inline-state.md) only for what the program itself changes.

A `state` that is never assigned is a constant in disguise. It is built once
and then stops following edits to its own source, and each reload reports it
as a stale seed. Declare it `const`; the `state-as-constant` lint finds these
and rewrites them. Keep `state` where a host writes the slot by name, or
where the value is meant to survive a reload.

A value that depends on the screen size, the theme or text measurement is
not static, because the host can change those. Compute it in a `let`, or put
it behind a function the memo can replay.

## See also

- [internals/static-values.md](../internals/static-values.md): how the
  classifier, the cache and the reload are implemented, and how they are
  tested.
- [hot-reload.md](hot-reload.md): what a reload does to the rest of the
  program.
- [inline-state.md](inline-state.md): the `state` keyword.
- [ffi.md, Constant natives](../usage/ffi.md#constant-natives): declaring a
  native constant.
- [dev/performance.md](../dev/performance.md#static-values): what keeping
  values saves, measured.
- [The static-values proposal](../proposals/static-values.md): the design and
  the alternatives that were considered.
