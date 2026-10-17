# The frame contract

What an engine does each frame to run a Petal script, and what it may assume
about the script in return. It is the same for every host, whatever the
language of the engine: petal-desktop-sdl's game loop, Garden's panels and
the `petal-ui` test harness all follow it (`core-libs/petal-ui/src/frame_core.rs`
is the shared Rust version), and the C bridge wraps it for C++ engines. The
code for each API is in [usage/ffi.md](../usage/ffi.md) (Rust) and
[usage/embedding-c.md](../usage/embedding-c.md#the-model) (C and C++).

## The model

The script's whole top level runs once per frame. Nothing carries over from
one run to the next except `state` (and the static values the runtime keeps
for itself), so a frame's output is a function of the program, its `state`
and what the host bound before the run. The script never blocks and never
waits for the engine: it reads what the engine bound, calls natives that
answer at once, and leaves commands in output buffers for the engine to carry
out after the run.

## Each frame, in order

| Step | Rust (`Env` / petal-ui) | C bridge |
|---|---|---|
| 1. Feed input events as they arrive | `InputState::event` | `vm.mouse_move`, `vm.key`, … |
| 2. Start the frame: promote input edges, bind `dt`, the frame number and the clock | `InputState::begin_frame(dt)`, `input::bind_frame_info`, `bind_time`, `bind_dimensions`, `bind_input` | `vm.begin_frame(dt, frame, seconds)` |
| 3. Bind the engine's own per-frame data | `env.set_binding(sym, value)`; `note_host_data_changed` for data natives read | `vm.set_value(name, builder)`, `set_float`, … |
| 4. Optionally ask whether the run is needed | `env.run_needed(stack)` | not exposed |
| 5. Clear the output buffers | `take_output_buffer` / `clear_output_buffer`, `draw::clear_draw_commands` | done by `run()` |
| 6. Run | `env.reset_stack(stack)`, then `env.run(stack)` | `vm.run()` |
| 7. Drain what the script emitted | `take_output_buffer(sym)`, `draw::take_draw_commands`, `take_output` | `vm.drain(buffer)`, `drain_draw()`, `take_output()` |
| 8. Between frames, poll for edited sources and reload | `watch_program_sources(pid, entry).changed()`, then `reload_program` | `vm.sources_changed()`, then `vm.reload()` |

**Input edges are per frame.** `key_pressed` and `mouse_pressed` are true on
the frame after the event, once. Events are latched until `begin_frame`, so a
press that goes down and up between two frames is still a press. Call
`begin_frame` exactly once per frame, before the run.

**Time comes from the engine.** `dt()` is the `dt` the engine passed, and
`time()` the absolute clock it passed: monotonic seconds read fresh each
frame, never a running sum of `dt`. A script that animates from them can be
stepped at a fixed rate, paused, or replayed, because the engine decides what
the clock says. A timeline that rewinds and replays frames has to rebind the
clock it recorded, or the replayed frames see the present
([time travel](../features/time-travel.md)).

**Bind before you ask.** The frame gate compares what the last run read with
what is bound now, so it is asked after every input of the frame is bound and
before `reset_stack`. A skipped frame means the last frame's commands are
this frame's: re-present them. The C bridge always runs.
([frame gate](../dev/frame-gate.md))

**The run is synchronous.** Natives are called inside `run`, on the engine's
thread, and must answer at once. A native may not call back into the same VM
(the C bridge returns `PB_ERR_REENTRANT`), and an ordinary native cannot call
a Petal closure. A value that will take time to produce is a
[pending value](../features/pending-values.md), or data the engine binds on a
later frame.

**Output is drained after the run.** A command pushed into an output buffer
is the engine's to carry out once the run has finished. The decoded views the
C bridge hands out stay valid until the next `run`, `call`, load or reload;
copy out what must outlive the frame.

## What a native must declare

A native that reaches the engine is registered with an effect row saying what
it does: pushes into an output buffer (`EMITS`), changes engine state directly
(`EFFECT`), reads engine data the engine reports changes of
(`reads(HOST_DATA)`), or computes from its arguments only (`PURE`). The frame
gate and memoized scopes skip work on the row's word, so a scene-building
native declared as an emitter that writes the engine's lists directly stops
reaching the engine when a script moves its call into a helper. The rules and
the Cheesecake case are in
[Declaring native effects honestly](../usage/ffi.md#declaring-native-effects-honestly);
the audits that catch a wrong row are in
[dev/native-effects.md](../dev/native-effects.md).

## Errors

A runtime error ends that run and returns an error with the line and column.
The engine shows it and keeps going: the next frame runs the script again, so
a script fixed by a reload recovers without a restart. A failed run is not
rolled back: `state` keeps what the run wrote before the error, and the next
run starts from there. In the C
bridge the output buffers then hold whatever was emitted before the error,
which an engine usually discards for that frame. A failed reload leaves the old
program running ([hot reload](../features/hot-reload.md)).

## Engine objects

An object the engine owns reaches the script as a [handle](handles.md), not a
copy. The engine destroys objects between frames as it likes; a handle to a
destroyed object is stale, `is_valid(h)` is false for it, and a method call on
it raises. A handle in `state` does not survive a save and load, so keep the
durable part (an id, a name) in `state` and look the handle up again.

## Reproducible frames

For a headless test or a recorded replay: seed the random stream once before
the first run (`set_seed`, `pb_vm_set_seed`), drive `dt` and the clock from
the recording rather than the wall clock, and replay input from a scenario
(`vm.apply_scenario`). With those fixed a frame is a function of the program
and its `state`.
