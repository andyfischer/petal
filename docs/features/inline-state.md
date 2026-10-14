# Inline state: the `state` keyword

`state` declares a variable that persists. It is initialized once, and after
that it keeps its value from one run of the program to the next and across a
[hot reload](hot-reload.md).

```petal
state hits = 0
hits += 1
print(hits)   // 1 on the first run, 2 on the second, ...
```

Petal hosts run a script again and again: once per frame in a game or a UI,
once per edit while live coding. `state` is how such a script remembers
anything. The declaration sits inline, in the code that uses the value, so
there is no separate store to set up and no handle to pass around: a widget
function that needs a hover timer declares one where it needs it.

This page is the reference for `state` as a script author sees it. The design
decisions behind the keying rules are in
[dev/state-call-paths.md](../dev/state-call-paths.md).

- [Declaring state](#declaring-state)
- [One slot per call path](#one-slot-per-call-path)
- [Keyed state: `state(key)`](#keyed-state-statekey)
- [Sharing on purpose](#sharing-on-purpose)
- [Callbacks run at the builtin's callsite](#callbacks-run-at-the-builtins-callsite)
- [`state var`](#state-var)
- [Reading and writing from other functions](#reading-and-writing-from-other-functions)
- [How long a slot lives](#how-long-a-slot-lives)
- [State is not a constant](#state-is-not-a-constant)
- [Pending initializers](#pending-initializers)
- [State and hot reload](#state-and-hot-reload)
- [State from the host side](#state-from-the-host-side)

## Declaring state

```petal
state score = 0
state score_typed: int = 0       // an optional type annotation, as on `let`
state(id) hp = 100               // keyed by a value (see below)
state var theme = "dark"         // a persistent mutable cell (see below)
pub state level = 1              // exported from a module
```

The initializer runs only when the slot does not exist yet. On every later run
the declaration reads the kept value and the initializer is skipped, so an
expensive initializer costs once:

```petal
fn build()
    print("building")
    [1, 2, 3]
end

state table = build()   // prints "building" on the first run only
```

After the declaration, a `state` name is read and assigned like a
[`let`](../language-guide.md#variables): `score += 10` rebinds it, and the
value it holds when the run ends is the value the next run starts with.

## One slot per call path

Which slot a declaration reads is decided by the declaration and by the path
that reached it: the chain of callsites and loop iterations running from the
top of the program down to this execution. Reached the same way, it is the
same slot. Reached a different way, it is a different slot. This is the rule
of React's `useState`, with the call path standing in for the component
instance.

What follows from it:

- **Two callsites of a helper hold two independent values.**
- **Recursion gets one slot per depth**, because each recursive call extends
  the path.
- **A declaration inside a `for`, or inside a function called from a `for`,
  gets one slot per iteration**, so a list's positions key its rows.
- **Two functions that declare the same state name never collide.** A
  declaration's identity is its name plus the enclosing module and function
  chain, so `count` in `fn a` and `count` in `fn b` are different
  declarations.
- **A top-level `state` is one slot.** Module scope runs on the empty path.

```petal
fn counter()
    state n = 0
    n += 1
    n
end

fn left()   counter() end
fn right()  counter() end

print(left(), right())   // 1 1 — two callsites, two slots
```

Within one run a path is normally reached only once: reaching the same
callsite twice takes a loop or a recursive call, and both of those extend the
path. So a counter inside a function counts runs (frames), not calls. The
program above prints `1 1`, then `2 2` on its second run. To count calls
within a run, key the slot explicitly or share one top-level cell (both
below).

```petal
fn walk(n)
    state visits = 0
    visits += 1
    if n > 0 then walk(n - 1) end
    visits
end
print(walk(3))   // 1 — depths 3, 2, 1, 0 are four slots, each on its first visit
```

```petal
fn row(label)
    state clicks = 0
    clicks += 1
    "{label}: {clicks}"
end

for label in ["a", "b", "c"] do
    print(row(label))   // a: 1  b: 1  c: 1, then a: 2  b: 2  c: 2 on the next run
end
```

Keying by position is how React treats a list without keys, and it has the
same cost: reorder the list and the values stay with the positions, not with
the items. When the item's identity is what matters, give the slot a key.

## Keyed state: `state(key)`

`state(expr) name = …` keys the slot by the value of `expr` and ignores the
call path. Every execution of that declaration with the same key value lands
on the same slot, whichever caller, depth or loop iteration it arrived from.

```petal
fn health(id, damage)
    state(id) hp = 100
    hp -= damage
    hp
end
print(health("goblin", 10), health("orc", 30), health("goblin", 5))  // 90 70 85
```

Use `state(key)` for anything whose identity outlives its position: entities
in a list that reorders, a tree node reached one level deeper each frame, an
input's repeat phase that two widgets are meant to share. There is one slot
per pair of declaration and key value.

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

Hoist the declaration to the top level when the cache is global, or key it
with `state(k)` when it varies by an argument (`state(ink) var rows =
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

Use a `for` loop when you want a slot per element, or key it:
`state(x) n = 0`.

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
shared cell, a `state var` inside a function is one cell per call path, and
`state(key) var` is one cell per key. Use `state var` only when a plain
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
removed, a `state(id)` for an entity that no longer exists: none of them leak,
and each starts from its initializer if it is reached again later.

```petal
state frame = 0
frame += 1

fn health(id, damage)
    state(id) hp = 100
    hp -= damage
    hp
end

// The goblin is hit on odd frames only. Its slot is dropped at the end of
// each even frame, so this prints 90 every time and never 80.
if frame % 2 == 1 then print(health("goblin", 10)) end
```

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
- [Call-path keyed `state`](../dev/state-call-paths.md): the design, the
  runtime key, and the measured cost.
