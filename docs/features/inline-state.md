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

### Loops

If `state` is used inside a loop, then it creates a list or map, with one element per iteration.

By default it creates a list, with each iteration of the loop stored using its iteration index.

You can also use the `key` syntax to specify a stable key to store state.

Example:

```petal
let tasks = [{id: 7, title: "write"}, {id: 3, title: "test"}]
for t in tasks key t.id do
    print(row(t))
end
```

Specifying a key will make the contents much more stable, in
case the incoming list has ordering changes, or if you
skip elements with `continue`.

### Conditionals

If `state` is used in an `if` block, then the data is only retained on runs where
that condition is true. If the condition fails then any nested state gets deleted.

### Recursive calls

If `state` is used in a recursive function call, then it creates a tree data structure.

TODO: Example here

## Sharing on purpose

By nature, `state` containers are hard to share because they're created
separately for each call path.

If you **want** to share the same stateful value across multiple parts of the
program then the "escape hatch" approach is to use `state var` at the top
level. These values can be accessed and modified anywhere using `get` and `set`.

The `var` approach is discouraged when not needed since it makes your code
harder to reason about. But in some cases, it's necesary.

Example:

```petal
state var theme = "dark"

fn ui_theme()      get theme end
fn theme_set(t)    set theme = t end

theme_set("light")
print(ui_theme())   // light
```

One useful approach is to store a Map in a state var:

```petal
state var hp = Map()

fn health(id, damage)
    set hp[id] = (get hp[id] ?? 100) - damage
    get hp[id]
end
print(health("goblin", 10), health("orc", 30), health("goblin", 5))  // 90 70 85
```

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
