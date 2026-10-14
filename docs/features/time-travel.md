# Time travel: fork, rewind, replay

A running Petal program can be copied whole, and a copy can be put back. A
host that takes a copy after every frame has a history it can step through,
return to, and run again through edited code.

`petal-sdl` does this for every script, with nothing in the script to opt in:

```petal ignore
import ui

config let SPEED = 2

state x = 0
x += SPEED

clear(20, 20, 30)
draw_rect(x, 100, 20, 20, 255, 200, 50)
```

Run it, press `F5` to freeze, hold `,` to scrub back through the last ten
seconds, point at the square and press `F7` to see the path it has taken.
Change `SPEED` in the editor and save: the frames from the cursor onward are
run again through the new code, and the path ahead of the cursor moves.
Press `F5` again and the program continues from the frame on screen.

This page covers the runtime operations that make that possible, the
`petal-sdl` timeline built on them, and what each of them does and does not
capture.

- [What the runtime provides](#what-the-runtime-provides)
- [Why a copy is exact](#why-a-copy-is-exact)
- [The petal-sdl timeline](#the-petal-sdl-timeline)
- [Replaying through an edit](#replaying-through-an-edit)
- [Trails](#trails)
- [Reproducible runs](#reproducible-runs)
- [What is captured and what is not](#what-is-captured-and-what-is-not)
- [What it costs](#what-it-costs)
- [Other hosts](#other-hosts)

## What the runtime provides

An execution is a stack (the `state` slots and frames of one running program)
bound to a context (the heap, closures, host bindings, counters, random
number state and pending resources). These `Env` methods copy, restore and
compare executions:

| Method | What it does |
|---|---|
| `fork_execution(src)` | Returns a new stack with its own context: a deep copy of the heap and registries and a copy of the stack. The fork and its source share no mutable state, so either can run without affecting the other. The fork's output buffers start empty. |
| `restore_execution(dst, src)` | The inverse. `dst` takes a fresh copy of `src`'s context and state, under `dst`'s own keys, so the stack id the host holds stays valid. `src` is untouched and can be restored from again. |
| `drop_fork(stack)` | Releases a fork and its context. |
| `run_speculative(stack)` | Forks, runs the fork once with `print` echo off, drops it, and returns the result. The source is not touched and the fork's output is discarded. |
| `diff_state(program, a, b)` | Compares the `state` of two executions by value and returns the names that differ, with each side's JSON value. |
| `snapshot_state(stack)` / `restore_state(stack, map)` | Copy and replace the `state` map alone. |

A rewind is a fork taken earlier and restored later:

```rust
let pid = env.load_program("state var n = 0\nset n = n + 1\n")?;
let live = env.create_stack(pid)?;
for _ in 0..3 {
    env.reset_stack(live)?;
    env.run(live)?;
}
let checkpoint = env.fork_execution(live)?;      // n = 3

for _ in 0..2 {
    env.reset_stack(live)?;
    env.run(live)?;
}                                                // n = 5

env.restore_execution(live, checkpoint)?;        // n = 3 again
env.reset_stack(live)?;
env.run(live)?;                                  // n = 4, from the checkpoint
env.drop_fork(checkpoint);
```

A fork can also be run forward on its own, to see what a different input or
a different value would produce. Feed it with the per-stack accessors
(`set_binding_for(fork, sym, value)`), read its output with
`take_output_buffer_for(fork, sym)` and `take_output_for(fork)`, and compare
its state with `diff_state(pid, live, fork)`. `run_speculative` is the short
form for when only the returned value matters. A host that needs what the
fork drew or printed drives the fork by hand, because `run_speculative`
drops the fork before its buffers can be read.

`restore_execution` reshapes the restored stack onto the program that is
loaded now, by the rules a [hot reload](hot-reload.md#state-across-a-reload)
uses: state whose declaration no longer exists is dropped, state the new
program adds is initialized on the next run, and closures are captured again
by the next run. A checkpoint taken before an edit therefore restores into
the edited program.

A context is restored whole. If another stack is bound to the same context
as `dst`, it loses what its last run derived (static values, memo records,
captured functions; its next run rebuilds them), and any `state` of its own
that refers to a list or record made after the checkpoint is no longer
valid. Give a stack that will be rewound a context of its own.

`snapshot_state` is a lighter tool with a narrower use. It copies the map of
slots, not the heap behind them: a slot that holds a list or a record holds a
reference into the stack's heap, and the snapshot does not keep that object
alive. Use it to move state between stacks or to put state back within the
same run of work. Use a fork for a checkpoint that has to last.

The Rust signatures are in [ffi.md](../usage/ffi.md#the-embedding-lifecycle),
and the per-stack accessors a fork needs are described in
[embedding-guide.md](../usage/embedding-guide.md).

## Why a copy is exact

Petal values are immutable. `append`, `set_at` and field assignment produce
new values, and the one mutable slot, a [`var`](../syntax/var.md) cell, lives
in the heap with everything else. Nothing in a running program points outside
its context, so copying the context copies all of it, and two copies cannot
reach each other
([ffi.md, Immutability and the in-place gate](../usage/ffi.md#immutability-and-the-in-place-gate)).

A script is also run from the top on every frame, and everything that
persists between frames is in [`state`](inline-state.md). Between two frames
there is no half-finished work to capture: an execution after a frame is its
`state` and its heap.

## The petal-sdl timeline

After every committed frame `petal-sdl` forks the live execution and files
a record of the frame:

- the fork,
- the input as the frame saw it (keys, pointer, text, with press and release
  edges already worked out),
- `dt`, `time` and the frame number that were bound for it,
- the draw commands the frame emitted,
- for each draw command, the chain of calls that emitted it (the
  [emit trace](direct-manipulation.md)).

The records form a ring of the most recent frames: 600 by default, ten
seconds at 60 frames per second. `--history <n>` sets the length and
`--no-timeline` turns recording off.

**Freeze** stops the program at the latest recorded frame. While frozen,
nothing runs.

**Scrub** moves a cursor through the recorded frames. Scrubbing paints the
draw commands that were recorded for the frame under the cursor. It does not
run the script, so it costs the same however expensive a frame was.

**Resume** continues from the cursor. The live execution is restored from
the cursor frame's fork, the frames after the cursor are discarded, and the
frame number and (in a windowed session) `time()` go back to that frame's
values. Resuming at an earlier frame is a rewind.

**Rewind** by a number of frames does the same in one step without freezing.

Windowed, the keys are:

| Key | Action |
|---|---|
| `F5` | Freeze, or resume from the cursor |
| `,` / `.` | While frozen: scrub back / forward one frame. Hold to scrub continuously, add `shift` for five frames at a time |
| `F6` | Show or hide the trail |
| `F7` | Track the shape under the pointer |

While frozen the window shows a bar with the cursor's position, the frame
number, and the tracked call.

In `--agent` and `--headless` sessions the same operations are JSON commands:
`timeline`, `freeze`, `unfreeze`, `scrub`, `rewind`, `replay`, `track` and
`trail`, and `screenshot` with `"overlay": true` for the picture the window
would show. Each timeline command answers with a `timeline` status object
(frames held, cursor, tracked call, frames last replayed). The fields and
replies are in the
[petal-sdl agent protocol](../../integrations/petal-desktop-sdl/docs/agent-protocol.md#timeline),
and the command table shared with other hosts is in
[debug-protocol.md](../dev/debug-protocol.md#commands-client--engine).

Two things to know when driving it from the protocol:

- `state` reports the live execution, not the frame under the cursor. While
  frozen that is the latest frame, or the last replayed one. To read state at
  an earlier frame, `rewind` to it or `unfreeze` at it, then ask.
- `step` while frozen resumes from the cursor first and then steps.

A script switch (the example browser launching a script) clears the history.

## Replaying through an edit

A hot reload that lands while the timeline is frozen replays the recorded
future through the new code. The timeline restores the fork of the frame
before the cursor, then runs the cursor frame and every frame after it
again: for each one it binds the `dt`, `time`, frame number and input that
were recorded for that frame, runs the program that is loaded now, and
replaces the frame's fork, draw commands and emit trace with the new ones.
Frames before the cursor are left as they were.

With the script at the top of this page, stepped five frames and frozen at
the second:

```text
→ {"cmd":"step","n":5}
→ {"cmd":"scrub","to":1}
→ {"cmd":"track","x":10,"y":110}
→ {"cmd":"trail"}
← trail x: 12, 14, 16, 18, 20
   (edit SPEED = 2 to SPEED = 10 and save)
[timeline] replayed 4 frames through the new code
→ {"cmd":"trail"}
← trail x: 12, 22, 32, 42, 52
→ {"cmd":"unfreeze"}
→ {"cmd":"state"}
← {"x": 12}
```

The first frame is the past and keeps its position. The four frames from the
cursor on are what `SPEED = 10` produces from the same starting state and the
same input. Resuming continues from the replayed cursor frame.

Every kind of reload that changes behaviour triggers the replay: a patched
literal, a `config let` value, or a recompile with
[state transferred](hot-reload.md#state-across-a-reload). An edit that only
moves text (a comment, whitespace) does not. A reload that lands while the
program is running live is applied as usual and replays nothing; the frames
already recorded keep what the old code drew.

The `replay` command runs the same pass on request: from the cursor when
frozen, and over all of the history except its oldest frame when not. A
replay always starts from a recorded fork, so a value written into the live
execution with `set_state` is not carried into it.

A replay is as faithful as the recorded inputs are complete. See
[What is captured and what is not](#what-is-captured-and-what-is-not).

## Trails

`F7`, or the `track` command, picks the shape under a point on the frame on
screen: the last draw command painted there, with clipping honoured. The emit
trace names the `draw_*` call in the entry script that produced it, and the
timeline then looks for that call in every recorded frame. Each match
contributes the centre of the shape it drew, and the points are drawn as a
path: blue for frames up to the cursor, orange for the frames after it. The
`trail` command returns the same points as numbers, one per frame.

A trail follows a call site, not an object. One `draw_rect` call that draws
the player gives a clean path. A call inside a loop that draws twenty
platforms gives twenty points per frame.

After a reload the tracked call is found again by its callee name and
source position, so a trail survives an edit, including one that moves the
call by a few lines. A frame that an older version of the program drew keeps
its trail points, but `track` on such a frame is refused, because its trace
refers to code that is no longer loaded.

How a draw command is traced back to a call is described in
[direct-manipulation.md](direct-manipulation.md).

## Reproducible runs

A replay gives the same result as the original run when the program is given
the same inputs. The runtime keeps the inputs a script can read inside the
execution, where a fork copies them:

- **Random numbers.** `random`, `random_int` and `choose` draw from a
  generator whose state is part of the context. A fork copies it and a
  restore puts it back, so a rewound program draws the same numbers again.
  Across separate runs the seed comes from the clock unless it is set:
  `petal run --seed <n>` or `PETAL_SEED=<n>` on the command line
  ([CLI reference](../CLI.md#run--execute-a-program)), `Env::set_seed` in
  Rust, `pb_vm_set_seed` in C. Seed once, before the first run.
- **Noise.** The seed set by `noise_seed` is part of the context too.
- **Time and input.** A script reads `time()`, `dt()`, `frame_count()`, keys
  and the pointer from values the host binds before each frame. The timeline
  records them per frame and binds them again on replay.

In `--headless` sessions and with `step`, `dt` is fixed at 1/60 s, and in
`--headless` sessions `time()` is the frame number divided by 60, so a
scripted session is reproducible from its commands.

The `replay` [run policy](../internals/run-policies.md) is a different
thing with the same word in it: it is about the runtime skipping a call
whose inputs have not changed within a run. It has no part in time travel.

## What is captured and what is not

A fork holds everything inside the execution:

- every `state` slot, at every call path, and every `var` cell;
- the heap: lists, records, strings, class instances, closures;
- host bindings (input, `time`, `dt`, screen size, anything set with
  `set_binding`);
- counters, the random number state and the noise seed;
- the table of pending resources, each in the status it had.

It does not hold anything outside it:

- **Host-side state.** Whatever a native function keeps in the host (an
  entity table behind [handles](../usage/ffi.md#handles), a physics world, a
  texture cache, the captures of a native closure) belongs to the `Env` or
  the host, not to an execution. A fork calls the same natives and sees the
  same host state. Restoring a fork does not rewind it.
- **Effects that already happened.** A sound that was played, a file that
  was written, a request that was sent. A replayed frame calls its natives
  again, so a native with a side effect on the host has that effect again.
  Natives that emit to an output buffer and let the host act on the buffer
  are safe here: `petal-sdl` does not hand a replayed frame's output to
  `Host::end_frame`, and `print` output from replayed frames is discarded.
- **Output.** A fork starts with empty `print` and output buffers. What the
  source had emitted and not yet drained stays with the source.
- **The wall clock, where the host binds it.** In a windowed `--agent`
  session `time()` is real elapsed time, and a rewind does not move it back.
  The frame number does go back. In a plain windowed session `time()` is
  simulated and rewinds with the frame.
- **History older than the ring.** A frame that has fallen off the end
  cannot be returned to.

So a script whose behaviour is a function of its `state` and its bound
inputs rewinds and replays exactly. A script that drives host objects
through natives rewinds its own half only, and the host has to rewind the
rest or accept the difference.

A replay through an edit also assumes the recorded input still makes sense.
The input is replayed as it was recorded, frame by frame. If the edit
changes where things are, a click recorded at a point lands on whatever is
at that point now.

## What it costs

Recording a frame is a deep copy of the heap and registries, so its time
and memory grow with the size of the program's live data, and the ring holds
one copy per frame of history. The petal-sdl
[README](../../integrations/petal-desktop-sdl/README.md#time-travel) and the
[Hopper debrief](../../examples/games/hopper/DEBRIEF.md) record measured
figures.

With the timeline on, `petal-sdl` also turns on the emit trace for every
frame, and it runs every frame: the frame gate, which skips a run when
nothing it read has changed, is off, because each record needs that frame's
draw commands.

A replay runs one full frame of the script per recorded frame, plus a fork
for each. Editing while frozen near the start of a long history replays all
of it on each save.

Use `--history <n>` to shorten the ring for a script with a large heap, and
`--no-timeline` to remove the cost.

## Other hosts

The fork and restore operations are in the core runtime and are available to
any Rust embedder. The timeline is a feature of `petal-sdl`.

| Host | What it offers |
|---|---|
| `petal-sdl` | The full timeline, windowed and over the agent protocol. In `--headless` it records, rewinds, replays and reports trails with no window. |
| A host built on the `petal_sdl` library | The timeline, if its `Host` implements `present_commands` and answers `true` from `supports_timeline`, and its `GameConfig` leaves `timeline` on. The `petal-fps` and `petal-fantasy-nes` example hosts turn it off. |
| `petal-web-canvas` (WebAssembly) | `run_speculative()`, which runs one frame in a fork and returns its draw commands. The diagram-canvas sample app uses it for `capture_draw_commands` and screenshots. No history, rewind or replay. |
| Garden | No fork, rewind or replay. It can reseed the random number stream of its panels and restore top-level `state` from JSON before a panel's first frame. |
| C bridge | `pb_vm_set_seed`, `pb_vm_state_json`, and recorded input scenarios that can be applied frame by frame ([embedding-c.md](../usage/embedding-c.md#input-scenarios-replay)). Forks are not exposed. |
| `petal` CLI | `run --seed` for a reproducible run and `run --state-storage` to carry `state` between runs. No rewind. |

A host that wants its own timeline needs three things from its frame loop: a
fork after each committed frame, the inputs it bound for that frame, and a
way to present recorded output without running the script.
[`timeline.rs`](../../integrations/petal-desktop-sdl/src/timeline.rs) is
written against `Env` and `petal-ui` only and is the model to copy.

## See also

- [`examples/games/hopper/`](../../examples/games/hopper/README.md): a small
  game written to be frozen, scrubbed and edited while it runs.
- [Hot reload](hot-reload.md): what an edit does to a running program.
- [Inline state](inline-state.md): what persists between frames.
- [Direct manipulation](direct-manipulation.md): the emit trace that trails
  are built on.
- [Program modification](program-modification.md): every way to change a
  running program, in one table.
- [`core/src/env/fork.rs`](../../core/src/env/fork.rs): the fork, restore and
  diff implementation.
