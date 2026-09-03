# Hopper — debrief

An experiment (2026-09-02): write a small side-scroller in Petal, then use it
to push on what Bret Victor's *Inventing on Principle* asks of a programming
environment — see the past, see the future, change the code and watch the
consequences move — and build whatever was missing. The game is
[`game.ptl`](game.ptl); the thing that was missing became the
[timeline](../../../integrations/petal-desktop-sdl/src/timeline.rs) in
`petal-sdl`.

## What was built

- **`Env::restore_execution(live, fork)`** — the inverse of
  `fork_execution`. A fork was already a perfect checkpoint (heap, closures,
  RNG, resources, state); it just had no way back. Restoring reshapes the
  snapshot onto the program loaded *now*, so a checkpoint from before a hot
  reload restores into the edited code.
- **The timeline in `petal-sdl`.** After every frame: fork, and file the
  fork with the frame's input, draw commands, and emit attribution. On top of
  that ring: freeze and scrub (`F5`, `,` `.`), rewind (resume from an earlier
  frame), and **replay through an edit** — a hot reload while frozen re-runs
  every recorded frame after the cursor through the new program with the
  recorded input. The whole thing is 600 lines and knows nothing about games.
- **Trails from provenance.** `F7` on a shape asks the emit trace which
  `draw_*` call painted it, then finds that call in every recorded frame and
  draws its positions as a path. After a reload the call is re-found by
  source position, so a trail survives edits. No script cooperation: the game
  never says "this is the player".
- **Agent-protocol commands** for all of it (`timeline`, `freeze`,
  `unfreeze`, `scrub`, `rewind`, `replay`, `track`, `trail`) and a
  `screenshot` that renders the frozen frame with its overlay. An agent can
  read a jump arc as a list of points and compare two arcs after an edit.

The demo: freeze mid-jump, scrub back to takeoff, track the player, lower
`GRAVITY`, save. The orange future arc rises. Lower `JUMP_VY` with the cursor
already past takeoff: nothing moves, correctly, because the impulse is in the
past. Resume: the game continues from the frozen frame under the new physics
with every other piece of state intact.

## What worked

**Fork-per-frame is cheap enough.** A release build runs Hopper at 1.9 ms per
frame headless; recording adds a heap copy and lands at 4.0 ms. Ten seconds
of history is 600 heap copies of a small game, a few megabytes. The heap's
own `fork` doc already names the next step (structural sharing) if it is
ever needed.

**State-by-name made replay-through-edit almost free.** `transfer_state`
already knew how to move a running program onto edited code; restoring an
old snapshot is the same reshaping. The only new core code is the restore.

**Emit attribution is the right primitive for trails.** The trace was built
for "hover a shape, find the line". The same chain of term ids, kept per
recorded frame, answers "where has this line been drawing" with a
membership test. Re-resolving a call by `(callee, line, column)` across a
reload was enough to keep a trail alive through edits in practice.

**The agent surface fell out for free.** Every timeline operation is a
function on `(env, live stack)`, so exposing it over stdin was a match arm
each. Being able to drive the demo from a script is also what made it
testable: `timeline.rs` has an end-to-end test that records, rewinds,
tracks, edits, replays, and checks the trail numerically.

**Garden already traces the game.** Opening `game.ptl` in `garden petal-ide`
and hovering a platform reports `draw_rect (line 210)`; hovering the sky
band reports its literal arguments as editable. The gap that showed up is the
first idea below.

## What did not, or is not there yet

**Provenance stops at data.** Platforms are records in a list literal;
`draw_rect(sx(p.x), p.y, p.w, …)` reports every argument as *computed*, so a
drag on a platform is declined. The number the user means (`y: 520` on line
39) is a literal, in the file, reachable in principle: the trace knows which
list element `p` was on that iteration. Nothing follows it there.

**Rewind rewinds `time()`, not the wall clock.** The loop now binds a
simulated clock so `time()` goes backward with the game; a script that reads
real time from the host (`elapsed()`) would see the jump.

**The per-term trace buffer is not part of the snapshot.** `explain` and
`ExplainTerm` answer questions about the frame that just ran. Scrubbing to a
past frame shows its draw commands but cannot yet answer "why is this pixel
this color *there*".

**Trails follow a call, not an entity.** Tracking a coin tracks all eight,
because one `draw_circle` in a loop draws them. That is the honest answer
from provenance (one line, eight shapes) but not the one a designer means.
Separating shapes drawn by the same call needs an identity — the loop
variable, the record, a `state(id)` key.

**The windowed chords are unverified by automation.** `F5`/`F6`/`F7` and the
scrub keys are wired through the same functions the protocol drives, and the
protocol path is tested, but no test presses a key in a window.

**Two `petal-sdl` renderer tests fail when run in parallel** (`cargo test`
without `--test-threads=1`), before and after this change; they share the
SDL TTF context. Not touched here.

## Ideas — ranked by how much they would change the experience

The small ones first, then the ones that are the point.

