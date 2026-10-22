# Cached results: the `memo` keyword

`memo(key) name = init` binds `name` to the result of `init`, and computes
that result only once per key. The next time the declaration runs with an
equal key, in this run or a later one, it skips `init` and binds the value it
computed before.

```petal
fn slow_square(n)
    print("computing", n)
    n * n
end

fn square(n)
    memo(n) sq = slow_square(n)
    sq
end

print(square(3), square(4), square(3))
```

This prints `computing 3`, `computing 4`, and then `9 16 9`. The second
`square(3)` finds the entry the first one made.

A memo is for **pure** results: values that depend on the key and on nothing
else. It is a cache, and Petal treats it as one. It is not state.

# Contents

- [Declaring a memo](#declaring-a-memo)
- [One entry per key, whoever asks](#one-entry-per-key-whoever-asks)
- [Keys are compared by content](#keys-are-compared-by-content)
- [A memo cannot be reassigned](#a-memo-cannot-be-reassigned)
- [How long an entry lives](#how-long-an-entry-lives)
- [A memo is not state](#a-memo-is-not-state)
- [Memo and hot reload](#memo-and-hot-reload)
- [Pending initializers](#pending-initializers)
- [Choosing between `memo`, `const` and `state`](#choosing-between-memo-const-and-state)

## Declaring a memo

The key is written in parentheses after `memo` and is required. A type
annotation takes the same place it does on `let`:

```petal
fn layout(text)
    memo(text) width: int = len(text) * 8
    width
end
print(layout("hello"))   // 40
```

The key is evaluated every time the declaration runs. The initializer is
evaluated only on a miss. There is no `memo var` and no `pub memo`.

## One entry per key, whoever asks

An entry is identified by the declaration and the value of the key. The call
path that reached the declaration plays no part: two callsites, every
iteration of a loop, and every depth of a recursion that ask for the same key
all get the same entry.

This is the opposite of `state`, where each call path gets
[its own slot](inline-state.md#state-is-nested). For state that difference
matters, because a slot can be written, and sharing it between callers would
make them interfere. A memo cannot be written, and its initializer is a pure
function of the key, so it does not matter which caller computed the entry.
Every caller would have computed the same value.

Two declarations never share entries, even with equal keys. Each declaration
caches its own initializer.

## Keys are compared by content

A key can be any value. Lists and records are compared by what they contain,
not by where they were built, so two separately built `[1, {a: 2}]` keys
select the same entry. Values that are `==` are the same key, so `memo(2)` and
`memo(2.0)` share an entry, as they would for any other keyed lookup
([Comparison](../language-guide.md#comparison-and-logical-operators)).

The comparison is made on a 64-bit hash of the key's content. Two different
keys whose hashes collide would share an entry. With 64 bits this is
vanishingly unlikely, but a memo is not a substitute for a map where exact
lookup is the point.

## A memo cannot be reassigned

The name is bound like a `let`, and any write to it is a compile error:

```text
Error: `x` is a `memo`, which cannot be reassigned: it is a cached result, and
a later hit returns its initializer's value. Bind the new value to a new name
(`let`), or keep data the program writes in a top-level `state var`
```

This covers `x = …`, `x += …`, `set x = …` and writes into the value
(`x.field = …`, `x[i] = …`). A write would read as an update of the cache, but
nothing could honour it: the next hit returns what the initializer computed.
Shadowing the name with a new `let x` is allowed.

## How long an entry lives

A frame-loop host (petal-sdl, Garden, an embedding engine) keeps entries from
one run to the next, the same way it keeps state.

**An entry that a run does not touch is dropped when that run ends**, like a
[state slot](inline-state.md#how-long-a-slot-lives). A memo inside an `if`
that was not taken, or keyed by a row that has been removed, does not leak. If
the declaration is reached with that key again, it computes the value again.

A [memoized scope](../dev/memo-scopes.md) that is replayed instead of run
still keeps the entries its last run touched, so a cache inside a function
that the runtime skips is not lost.

## A memo is not state

Because an entry can always be recomputed, Petal does not treat it as data
the program owns:

- Host state dumps (`get_state_json`) and the names a host can set
  (`set_state`) do not include memo entries.
- `petal run --state-storage` does not save them, and the next process
  recomputes them.
- A state snapshot leaves them out, so restoring one drops them.
- The `state-as-constant` lint and the static classifier's state reports do
  not apply to a memo.

## Memo and hot reload

**A reload that changes the program drops every memo entry.** Whether an
entry is still right depends on everything its initializer calls, anywhere in
the program, and a reload cannot tell cheaply which of those an edit touched.
The next run recomputes what it asks for. `state` is kept as usual.

A reload that only moved code (whitespace, comments) changes nothing and
keeps the entries.

## Pending initializers

As with `state`, an initializer that produces a
[pending value](pending-values.md) is not committed. The entry stays empty,
reads see the pending value for that run, and the initializer runs again on
the next run until its value arrives.

## Choosing between `memo`, `const` and `state`

- A value that depends only on the source text: [`const`](static-values.md).
  It is built once and rebuilt when its source changes.
- A pure value that depends on a runtime value: `memo(key)`.
- A value the program changes over time, per call path: `state`.
- Data that several functions read and write: a top-level `state var`,
  written with `set` ([var.md](../syntax/var.md)).

## See also

- [Inline state](inline-state.md): `state`, its slots and their lifetime.
- [Call-path keyed `state`](../dev/state-call-paths.md): how slots are keyed
  at runtime. A memo entry is the absolute, keyed form of §2.2, under its own
  kind of key.
