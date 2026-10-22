# Inline state: the `state` keyword

Petal has a first-class way to declare persistent state, with the `state`
keyword. Variables declared with `state` are persisted across time.

This system is an enabler for a lot of Petal's features including [hot reloading](hot-reload.md).
Since the Petal runtime knows what state is essential to preserve, it's
able to preserve that state across a live code modification.

# Contents

- [Declaring state](#declaring-state)
- [State is nested](#state-is-nested)
- [Interaction with control flow](#interaction-with-control-flow)
  - [Keyed loops](#keyed-loops)
- [Deprecated: `state(key)`](#deprecated-statekey)
- [Sharing on purpose](#sharing-on-purpose)
- [Callbacks run at the builtin's callsite](#callbacks-run-at-the-builtins-callsite)
- [`state var`](#state-var)
- [Reading and writing from other functions](#reading-and-writing-from-other-functions)
- [How long a slot lives](#how-long-a-slot-lives)
- [State is not a constant](#state-is-not-a-constant)
- [Pending initializers](#pending-initializers)
- [State and hot reload](#state-and-hot-reload)
- [State from the host side](#state-from-the-host-side)
- [See also](#see-also)

## Declaring state

State vars are declared with an initializer.

In this example, the `ticks` number would start at 0 and increase by 1 every time this script is called:

```petal
state ticks = 0
ticks += 1
```

## State is nested

State vars can be declared in nested blocks and inside function calls.

One area where newcomers get confused is assuming that a nested state variable
is shared across the program like a global variable, but it doesn't work like a global. Petal state does
NOT work the same way as the `static` keyword in the C language.

Instead, state variables are created separately for each callsite.

For an example let's set up a `stable_random` function. Our function will
create a random number and store it.

```
fn stable_random() 
  state val = rand_int(100)
  return val
end

print(stable_random())
```

If we run this program, it will pick a random number (let's say `42`), and
then the same program will print `42` every time.

In the debugger we could inspect the persisted state for this program and
see something like:

TODO: example.

Then, let's say we have multiple calls to the same function:

```
fn stable_random() 
  state val = rand_int(100)
  return val
end

print("first: ", stable_random())
print("second: ", stable_random())
print("third: ", stable_random())
```

Each call will create a *different* random number that's stored separately.

The output might look like:

```
first: 39
second: 14
third: 77
```

In the debugger, the state might look like:

TODO: example.

## Interaction with control flow

If `state` is used inside a loop, then it creates a list, one element per iteration.
A loop keys each iteration by its position, like a list index: if the list
reorders, each value stays with its position rather than its item. To make a
value follow its item, give the loop a key ([Keyed loops](#keyed-loops)).

If `state` is used in an `if` block, then the data is only retained on runs where
that condition is true. If the condition fails then any nested state gets deleted.

If `state` is used in a recursive function call, then it creates a tree data structure.

### Keyed loops

`for x in xs key k do` keys each iteration by the value of `k` instead of by
its position. The state inside the body, including the state of every
function the body calls, then follows the item when the list is reordered,
when an item is inserted ahead of it, or when an item before it is removed:

```petal
fn row(task)
    state open = false     // follows task.id, not the row's position
    if open then task.title ++ " (open)" else task.title end
end

let tasks = [{id: 7, title: "write"}, {id: 3, title: "test"}]
for t in tasks key t.id do
    print(row(t))
end
```

The key is evaluated for each iteration, with the loop variable bound,
before the body runs. It is relative to the loop: it names an item among
the iterations of *this* loop on *this* call path. Two callers of a function
that runs a keyed loop still hold separate slots, so stateful code stays safe
to call twice. That is the difference from the deprecated `state(key)`,
which is global.

- **A key must be unique within one run of the loop.** Two items with an
  equal key would share every slot without any sign of it, so this is a
  runtime error (`Duplicate loop key 3: …`). The same key in the next run
  of the loop, or in another loop, is fine.
- **Keys compare like [Map](../language-guide.md#maps) keys**: by content,
  consistent with `==`. A function, a `var` cell, a pending value and NaN
  cannot be keys.
- **A removed item's state is dropped** at the end of the run, the way any
  slot a run does not reach is ([How long a slot lives](#how-long-a-slot-lives)).
  No cleanup code is needed.
- `key` works on `for` over a list, a map's keys or a `range`. A `while` loop
  has no item to key by and stays positional. `key` is a keyword only in that
  position, so it is still an ordinary name everywhere else.

A state dump shows a keyed slot under its key value: `[key=7]/row/open`.

## Deprecated: `state(key)`

`state(expr) name = …` keys the slot by the value of `expr` and ignores the
call path, so every caller that passes the same key lands on the same slot.
That makes it a global hidden inside a function, and calling the code that
declares it twice is no longer safe. It still works, but it is deprecated:
each declaration is a compile warning (`[state-key-deprecated]`). Use one of
these instead:

- **A pure cache**, a value computed from the key and nothing else:
  [`memo(key)`](memo.md). `petal lint --fix` makes this rewrite for a keyed
  `state` that nothing writes.
- **Identity that must survive a change of position** (an entity in a list
  that reorders): a [keyed loop](#keyed-loops), `for t in tasks key t.id`.
- **State shared on purpose**, or identity a loop key cannot express (one
  that survives a change of recursion depth, as in Garden's `plant.ptl`): a
  top-level `state var` holding a [Map](../language-guide.md#maps) keyed by
  the id. See [Sharing on purpose](#sharing-on-purpose).
- **Per-instance state** whose position is a fine identity: plain `state`.

The lint fix cannot tell whether an initializer is pure. A memo entry is
recomputed after a reload, so `state(id) tint = random(0.0, 1.0)` rewritten
as a memo would draw a new tint. Move such a slot to a Map instead. To keep
a declaration as it is without the warning, put
`// petal-lint-ignore state-key-deprecated` on the line before it.

## Sharing on purpose

Every caller is a different path, so a helper cannot hold a value that all of
its callers share. An accessor function with a single `state` inside it gives
each caller its own slot. State that is meant to be shared is a **top-level
`state var`**: module scope is one path, so there is exactly one cell, and
`get` and `set` keep every read and write visible at the site that does it.

```petal
state var theme = "dark"

fn ui_theme()      get theme end
fn theme_set(t)    set theme = t end

theme_set("light")
print(ui_theme())   // light
```

A Map in a top-level `state var` is shared state with one value per key, and
its entries follow an id rather than a position. Per-row state of a list is
better served by a [keyed loop](#keyed-loops); a Map is for state that is
meant to be shared, or whose identity no loop expresses:

```petal
state var hp = Map()

fn health(id, damage)
    set hp[id] = (get hp[id] ?? 100) - damage
    get hp[id]
end
print(health("goblin", 10), health("orc", 30), health("goblin", 5))  // 90 70 85
```

A map entry stays until it is removed. It is not dropped when a run does not
touch it, as a slot is ([How long a slot lives](#how-long-a-slot-lives)), so
remove an entry when its entity goes away: `set hp = remove(get hp, id)`.

A cache behind an accessor is the same mistake in a quieter form. `state` runs
its initializer only on a slot's first touch, so wrapping an expensive build
in a function reads like a memo. But the memo is per path, so a caller inside
a loop builds it again on every iteration and never gets a hit.

```petal ignore
fn char_table()
    state var t = build_table()   // once per callsite, once per iteration
    get t
end

state var char_table = build_table()   // once for the program
```

Hoist the declaration to the top level when the cache is global, or make it a
[`memo`](memo.md) when it varies by an argument (`memo(ink) rows =
build_rows(ink)` gives one build per ink, shared by every caller). If the
value is a function of the source alone, it is not state at all: see
[State is not a constant](#state-is-not-a-constant).

## Callbacks run at the builtin's callsite

`map`, `filter`, `reduce`, `forEach` and the other higher-order builtins call
your function from one place, the builtin's own callsite. A `state` inside
the callback therefore has one slot shared by every element of that call:

```petal
fn seen(x)
    state n = 0
    n += 1
    n
end
print(map([10, 20, 30], seen))   // [1, 2, 3] — one shared slot, not three
```

Use a `for` loop when you want a slot per element, and a
[keyed loop](#keyed-loops) when the slot belongs to the element's value rather
than its position. A higher-order builtin has no keyed form.

## `state var`

`state var` combines persistence with a [mutable cell](../syntax/var.md). The
slot holds the cell, so the value survives across runs and hot reloads, and it
can also be written with `set` from inside another function or a callback.

```petal
state var hits = 0
set hits = hits + 1
print(hits)
```

The slot is keyed like any other: a `state var` at the top level is one
shared cell, and a `state var` inside a function is one cell per call path.
For one cell's worth of state per key, hold a Map in a top-level `state var`.
Use `state var` only when a plain
`state` cannot express the write. A read of a plain `state` carries its
dataflow edges, and a read of a cell does not
([var.md, Provenance](../syntax/var.md#provenance)).

## Reading and writing from other functions

A plain `state` follows the rules of `let` at a function boundary.

**Writing.** `=` on a `state` declared outside the function is a compile
error, as it is for a `let`:

```text
Error: `total` is bound outside this function; this assignment creates a local
shadow and does not modify `total`. Use `let` for a new local, return the
value, or — if it really must be mutable — declare it `var total = ...` and
write it with `set total = ...`
```

A `state` that a function has to update is a `state var`, written with `set`.
`petal apply-change convert-to-var` makes that change to a binding and all of
its uses ([CLI reference](../CLI.md)).

**Reading.** A function captures a plain `state` by value where the function
is defined. If the top level writes the state after that point, the function
sees the value from before the write, and `petal check` warns:

```text
warning: `total` is a `state` binding written on line 5, after this function
was defined, so this reads the value captured at the definition — one run
behind the write. Pass it in as a parameter if you want the current value.
```

Pass the value as a parameter, or make it a `state var` and read it with
`get`, which reads the cell at the time of the call. The rule and its reasons
are in [var.md, Capture lag](../syntax/var.md#capture-lag).

## How long a slot lives

**A slot lasts as long as the host keeps it.** A frame-loop host (petal-sdl,
Garden, an embedding engine) keeps state from frame to frame for as long as
the program is loaded. `petal run` is one run in one process, so each
invocation starts from the initial values and warns that the script declares
state. `petal run --state-storage <file>` loads every slot from a JSON file
before the run and saves it after
([CLI reference](../CLI.md#state-between-runs)).

**A slot that a run does not touch is dropped when that run ends.** A
declaration behind an `if` that was not taken, a row whose list item was
removed (positional or [keyed](#keyed-loops)), a function that was not called
this run: none of them leak, and each
starts from its initializer if it is reached again later.

```petal
state frame = 0
frame += 1

fn hits()
    state n = 0
    n += 1
    n
end

// hits() runs on odd frames only. Its slot is dropped at the end of each
// even frame, so this prints 1 every time and never 2.
if frame % 2 == 1 then print(hits()) end
```

Entries in a Map held by a `state var` are not slots, so they are never
dropped this way. Remove them yourself when their entity goes away.

To keep a value through runs that do not use it, touch the slot on every run:
declare it at the top level, outside any condition.

## State is not a constant

A `state` that is never assigned is built once and then never updated,
including by a hot reload of the line that declares it. A table of tiles or
icons kept in `state` to avoid rebuilding it stops following edits to its own
source. Declare such a value with
[`const`](../language-guide.md#const--a-value-that-must-be-constant): it is
built once, kept between runs, and rebuilt when the source it depends on
changes ([Static values and `const`](static-values.md#const-let-or-state)).

The lint rule `state-as-constant` reports a plain top-level `state` with a
static initializer that nothing in the program assigns, and its fix rewrites
it as a `const`:

```text
$ petal lint --rules-include=state-as-constant app.ptl
app.ptl:12:1: state-as-constant: `state TILES` is never assigned and its initializer is constant; declare it with `const` so that it follows edits (keep `state` if a host writes it by name)
$ petal lint --fix --rules-include=state-as-constant app.ptl
```

A `pub state` qualifies like any other, because a module's exports are
read-only to the modules that import them. The rule runs only when asked for
by name, because the rewrite is a judgement and not a respelling. Keep `state`
where a host writes the slot by name, or where the value is meant to survive a
reload. `// petal-lint-ignore state-as-constant` on the line records that.

## Pending initializers

A [pending value](../dev/pending-values-plan.md) (the result of an
asynchronous fetch that has not arrived) is not committed by a `state`
initializer. The slot stays uninitialized, reads yield the pending value for
that run, and the initializer runs again on each run until it produces a
value that is not pending. So `state user = fetch(url)` means "initialize
this state from the fetch, once it arrives", and it does not store the
loading state forever. The rule is per slot.

## State and hot reload

Every part of a slot's identity is derived from names, never from positions
in the file. The declaration is identified by its name and its enclosing
module and function chain. Each callsite is identified by the callee's
spelling plus its ordinal among identically spelled calls in that function.

- Editing elsewhere in the file (reordering declarations, adding lines,
  reformatting) keeps state.
- Renaming a state variable, renaming a callee, or inserting an earlier call
  to the same callee changes an identity. That slot, or that callsite's
  subtree of slots, starts fresh.
- Editing a state's initializer does not change a slot that already exists.
  The reload report lists it as a stale seed when the initializer read a
  value that changed.

What a reload does to the rest of the program, and what it reports, is in
[hot-reload.md](hot-reload.md).

## State from the host side

A host can read and write top-level state by name, dump every slot as JSON,
and snapshot or restore the whole map. Slots under a call path have no name to
address them by; they appear in the JSON dump under their path
(`[3]/row/hovered`, `counter/n`).

- Rust: [ffi.md, Retained state](../usage/ffi.md#retained-state).
- C and C++: [embedding-c.md](../usage/embedding-c.md).

A function that the host calls directly starts a path of its own. Repeated
host calls of one function share slots with each other, but not with calls of
that function made inside the program. To share a value across both, use a
top-level `state var`.

## See also

- [`examples/console/state.ptl`](../../examples/console/state.ptl) and
  [`examples/console/reactive_ui.ptl`](../../examples/console/reactive_ui.ptl):
  runnable examples.
- [`var`: mutable cells](../syntax/var.md): `var`, `set` and `get`.
- [Cached results: `memo`](memo.md): a pure value cached by key, shared by
  every caller.
- [Call-path keyed `state`](../dev/state-call-paths.md): the design, the
  runtime key, and the measured cost.
