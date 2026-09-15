# Memoized scopes

How a frame that runs skips the calls inside it whose inputs have not changed,
replaying what they did last time. The second layer of the incremental
rendering plan (P1 of *Reactive Rendering for Petal*, 2026-09-14), on top of
the [frame gate](frame-gate.md): the unit is a user-function call, the
decision is exact, and no script changes.

## The problem

The frame gate skips a frame whose inputs did not move at all. A frame where
*anything* moved — the pointer, one keystroke, a clock — still re-ran the
whole script, so a 5,000-row list paid its ~11 ms on every pointer move even
though the move changed one row's hover. The cost of a frame tracked the
size of the UI, not the size of the change.

## The mechanism

Every call of a user function through a `Call` instruction is a **scope**,
addressed by the call path its frame runs on: the same
`[Call(site) | Index(i)]` chain that keys `state`
([state-call-paths.md](state-call-paths.md)). While a scope runs, the VM
records, in execution order, what it depended on and what it did
(`rust/src/memo.rs` holds the model, `backend/bytecode/vm/memo.rs` the VM
half):

| Recorded | How | Validated by |
|---|---|---|
| Arguments and captures | copied at entry | structural equality: records by content, closures by function and captures, cells by identity |
| A native that read a host binding | a **probe**: the native, its arguments, its answer | calling the native again; an unchanged answer keeps the scope valid even though the binding moved |
| `state` slots read, initialized, or written | key and value | the slot holds the value that was read (writes seen earlier in the record overlay the live value) |
| `var` cells read or written, when created outside the scope | cell and value | the cell is live and holds the value that was read |
| Host data and the resource table consulted | a flag | the host-data / resource revision is what it was |
| Child scopes | path and record serial | the child's own record validates, recursively |
| Output emitted | the tail of each output buffer since entry | — (spliced back on replay) |
| Observations recorded | term and value, in order with the children | — (re-recorded on replay) |
| State keys touched | the touch capture of §3.3a | — (retained on replay, so the sweep keeps them) |

On the next entry at the same path with equal arguments and captures, the
record is validated entry by entry. A valid record is **replayed**: the
cached output is appended to the buffers, the writes are re-applied in order
(children's included), the touched keys are marked live, the observations are
re-recorded, and the cached result is returned without pushing a frame. An
invalid record makes the call run normally, and the run replaces the record.

**Early cutoff.** A child whose record fails validation is not necessarily a
changed child. If the child's subtree wrote no state and no cell, the VM
re-executes it alone, at its own path, with its recorded arguments; if the
result, output, touched keys, writes and children come out the same, the
parent's dependency on it is satisfied and the parent stays valid (the
child's output from the re-execution is discarded — the parent's replay
already carries it). A child that writes is never re-executed
speculatively: the parent's own re-run would apply its writes a second time.
This is why `hovered(r)` is a native rather than a prelude function: as a
native it is a single probe whose answer rarely changes, so a row re-runs
only when the pointer crosses its edge; written in Petal it would be two
pointer reads, and every row would re-run on every move.

**What is not recorded.** A scope that did something a replay could not
reproduce is *effectful*, and so is every scope enclosing it: it printed
(`PetalCxt::print`), advanced a counter, touched the mutable resource table,
reseeded noise, consumed randomness, called a handle method, returned or
stored a `Pending`, or let a `var` cell it created escape through its result
or a write (a closure over a local `var`, handed out, would be replayed with
the same box still holding last frame's increments). A native that reaches
host state some other way declares itself with `PetalCxt::note_effect`, the
way one that reads host data declares itself with `note_host_read`. A scope
with more than `MAX_SCOPE_DEPS` entries is treated the same way: validating
it would cost about what running it does.

**What is not worth recording.** A scope that recorded nothing and retired
fewer than `MIN_SCOPE_INSTS` instructions is folded into its parent, whose
output range and record already cover it (`_get(style, "radius", 0)`,
`point_in`, a `draw_rect` wrapper in the prelude). A parent-less emitting
scope is recorded regardless: it is the unit a top-level loop would
otherwise re-run.

**Aliasing.** A replayed result is the record's own value, so the escape
analysis, under `OptFlags::memo_scopes`, no longer treats a user call's
result as a fresh container the caller may mutate in place
(`escape::analyze_with`). Builtin results are unaffected. In-place mutation
of a `state` slot inside a scope (`StateWrite.mutated`) makes the scope
effectful, since the slot already holds the edited object and no comparison
could tell.

**Lifecycle.** Records are evicted when a run completes without visiting
them, the rule the state sweep applies. A program transfer (hot reload)
clears the table, since records name the old program's functions. Forcing a
run, setting state from the host, and restoring a snapshot need nothing
special: the validation reads the live slots.

## Where it is wired

| | |
|---|---|
| Switch | `OptFlags::memo_scopes` (on by default; off under `PETAL_OPT=off` / `--no-opt` with the other optimizations), `Env::set_memo_scopes`, `Headless::memo` |
| Off regardless | while the `explain` trace is on (`Env::memo_enabled`): a trace of a replayed scope would have no instructions in it |
| Host calls | `Env::call_function` runs outside any frame's run and is never a scope |
| Counters | `Env::memo_stats` / `memo_slots`; `petal-ui-run --memo-stats`; `bench_panel` prints them |
| Reference | `petal-ui-run --no-memo`, `bench_panel --no-memo`: every call runs |

Observation (`panel.values`, the debug server) works with memoization on: a
replayed scope re-records the bindings its run recorded, so a host reading
them sees what a run would have left. Garden leaves observation on, and
benefits.

## Checking it

`petal-ui/tests/memo.rs` pins the semantics on small scripts and runs the
differential oracle: every panel app in `examples/`, driven by a monkey
scenario with the frame gate off, must produce identical commands, state and
observations frame for frame with memoization on and off. `rust/src/memo.rs`
has the model's unit tests and end-to-end checks through `Env::run`.
`petal-ui-run --memo-stats` prints hits, misses, records, inlined, effectful,
re-executions and cutoffs — the first thing to look at when a widget that
should replay keeps running (an `effectful` count that grows every frame
means something in it prints or draws randomness).

Measured with `bench_panel` (release, 1200×800, `--wiggle`, p50 over 60
frames):

| Script | Every call runs | Memoized |
|---|---|---|
| synthetic list, 5,000 rows in `state` | 10.7 ms | 3.4 ms |
| synthetic list, 1,000 rows in `state` | 2.2 ms | 0.7 ms |
| examples/dashboards/analytics-dashboard | 2.3 ms | 0.64 ms |
| examples/dashboards/server-monitoring | 4.9 ms | 1.3 ms |
| examples/productivity/photo-adjust | 3.3 ms | 0.68 ms |
| examples/productivity/notes | 0.88 ms | 0.26 ms |
| examples/productivity/kanban | 0.61 ms | 0.22 ms |
| examples/productivity/todo | 0.49 ms | 0.38 ms |
| examples/productivity/spreadsheet | 1.32 ms | 1.15 ms |

The synthetic row is a function drawing one rect and one label with a
`hovered(rect)` check. What remains of the list's 3.4 ms is the top-level
loop that calls the rows (it is not a scope: the root frame has no record)
and 5,000 validations of about 0.4 µs each, most of it the `hovered` probe
re-evaluated per row. Both are the subject of the next steps — dependency
classes to guard whole blocks, keyed collections and a hit-test index so a
pointer move touches only the rows whose edge it crossed.
