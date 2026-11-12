# Programming by direct manipulation

"Programming by direct manipulation" is an approach to programming where
you modify the program by interacting with the program's output, instead
of working with the source code. This is a concept that has a lot
of research behind it (Bret Victor's *Inventing on Principle* is the
best-known statement of it), but it hasn't shown up much in mainstream languages. Petal aims to provide good support for it.

A very simple example of this could look like:

 1. A Petal script uses a function call to draw an object to the screen.
 2. While the script is running, the user clicks and drags the object.
 3. The Petal runtime figures out how to modify the original script to move the object based on the drag.

And, this same approach can work in lots of situations, not just with 2D graphics.

In order to make this work, the runtime needs a few things:

 - High introspection and tracing and provenance, so that you can find the sources for certain values or function calls.
 - Program differentiation ([differentiation.md](differentiation.md)), which helps backpropogate a numerical value towards a goal.
 - Frozen state and safe replayability. When using this approach, we often need to safely re-run a program without effects and without modifying state.
 - Source code rewriting tools that preserve formatting.
 
## What it looks like

Take this sketch:

```petal
import ui

let radius = 40
draw_circle(100, 100, radius, 200, 80, 80)
```

In an editor that supports direct manipulation (such as
[Garden](../../garden/docs/petal-ide-mode.md)), hovering the circle on the
canvas highlights the `draw_circle` call that drew it. Dragging the circle
changes the `100, 100` on line 4. Changing its size would edit the `40` on
line 3, because that's where the radius actually comes from. The rest of the file
keeps its formatting; only the numbers change.

The edit is live: as you drag, the predicted change is written into the
code and the running sketch is reloaded with it, so the canvas shows the
real program, not a ghost of the shape. When you let go, Petal checks the
answer by running it, ranks every way it found, and keeps the best. Picking
another redoes the same drag that way, from the code as it was when you
pressed. Petal remembers that pick for the rest of the session: the next
drag prefers the same numbers when they can reach its goal, and ranks as
usual when they can't. The preference is never written into the source,
and you can forget it at any time.

## What can be edited

