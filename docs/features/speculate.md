# `speculate`: a hypothetical call

`speculate(f, ...args)` calls `f(...args)` *as if*. The call runs on the live
program, against its real state and data, and when it returns everything it
changed is put back. What it computed comes back as a value you can look at:

```petal
fn view(cards)
  for i in range(0, len(cards)) do
    speculate_place(cards[i], {x: 40 + i * 70})
  end
  len(cards)
end

let r = speculate(view, ["B", "A", "C"])
print(r.value)                 // 3
print(r.places[1].key)         // A
print(r.places[1].value.x)     // 110
```

It is what a drag engine needs to ask "where would this card be if the board
looked like that?" without the answer leaving a trace: run the view on a
candidate board, read where the card landed, and drop everything else
([declarative dragging](../proposals/declarative-dragging.md#p1-speculate-a-hypothetical-call)).
`ui_measure` in petal-ui is built on it, and so is anything that wants to
know what a piece of code *would* do: a hover preview, a "what will this
button change" hint, a size measured before drawing.

- [What it returns](#what-it-returns)
- [What is put back](#what-is-put-back)
- [Natives: what runs and what does not](#natives-what-runs-and-what-does-not)
- [`state` inside the call](#state-inside-the-call)
- [Places: recording keyed values](#places-recording-keyed-values)
- [Rules](#rules)
- [How it works](#how-it-works)
- [Performance](#performance)

## What it returns

A record `{value, emits, places}`:

| Field | What it is |
|---|---|
| `value` | what `f(...args)` returned |
| `emits` | `[{channel, value, at}]`: everything the call pushed into an output buffer (draw commands, hit-test registrations, `push_output`), in emit order per channel, channels in name order. `channel` is the buffer's name; `at` is `{file, line, column}` of the call that emitted (the first frame outside the prelude, so a `draw_rect` through a widget points at the widget call you wrote), or nil |
| `places` | `[{key, value}]`: what [`speculate_place`](#places-recording-keyed-values) recorded during the call, in order |

```petal
import io

fn draw(n)
  for i in range(0, n) do push_output(symbol("cmds"), i * 10) end
  n
end

let r = speculate(draw, 2)
print(len(r.emits))            // 2
print(r.emits[1].channel)      // cmds
print(r.emits[1].value)        // 10
```

Nothing the call emitted reaches the host: the buffers hold only what the
live program pushed before and after it.

## What is put back

Everything the call changed that outlives it:

- **`state` slots.** A slot the call writes holds its old value again when the
  call returns; a slot first created inside the call is removed.
- **Cells.** A `set` on a `var` or `state var` the call did not create is
  undone, including `set xs[i] = v` and `set rec.field = v`.
- **Random draws.** The call draws from a copy of the random stream, so the
  live program's next `random()` is what it would have been without the call
  (and is the same number the call's own first draw was).
- **The noise seed, counters and `print` output.** A `noise_seed` inside the
  call is undone; `print` inside the call prints nothing.
- **What it emitted**, which is returned in `emits` instead.

```petal
var draws = 0

fn view(n)
  set draws += 1
  random() * n
end

let a = random()
let r = speculate(view, 10)
print(get draws)               // 0
print(r.value == random() * 10)   // true
```

What is not put back: anything a host native did (see below; such a native is
not called at all), expectations (`expect` inside the call is recorded like
any other), and resources a call started loading (a `fetch` inside the call
is a request the live program would make too, so the resource is left to load).

## Natives: what runs and what does not

A native's effect row ([native-effects.md](../dev/native-effects.md)) decides,
with the same rules as direct manipulation's speculative runs:

| Row | Inside `speculate` |
|---|---|
| pure, probe, read (`sqrt`, `mouse_x`, `hovered`, `binding`) | runs; reads the live input |
| emits (`draw_rect`, `ui_register`, `push_output`) | runs; what it pushes is captured into `emits` |
| contained effect (`print`, `noise_seed`, a counter) | runs; undone when the call returns |
| escaping effect or command (a host's `save_file`, `sfx`) | **not called**; answers `nil` |
| a method on a host handle | **not called**; answers `nil` |

A native that is not called is reported once per call site, as a warning the
host collects (`Env::take_runtime_warnings`; `petal run` prints it):

```text
warning: speculate(): `save_file` was not called (its effect escapes the run)
  --> board.ptl:58:3
```

## `state` inside the call

The call's `state` is keyed by call path like any other, and the path runs
through the `speculate` call's own site. So a widget called inside the call
has slots of its own, separate from the same widget's slots in the live view,
and those slots exist only while the call runs: **a slot first reached inside
the call starts from its initializer** every time. A slot the call reaches by
the same path as the live program (a top-level `state` it reads, a `state
var` cell, a `memo(key)` entry) is the live slot, read as it is, and written
only for the length of the call.

```petal
fn counter()
  state n = 0
  n += 1
  n
end

print(speculate(counter).value)   // 1
print(speculate(counter).value)   // 1
```

`state` is also allowed in a call made from a `map` (or another higher-order
builtin's) callback, where it is otherwise an error: the rule exists because a
callback has no place to keep a slot from one run to the next, and nothing a
speculative call keeps outlives it.

## Places: recording keyed values

Two natives let a library code differently inside and outside a speculative
call:

- `speculating()` is true while a speculative call runs, false otherwise.
- `speculate_place(key, value)` appends `{key, value}` to the running call's
  `places` and returns `value`; outside a call it only returns `value`. The
  key must be one a `for … key` loop would accept (no functions, cells,
  pending values or NaN).

A drag library's `place(key, rect)` is one line on top of them: inside a
speculation it records where the element is, and outside it returns the
animated rect it displays.

```petal ignore
pub fn place(key, r)
  if speculating() then speculate_place(key, r) else _animate(key, r) end
end
```

## Rules

- **Nesting is an error.** `speculate` inside a speculative call fails with
  `speculate() cannot call itself: a speculative call is already running`
  and the help `compute the candidate scenes first, then compare them
  outside`. Compute each candidate with its own top-level call.
- **Memo.** No memo scope records inside the call, and a scope around it is
  never replayed (nothing the call read was recorded, so a replay could not
  know it was stale). Memo records made outside are not replayed inside
  either: records are keyed by call path, and every path inside the call
  passes through the `speculate` call's site, which no outside record has.
- **Errors.** If `f` fails, everything is put back and the error propagates.
- **Observation.** The values named terms take inside the call are not
  observed: the "last value" a tool shows for a name is the live program's.

## How it works

`speculate` is a write log over the live VM, not a copy of it
(`core/src/backend/bytecode/vm/speculate.rs`). The first write the call makes
to a state slot or to a cell older than the call logs the old contents; on
return the log is played back. Reads see the live slot until the call writes
it. Values themselves are immutable, so the only other thing the call could
change under the live program is a container the optimizer edits in place.
The heap stamps each object with the speculative call it was allocated in,
and while a call runs an in-place edit of an older object copies it instead:
the call edits its own lists in place, at full speed, and never the live
program's. Natives go through the gate above, emits are cut off the end of
each buffer, and the random stream, noise seed, counters and print buffer are
saved and restored around the call.

Because nothing is copied up front, a call costs what the code it runs costs,
plus its writes. `fork_execution` (time travel, direct manipulation) copies
the whole heap instead, 0.9-30 ms per run.

## Performance

`test/benchmarks/micro/speculate.ptl`: a ten-card view that reads state,
keeps a slot, writes a cell and records ten places, called 200 times plainly
and 200 times through `speculate`. Release build, `petal
bench --fn plain --fn spec`:

| | instructions/call | ms/call (median) | allocations/call |
|---|---|---|---|
| `view(cards)` | 137 | 0.0017 | 10 |
| `speculate(view, cards).value` | 138 | 0.0034 | 23 |

About 1.7 µs per call on top of the view: entering and leaving (saving the
random stream, the counters and each buffer's length), logging two writes,
and building the result record with its ten places. The instruction count is
the same: the overlay costs nothing per instruction, only per write. With the
optimizer off the figures are 0.0018 and 0.0036 ms.
