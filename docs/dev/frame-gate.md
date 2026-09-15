# The frame gate

How a host skips a frame that would draw exactly what the last one drew, and
what the runtime records to know that. This is the first step of the
incremental-rendering plan (P0 of *Reactive Rendering for Petal*, 2026-09-14):
the frame is the unit, the decision is exact, and no script changes.

## The problem

Every host runs the whole script once per frame. On a quiet frame — the
pointer still, nothing animating — that run reads the same inputs, leaves
`state` as it found it, and emits the same commands. A 5,000-row list paid
its 22 ms anyway. Garden's answer was a heuristic (sleep a panel 10 s after
the last event); every other host had none.

## The mechanism

The runtime records, per stack, what its most recent run *depended on*
(`rust/src/run_deps.rs`, held on the `Stack` as `run_deps`):

| Recorded | How | Why it matters |
|---|---|---|
| Bindings read | `PetalCxt::binding` sets a per-symbol flag; at run end each read symbol's value is fingerprinted by content | A host re-binds every input every frame (a fresh key list, a re-allocated palette record). Fingerprinting by content, not id, is what makes a quiet frame look quiet. |
| Host data read | A native that answers from outside the binding table calls `PetalCxt::note_host_read` (`host_data`, Garden's `query`, `edit_view_text`) | Only the host can know when that data changed; it says so with `Env::note_host_data_changed`. A run that read none is unaffected. |
| State settled | `StateWrite` compares old and new (records by content, budgeted); `state var` cells are snapshotted at run start and compared at run end; an in-place-mutated slot is always a change (`StateWrite.mutated`, set by lowering from escape analysis) | A counter, a motion that has not landed, an accumulator: the next run starts from different state even with identical inputs. |
| Randomness | RNG state before vs after | Another run would draw different numbers. |
| Resources | The resource table's revision | A pending value resolving between frames changes what the script would compute. |

`Env::run_needed(stack)` (and `run_needed_reason`, which names the cause)
compares the record against the present. `None` means: running now would
reproduce the last run. The host asks *after* binding the frame's inputs and
*before* `reset_stack`/`run`:

```text
input.begin_frame(dt) → bind frame_info/input → if !env.run_needed { keep last output }
                      → clear → reset_stack → run → take_draw_commands
```

Things the record cannot see are forced from outside: `Env::set_state`,
`restore_state`, `transfer_state` (hot reload), `set_seed`, `call_function`
and `restore_execution` all mark the next run needed; a host with its own
reasons calls `Env::invalidate_run`.

## What a skipped frame means

The host keeps and re-presents the last run's commands. Nothing else
happens: no prints, no emitted events, no state sweep, no GC. That is the
contract a correct script cannot distinguish from a run — with two
consequences worth knowing:

- **`print` in a frame loop prints only on frames that run.** A trace from
  `petal-ui-run` records `[]` prints on a skipped frame.
- **Per-frame declarations stand.** Garden's key claims are declared by the
  frame that made them; a skipped frame leaves the previous claims in force
  (`PanelView` checks `PanelHost::last_frame_skipped`).

A failed run is gated like any other: its read-set is whatever it read before
failing, so the error card stays until one of those inputs moves.

## Where it is wired

| Host | Gate | Retained output |
|---|---|---|
| `petal-ui` `Headless` (and `petal-ui-run`, `bench_panel`) | `Headless::gate` (on by default); `--no-gate` on the CLI and the bench | `Headless::commands` |
| Garden panels (`PanelHost::frame`) | on; query-cache revision and freshness expiry, edit-view texts, and data-provider swaps feed `note_host_data_changed` | `PanelHost` returns the last commands; `PanelView` leaves its bookkeeping alone |
| petal-desktop-sdl (`game_loop`) | `Host::frame_gating()` (default on; the fantasy console opts out because `end_frame` pumps audio from every frame's output); off while the timeline records | the default host's persistent framebuffer re-blits; `present` still runs for vsync pacing |
| petal-web-canvas | `PetalRuntime::frame_needed` from `runFrame`; a canvas resize invalidates | the canvas keeps its pixels; `lastCommandsJson` is served |
| petal-web-html | event-driven (a run per click), so no gate | the renderer now patches the DOM keyed by `key` instead of `innerHTML` |

Prelude changes that the gate exposed: `_host_theme` cached its palette
projection on `frame_count()`, which made *every* widget read the frame
number; it now caches on the palette record. `approach` no longer reads
`dt()` once it has landed, so a settled toggle stops ticking.

## Checking it

`petal-ui/tests/gating.rs` pins the semantics on small scripts and runs the
differential oracle: every panel app in `examples/`, driven by a monkey
scenario, must produce identical commands and state frame-for-frame with the
gate on and off. `petal-ui-run --gate-stats` prints frames run vs skipped and
a histogram of run reasons — the first thing to look at when a panel that
should idle keeps running.

Measured with `bench_panel` (release, 1200×800), a quiet frame of any example
app is now the gate check itself, a few microseconds, against 0.7–3.6 ms
before. A frame that does run is the subject of the next layer,
[memoized scopes](memo-scopes.md): the calls inside it whose inputs did not
move are replayed rather than run, so a pointer move costs the rows it
crossed.