Petal can edit **any number the shape depends on, through any function**,
as long as the shape moves smoothly when that number changes. (This is the
v2 engine, the default. The legacy v1 engine, still available with `--v1`,
only looks at the call that drew the shape; see
[The legacy engine](#the-legacy-engine-v1) below.)

### Through function calls

Take a row of circles placed by a helper:

```petal
import ui

let gap = tunable(30.0)
fn place(i)
  return 20 + i * gap
end
for i in range(0, 4) do
  draw_circle(place(i), 100, 10, 200, 80, 80)
end
```

Drag the third circle 25 pixels to the right. The `x` of that
`draw_circle` call is `place(2)`, a function call. Petal works it out by
experiment: it runs the program again with each number that could
matter nudged a little (`gap` to 30.03, the `20` to 20.02, and so on) and
watches how far the circle moves. From those measurements it knows that the
circle moves 2 pixels per unit of `gap` and 1 pixel per unit of the `20`,
and it solves for the values that put the circle where you dropped it.
It then gives you a ranked list:

1. set `gap` to 42.5 (it's a setting, so it's preferred; the other
   circles spread out too)
2. change the `20` in `place` to 45 (the whole row moves)

Every alternative is checked by running the edited program before it is
offered, and it's marked `verified` only if the circle really ends up where
you dropped it. Values are rounded to nice numbers (`42.5`, not
`42.4999`) and checked again after rounding.

This works through nested helpers, loops, `map`, records, trigonometry,
recursion (the tip of a fractal tree, say) and values assigned in nested
branches (an `else if` arm, an `if` inside an `else`, a `match` arm). Where the relationship
isn't a straight line, Petal takes a few steps towards the goal,
re-checking each time.

### In the call itself

The simple cases:

- **A literal number in the call**, like the `100` above. Petal edits that
  number.
- **A variable**, like `radius`. Petal edits the variable's definition. If
  other code also reads that variable, the edit moves those things too.
  Petal measures exactly what else moves and lists it as the alternative's
  *collateral*. By default every other shape counts the same; when some
  matter more (platforms more than coins), say so per output channel with
  `petal propose-edit --collateral solid=10`
  ([CLI](../CLI.md#propose-edit--propose-source-edits-that-change-an-emitted-value)).
- **A calculation**, like `x + offset`. There may be more than one answer:
  you could change `x`, or you could change `offset`. Petal offers both, and
  the editor asks which one you meant.

### The legacy engine (v1)

Before v2, Petal inverted the arithmetic of the drawing call itself: a
literal, a variable, or `+ - * /`. That engine (v1) refuses values that come
out of a function call. v2 became the default after the examples study in
the
[v2 plan](../dev/direct-manipulation-v2-plan.md#m5-examples-ladder-and-complexity-study)
cleared the bar the plan set for it (94% of ladder goals and 93% of drags
on real apps verified, at a median of 56 runs). v1 is still there
with `petal propose-edit --v1` (or `engine: "v1"` over MCP) for tools that
read its output.

### What can't receive the change

Some numbers have no useful effect on the shape, or must not be touched.
Petal leaves these out and says why (`--explain` lists them):

- **Random numbers.** A shape scattered with `random()` stays where the
  random draw put it. The draw isn't something you can edit, and the numbers
  that only feed it (`random_int(0, 10)`) are left alone. While Petal measures,
  every random draw gives the same value it gave before, so the only thing
  that changes between runs is the nudge.
- **Numbers that only choose what runs**: a loop bound, a comparison, an
  index. Changing them doesn't slide the shape, it makes a different shape.
- **Library code.** Numbers in the standard library and in packages are
  never edited.
- **`state` initializers** in a running program. The program keeps its state
  across a reload, so changing the initial value wouldn't move anything. On a
  fresh start (the command line) they can be edited.
- **Anything the client ruled out** (see below).

## Making a program easy to edit

How well this works depends on how the program is written. It's a sliding
scale, not a yes-or-no:

- **Most editable:** positions computed from `let` values and settings
  through plain functions. Every number has a clear effect.
- **Less editable:** values that pass through `var` cells, or that build up
  over a loop (`set total = total + w`). They still work, but they cost a
  little more, and more of the program moves together.
- **Least editable:** values fed back from earlier frames through `state`,
  and values tied to the clock. An orbit at `time() * speed` has two numbers
  that move the planet right now, the radius and the speed, but changing the
  speed changes the animation, not the layout. Petal notices numbers that
  move a shape along its own motion and ranks them low.

### Marking what's tunable

You can tell Petal which numbers are meant to be adjusted.

A *setting*, a top-level `let` whose whole initializer is `tunable(…)`,
marks a top-level knob:

```petal
import ui

let offset = tunable(10)
let x = 20
draw_circle(x + offset, 110, 60, 200, 80, 80)
```

Dragging the circle above changes `offset`. Settings are preferred, but
other numbers are still offered further down the list (the legacy v1 engine
leaves the other `let` bindings alone entirely once a script has any
setting, and `--knob-strict` does the same once it has any setting or
`tunable` knob). Every literal in a setting's data counts, so the `140` in
`let BOX = tunable({x: 140, y: 90})` is preferred and matches the `knob`
selector as a scalar setting does. A setting doesn't change how the program
runs. See [Settings](../syntax/settings.md). (The deprecated
`config let offset = 10` means the same as `let offset = tunable(10)`.)

For numbers inside functions, use `tunable` on the literal. To say "never
move this" use `rigid`:

```petal
let speed = 1.1
let t = 2.0
let angle = t * rigid(speed)            // the rate stays put
let radius = tunable(40, 10, 80)        // preferred, kept within 10..80
print(cos(angle) * radius)
```

Both return their value unchanged. A
`rigid(...)` value doesn't pass changes through to anything upstream of it.
See [Builtins.md](../Builtins.md#direct-manipulation-hints).

Where `tunable` is written decides its job. As the whole initializer of a
top-level `let` it declares a setting of any type that hosts list and set;
anywhere else it marks a number for direct manipulation. Either can carry a
range: `let radius = tunable(40, 10, 80)` is the setting `radius` with the
value `40` and the range 10..80 to hosts, and a drag keeps within it
([Ranges and options](../syntax/settings.md#ranges-and-options)). For direct
manipulation the two are one tier: a setting's literal and a `tunable`
literal are both knobs and weigh the same. The `knob` selector (alias
`config`), `--knob-strict` (alias `--config-strict`) and the `[config]` tag
on an alternative all mean "a knob" in this sense.

### Telling Petal which numbers may move

Sometimes the first answer is the wrong one. The editor (or an AI agent)
can say which numbers a drag may change, and let Petal work out the values:

- **allow** a set: a variable name, a line (`line:12`), a function's body
  (`fn:place`), every `config` value, or one number by where it is written
  (`at:core/level.ptl:41:12`, line and column) or by the `term` id the
  `--json` report gives it (`term:123`). Only those move. With two or more of
  them and one goal, Petal picks the smallest change that spreads across
  them.
- **deny** a set: everything else may move.

### Best effort

A host with nowhere to show a list of alternatives, and no source view to
select lines in (a game window, a document editor, a C++ embedder), asks
for **best effort** instead: `policy: "best_effort"` on the request,
`--best-effort` on the command line. It answers with one verified edit or
an error, and chooses by a fixed rule:

1. **Knobs first.** If the `tunable` and setting literals can reach the
   goal, only they move. This is a filter, not a preference: a knob wins
   even over a number written in the drawn shape's own call.
2. **Never `rigid`, never `const`.** A `rigid(…)` literal never moves (as
   always), and neither does one that reaches the goal only through a
   `const` binding (`const SIZE = 16`, `let SIZE = const(16)`). A literal
   that also reaches the goal some other way is not excluded.
3. **The cheapest tier, split equally.** Of the rest, the literals with the
   lowest cost weight move: a number in the shape's own call before a
   top-level binding, before a function body, before another file, with
   `var`, `state` and clock penalties as in the ranking. When several tie,
   each moves by the amount that would produce an equal share of the change
   on its own. With `let a = 10.0`, `let b = 20.0` and `draw(a + twice(b),
   …)`, a drag of +20 moves `a` by 10 and `b` by 5. With two goals (x and
   y of one drag), a literal that moves only x takes x's whole share. If
   whole numbers can't be split exactly, the remainder goes to one literal.
4. **Verified, or the next tier.** The edit is checked by a re-run like any
   other. If the cheapest tier can't reach the goal (an int that steps past
   it), a dearer tier that can reach it alone is tried, up to three.
5. **An error when nothing works.** When no literal may move, or no edit
   reaches the goal, the request fails with the reason, and nothing is
   written:

   ```
   $ petal propose-edit --best-effort --channel shapes --emit 0 --value --to 60 rigid.ptl
   Error: nothing can move the goals: no editable literal moves the goals (line 2: declared `rigid`)
   $ petal propose-edit --best-effort --channel shapes --emit 0 --value --to 200 sin.ptl
   Error: no edit reaches the goals: set `a` to 1.0 (line 2); moves nothing else; misses the goal by 57.926
   ```

   (`sin.ptl` draws `100 + sin(a) * 50.0` with `let a = tunable(0.5, 0.0,
   1.0)`: the knob is the only thing that moves, and its range tops out
   short of 200.) A run an `assert` rejected is named in the message too.

In an editor with a line selection, the selection decides instead ("with a
selection, only those lines; without one, best effort").

Best effort solves fewer goals than the ranked list offers. On the 105
goals of the examples ladder (`test/ui-corpus/tests/dm_study.rs`,
`best_effort_vs_ranked`, measured 2026-11-11):

| | Ranked (top alternative) | Best effort |
|---|---|---|
| Solved (verified, and the edited program meets the goal after a reload) | 99 | 94 |
| Solved with the edit the ladder expects | 91 | 64 |
| No answer (an error) | 1 | 11 |

The 5 goals only the ranking solves are ones where it moves a dearer number
that best effort's rule passes over: a bounded knob that can't reach the
goal while a plain literal can (`l07b-fractal-tree-hints`, twice), an
equal split over whole numbers that ends one pixel off where moving one of
them alone is exact (`l08-flex-row`, `big-server-monitoring`), and a goal with a `deny`
whose cheapest remaining tier misses (`l10-bar-chart`). Where both solve, best effort
often spreads the change over more literals (`l04-clock-hand`'s `to-4-oclock`
moves three numbers instead of `hours` alone), which is why fewer match the
expected edit.

### Constraints between literals: `assert`

Some numbers only make sense together: a patrol ends after it starts, a
width is positive, a gap fits inside its wall. Say so with an `assert` where
the numbers meet, usually the function that builds the data:

```petal
import io

fn patrol(x0, x1)
  assert(x1 >= x0, "a patrol ends after it starts")
  return {from: x0, to: x1}
end

let beetle = patrol(3.5, 5.5)
push_output(symbol("enemies"), beetle.to)
```

Every measuring run executes the assert. A run that fails it is not taken:
Petal shortens the step and tries again, so it never proposes an inverted
patrol. Dragging the end to 2.5 stops at the start instead, and the answer
says what held it back:

```
$ petal propose-edit --channel enemies --emit 0 --value --to 2.5 --tol 0.01 level.ptl
engine v2 · seed 0 · 14 runs · converged within budget
goal: enemies[0] value (push_output, line 9): 5.5 -> 2.5
1 alternative:
  1. [unverified] change `5.5` to 3.55 (line 8); moves nothing else; misses the goal by 1.050; rejected beyond it: assertion failed at level.ptl:4 (x1 >= x0): a patrol ends after it starts
       line 8 patrol(x1:): 5.5 -> 3.55
       reaches 3.55 · off by 1.05 · score 210.6855
       rejected: assertion failed at level.ptl:4 (x1 >= x0): a patrol ends after it starts · 3 runs, first at `5.5` -> 2.5 (line 8)
```

An answer that stops short is still rounded to a clean number (`3.55`, not
`3.5546874999999756`), as long as that moves it no further from the goal than
the tolerance. To reach 2.5, move the start too, with two goals or an
`--allow` naming both numbers.

The assert runs in the program as well, so it also catches a hand edit that
breaks the rule. Keep it cheap: it runs on every frame. Any other error a
run raises (a missing field, a division by zero) rejects the step the same
way, and is reported the same way.

## How it stays safe

To measure, Petal runs the program many times: usually a few dozen runs,
at most a couple of hundred. Those runs must not do anything you'd notice,
and two runs must differ only by the number being nudged:

- **No side effects.** Anything a function does outside the program (print,
  write a file, send a message) is skipped during the measuring runs. A
  host's native function says whether its effect is safe to repeat (see
  [dev/native-effects.md](../dev/native-effects.md)); anything not marked
  safe is skipped.
- **Frozen inputs.** `state` is read but never written, the clock, the mouse
  and host data give the same values each run, and random numbers are
  replayed by where they're drawn.
- **Checked.** Petal runs the unmodified program twice first and compares
  the results bit for bit. If they differ, something isn't frozen, and Petal
  says so instead of guessing.
- **Bounded.** Each run has a step limit and a time limit, so a nudge that
  makes a loop run away is just a failed experiment.

The live program is never touched. Every run starts from a copy.

## Limits

- Only numbers are solved for. A goal that's a string or a bool is passed
  to the legacy v1 engine (and comes back unverified).
- A shape that only exists for some values (behind an `if`) can be solved
  only while it stays on the same side of the `if`. Alternatives that
  change which code runs are marked as such.
- When the goal can't be reached (`sin` never goes above 1), Petal gets as
  close as it can and marks the alternative unverified, with how far off
  it is.
- Integer literals stay integers unless you allow them to become decimals.
- A rule between two numbers (`x1 >= x0`) is stated with an `assert`, not
  with a mark on the numbers: Petal respects it by rejecting the runs that
  break it. There is no range on a function parameter yet (`tunable` marks
  one literal).
- On the command line, the program is measured on its first run. In an
  editor, it's measured on the frame you grabbed the shape in.

## Trying it from the command line

You can try direct manipulation without an editor. Take this `sketch.ptl`:

```petal
import io
let offset = 10
let x = 20
push_output(symbol("shapes"), x + offset)
push_output(symbol("shapes"), "label")
```

`petal run --trace-emits` shows each value the script produced and where it
came from:

```
$ petal run --trace-emits sketch.ptl

Channel 'shapes' (2 emits):
  [0] push_output [line 4] <- 30
      native args: [0, 30]
      arg 0: computed
      arg 1: computed
  [1] push_output [line 5] <- "label"
      native args: [0, {"str":"90eeeb8b2c329bfe"}]
      arg 0: computed
      arg 1: literal = "label" (edit line 5)
```

`petal propose-edit` asks: "what should I change so that this value is
different?"

```
$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42 sketch.ptl
engine v2 · seed 0 · 11 runs
goal: shapes[0] arg 1 (push_output, line 4): 30 -> 42
3 alternatives:
  1. [verified] set `x` to 32 (line 3); moves nothing else
       line 3 `x`: 20 -> 32
       reaches 42 · score 1.06
  2. [verified] set `offset` to 22 (line 2); moves nothing else
       line 2 `offset`: 10 -> 22
       reaches 42 · score 1.12
  3. [verified] set `offset` to 12 (line 2), set `x` to 30 (line 3); moves nothing else
  ...
Apply one with --apply --pick N; narrow with --allow/--deny; see why with --explain.
```

Add `--deny x` to rule out `x`, which leaves one alternative, and `--apply`
to write it to the file. `--native-arg 1` addresses the raw second argument
of `push_output` instead of the user-file call's, and `--by 12` asks for
the value to go up by 12 rather than naming the target. When any value in
a range will do, say so: `--at-most 3`, `--at-least 1` or `--within 0..1280`
(a coin "within 0.6 of the bot's path", a panel "inside the screen"). A range
that already holds asks for no edit, and one that does not is moved just
inside its bound instead of to an exact number the program may not be able
to reach. To change several
values at once (like x and y for a single drag), repeat the goal. `x` and
`offset` are int literals, so they stay ints: `--to 42.5` misses by half
unless you add `--allow-respell`.

`--v1` runs the legacy engine, whose output keeps its old shape:

```
$ petal propose-edit --v1 --channel shapes --emit 0 --arg 1 --to 42.5 sketch.ptl
2 proposals:
  1. set `x` to 32.5 (line 3)
  2. set `offset` to 22.5 (line 2)
Narrow with --configurable <var> / --static <var>, or apply one by hand.
```

`--explain` shows every number it considered and why it kept or dropped
each one; `--allow`/`--deny` say which may move; `--apply --pick N` writes
the alternative you chose (every file it touches), and plain `--apply` writes
the only verified one when there is just one. A value that only exists after
the program has run a while (a score in `state`, an enemy that spawns on frame
3) needs `--frames N`: it runs `N` frames first and drags frame `N + 1`. All options, and the
`--json` shape, are in the
[CLI reference](../CLI.md#propose-edit--propose-source-edits-that-change-an-emitted-value).

### Scripts that draw with `ui`

`petal propose-edit` only has the core natives, so a script that does
`import ui` (a HUD, a Garden panel, the examples in
`examples/direct-manipulation-v2/`) does not load there. petal-ui ships the
same command with its natives registered:

```
$ cd core-libs/petal-ui-headless
$ cargo run --bin petal-ui-propose-edit -- hud.ptl --trace-emits
$ cargo run --bin petal-ui-propose-edit -- hud.ptl \
    --channel draw_commands --emit 2 --native-arg 2 --to 42 --deny bar_h
```

It runs the app headlessly for `--frames N` frames (default 1; `--size`,
`--scenario`, `--host-data` and `-I` as in `petal-ui-run`) and drags from
the frame after the last one. Every draw call is an emit on the channel
`draw_commands`, in drawing order; `--trace-emits` lists them with their
raw arguments, so `--native-arg 2` of `draw_text("…", x, y, color)` is its
`y`. The goal and filter flags, the text report and the `--json` document
are the core command's.

## Hosts

Drag-to-edit is an **edit mode** in each host that draws: a mode you turn
on, not a gesture on the running program. While it is on, the program is
frozen (its inputs and `state` held), so input never reaches the script and
every speculative re-run starts from the frame on screen. Clicking a drawn
item names the call that drew it; dragging it sets a goal on its position,
and Petal solves for an edit.

Which numbers may change: in a host with a source view, the lines you
select become the request's allow set (`line:N` selectors, for this session
only, never written into the source). With nothing selected, or in a host
with no source view, the request runs [best effort](#best-effort): one
answer or an error.

| Host | Turn it on | Choosing what may change | The edit |
|---|---|---|---|
| Garden (`garden petal-ide`) | `Cmd+Shift+D` / `:DragMode` | `:DragAllow`, the candidate picker, gutter hints | Applied live while you drag; ranked alternatives in a panel on release; a pick is remembered (`:DragForget`) |
| `petal-sdl` | `F9` | Best effort | Written to the file, then hot reloaded |
| `petal-web-canvas` (`diagram-canvas`) | **Edit Mode** in the sidebar | The lines selected in the page's source editor; none selected (or the editor closed) is best effort | Put in the editor as one undoable change and run; the other alternatives listed to pick from |

**petal-sdl.** `F9` turns edit mode on and freezes the timeline at the
current frame (`F5` and `,` / `.` still scrub). Click a shape: the banner
names its call and source line (`` `draw_rect` at line 5: draw_rect(60 + i
* gap, …) ``), and its outline is lit. Drag it: on release the drag runs a
best-effort request against a re-run of the frozen frame, writes the answer
to the sketch (or the imported file the number lives in), and the hot reload
replays the frozen frame through the edit. When no edit reaches the spot
(every number behind the shape is `rigid`, `const` or doesn't move it), the
banner shows the reason and nothing is written. The applied edit stays in
the banner, `z` undoes it (when the file still holds it), and `F9` again
leaves edit mode and resumes the game. The agent protocol has the same
operations (`edit_mode`, `edit_pick`, `edit_drag`, `edit_undo`; see
[petal-sdl's agent-protocol.md](../../integrations/petal-desktop-sdl/docs/agent-protocol.md#edit_mode--edit_pick--edit_drag--edit_undo)).

**petal-web-canvas (`diagram-canvas`).** The wasm runtime keeps an emit
trace while edit mode is on (`PetalRuntime.set_emit_trace`) and a pre-run
checkpoint of each frame (`set_edit_checkpoint`), and answers in JSON:
`edit_pick(x, y)` names the call under the pointer (`callee`, 1-based
`line`, `bounds`); `edit_begin(index, lines)` opens a drag with the selected
lines as the allow set, or best effort when `lines` is `[]`;
`edit_preview(dx, dy)` is the per-move answer; `edit_finish(dx, dy)` returns
the alternatives, each with the whole new sketch text, or `{ok: false,
error}`. For a live drag, `edit_live(dx, dy)` writes the per-move answer
into the press-time source and hot-reloads it in place (state and drag
kept), and `run_checkpoint()` draws the frame on screen under it;
`edit_finish` still verifies from the press-time source, `edit_show` puts
the chosen text in place, and `edit_choose(k)` applies alternative `k` and
remembers it as the sticky choice (`edit_preference`,
`edit_clear_preference`). The `diagram-canvas` sample app is the reference page
(`examples/custom-apps/diagram-canvas/src/edit-mode.ts`). **Edit Mode**
freezes the frame loop after one traced frame and lays an overlay over the
canvas, so the script stops receiving input. Clicking a shape outlines it,
names its call in the edit panel and selects that line in the editor; widen
the selection there (any lines, with the editor's own selection) and the
panel says which lines may change, which the editor marks. Dragging a shape
runs the request on release: the best answer goes into the editor as one
change (so one undo takes it back) and runs at once, and with a selection
the other alternatives are listed to pick from. A refusal shows its reason
and changes nothing. The selection is the page's: it is never written into
the source. Its end-to-end test is `npm run test:e2e --workspace
examples/custom-apps/diagram-canvas` (Playwright, headless Chromium).

**For host authors.** The host-independent half lives in
`petal_ui::edit_mode` (no native-only dependencies, so it builds for wasm):

- `DragFilter::allow_lines` / `allow_selection` turn a line selection into
  the allow set;
- `GoalTarget::at(origins, index, property)` names the grabbed emit from the
  frame's emit trace;
- `EditSession::open` probes a checkpoint (a fork of the stack before the
  frame ran, with its inputs bound) and opens a `DmSession`; `preview` is the
  per-move linear answer, `preview_live` the same written into the
  press-time source for a host to hot-reload under the drag (the release
  accepts the session's own live text), `finish` verifies and returns the
  alternatives (one under `SessionOptions::best_effort()`) or the reason for
  a refusal; `SessionOptions::prefer` is the sticky choice, a soft
  preference (`DragAlternative::preference` of the last pick);
- `apply_rewrites_to` / `DragAlternative::apply_to` splice the answer into
  the source text;
- `hit_test` / `bounds` find the drawn item under a point, and
  `describe_call` names the call in the sketch that drew it.

A host supplies the checkpoint, a `SpecHost` that installs its providers
around each speculative run, and the UI. Garden's
`garden-script/src/panel/dm.rs`, petal-sdl's `src/edit_mode.rs` and
petal-web-canvas's `rust/src/lib.rs` are its users. Turning emit tracing
on after frames have run drops the stacks' memo records (a record made
untraced has no origins to replay), so the first traced frame attributes
every emit.

## For AI agents

The same tools are available to AI agents through the dev MCP server, as
`TraceEmits` and `ProposeEdit` (see [../dev/mcp-server.md](../dev/mcp-server.md)).
A good workflow:

1. `TraceEmits` to find the shape: its channel, emit index, and the raw
   arguments (`native_args`) of the native that drew it.
2. `ProposeEdit` (engine v2 is the default) with the goal (`nativeArg: 0, by: 25`, or
   `to`), and `explain: true`. Read the ranked `alternatives` and the
   `candidates` table.
3. If the top alternative changes the wrong thing, ask again with `allow`
   naming the numbers you want moved (`["gap"]`, `["fn:place"]`,
   `["line:12"]`) or `deny` for the ones to keep. Petal works out the values.
4. Apply the `edits` of a `verified` alternative.

## Adding it to your own app

An app that runs Petal scripts can offer the same experience. The steps
are:

1. Turn on tracing before running the script, so each value the script
   outputs remembers which call produced it.
2. When the user clicks, work out which output they clicked on. Only your
   app knows what its outputs look like, so this part is up to you.
3. Ask Petal where that output came from (to highlight the code), or ask
   for edits that would change it to a new value.
4. Apply the chosen edit to the source, then re-run the script.

After each edit, use the results of the new run. Results from before the
edit don't apply to the edited program.

A host that registers natives of its own can run the v2 engine on its own
`Env` without going through a command line:
`petal::propose_edit::propose_json(env, host, program, checkpoint, &request, entry_path)`
takes a JSON request with the CLI flags' fields (`{"channel": "shapes",
"emit": 0, "native_arg": 1, "by": 25, "deny": ["x"]}`, see
`opts_from_json`), re-runs the checkpoint speculatively with the host's
natives (a native whose effect escapes the run is not called), and answers
with the same document as `propose-edit --json`. Add `"policy":
"best_effort"` for one answer or an error ([Best effort](#best-effort)),
when the app has no list to choose from. C and C++ hosts get it as
`pb_vm_propose_edit` in the C bridge
([ffi.md](../usage/ffi.md#direct-manipulation)).

The [embedding guide](../usage/embedding-guide.md) covers running scripts
from a host app, and Garden's
[petal-ide-mode.md](../../garden/docs/petal-ide-mode.md) is a complete
example.

## See also

- [goal-based-editing.md](goal-based-editing.md): the same idea of "say
  what you want, let Petal find the edit", applied to config files.
- [../usage/embedding-guide.md](../usage/embedding-guide.md): running Petal
  from a host app.
- [Garden's petal-ide-mode.md](../../garden/docs/petal-ide-mode.md): hover a
  shape on the canvas, see its code highlighted in the editor.