### Near

1. **Drag the level data.** Extend `propose_edits` through a field read of a
   record that came from a list literal, using the trace to know which
   element. Dragging a platform would rewrite `{x: 720, y: 470, …}` on line
   38. This is the single most valuable direct-manipulation gap for games:
   the *content* of a game is data literals, and today the tools can only
   reach the code around them.
2. **Trails in Garden.** The IDE has the editor beside the canvas and already
   traces every shape; add the timeline there so the trail and the edited
   line are on screen together, and the future replays as you type, not on
   save.
3. **Snapshot the trace buffer with the fork**, so `explain` works on a
   scrubbed frame: click a pixel three seconds ago, get the provenance chain
   with the values it had then.
4. **Entity identity for trails.** A `state(id)` key, a loop index, or a
   record field named `id` distinguishes the eight coins; the trail overlay
   would offer "this one" versus "all drawn by this line".
5. **Diff two futures.** Keep the pre-edit future alongside the post-edit one
   and draw both; the eye reads the delta instantly. Cheap: the old frames
   are already in hand when replay replaces them.

### Bigger

6. **The trail as a draggable goal.** Grab the apex of the orange arc and
   pull it higher. That is a goal on a *future* frame's emitted value; the
   runtime solves for the constant that satisfies it (`JUMP_VY`, or
   `GRAVITY`) through the replayed frames. Provenance plus arithmetic
   inversion handles the single-frame case today; this is the same protocol
   with the replay in the loop, and the first concrete case that wants the
   reverse-mode AD the goals document defers: the apex depends on the constant
   through forty frames of integration, not a literal argument.
7. **Every number a slider, scoped by the trace.** Hover a literal in the
   editor and see, on the canvas, every shape whose value flows from it
   (`show-dependents` restricted to the last frame's trace), then scrub the
   number and watch them all move with the future replayed. The static query
   exists; it needs the frame's trace to prune to what actually ran.
8. **Record the run as a first-class file.** A `.ptlrun`: seed, initial
   state, input per frame. Load one into any version of the program and get
   the whole timeline back. Replaying it on CI is a regression test that
   says "the player still reaches the flag", and a bug report is a file.
9. **Speculative futures on demand.** While frozen, fork the cursor and run
   thirty frames under *hypothetical* input (hold jump, do nothing, run
   left) and draw the fan of trajectories. The fork is the primitive; the
   overlay is the timeline's; only the input synthesis is new. For a designer
   this is "what can the player do from here" made visible.
10. **Bidirectional level editing.** Once idea 1 lands, the editor pane
    *is* the level editor: drag a platform on the canvas, the list literal
    changes; type a number, the platform moves. `examples/games/side-scroller/`
    carries a 500-line hand-written editor that this would delete.

### Ambitious — too big for a session, and worth it

11. **Persistent, structural history.** Make forks structurally shared
    (the heap doc already proposes `Rc` payloads) and the timeline can keep
    *minutes*, with a snapshot every frame costing what changed. Then
    history is not a debugging mode but the default: every run of every
    Petal program is scrubbable, always, at no thought from the author.
12. **Incremental replay.** Replay-through-edit re-runs whole frames. With
    the dataflow graph and a diff of the edit, most of a frame is unaffected:
    recompute only the terms downstream of the change, per recorded frame.
    This is the incremental-evaluation item in the goals document, arriving
    with a reason: it turns "save and wait" into "type and see", for the
    whole recorded future, on every keystroke.
13. **Time as a dimension of the program view.** Show a `state` variable as
    a strip chart over the recorded history in the editor gutter, next to
    its declaration; `vy` becomes a curve, `grounded` a bar. Click the curve
    to scrub. Edit the code and the curves redraw. This is the projectional
    view the goals document imagines, with time as the first projection,
    and it needs no new runtime — the snapshots already hold every value.
14. **Search over histories.** "Find a frame where the player is above a
    platform with `vy > 0` and `grounded` is false" as a query over
    snapshots, since each is a full execution that can be asked anything.
    Then "show me the input that led there" is the recorded input, and "make
    it not happen" is idea 6 pointed at a constraint instead of a position.
15. **Multiplayer time.** Two people editing one running game, each with
    their own cursor on a shared timeline; an edit by one replays for both.
    The fork model already isolates speculative work per context, which is
    most of the hard part.
16. **The game teaches the editor.** Record the *designer's* input while
    they tune: which constants they touch, in what order, while watching
    which shape. Offer that as the default policy for goal-based editing
    (`config let` written by observation), so a drag on the player resolves
    to `JUMP_VY` because that is what the designer reached for last time.

The through-line: Petal's runtime already holds the whole state of a program
as a value that can be copied, compared, restored, and asked where anything
came from. Most of what *Inventing on Principle* shows is what happens when
an environment treats that as the normal case rather than a debugging trick.
The timeline is one demonstration; the list above is what the same three
primitives — fork, restore, trace — look like when they are pushed all the
way.
