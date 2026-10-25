# Programming by direct manipulation

"Programming by direct manipulation" is an approach to programming where
you modify the program by interacting with the program's output, instead
of working with the source code. This is a concept that has a lot
of research behind it (TODO add references here), but it hasn't shown
up much in mainstream languages. Petal aims to provide good support for it.

A very simple example of this could look like:

 1. A Petal script uses a function call to draw an object to the screen.
 2. While the script is running, the user clicks and drags the object.
 3. The Petal runtime figures out how to modify the original script to move the object based on the drag.

And, this same approach can work in lots of situations, not just with 2D graphics.

In order to make this work, the runtime needs a few things:

 - High introspection and tracing and provenance, so that you can find the sources for certain values or function calls.
 - Program differentiation (TODO - add link here) which helps backpropogate a numerical value towards a goal.
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

config let gap = 30.0
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

1. set `gap` to 42.5 (it's a `config let`, so it's preferred; the other
   circles spread out too)
2. change the `20` in `place` to 45 (the whole row moves)

Every alternative is checked by running the edited program before it is
offered, and it's marked `verified` only if the circle really ends up where
you dropped it. Values are rounded to nice numbers (`42.5`, not
`42.4999`) and checked again after rounding.

This works through nested helpers, loops, `map`, records, trigonometry,
recursion (the tip of a fractal tree, say) and values assigned in nested
branches (an `elsif` arm, an `if` inside an `else`, a `match` arm). Where the relationship
isn't a straight line, Petal takes a few steps towards the goal,
re-checking each time.

### In the call itself

The simple cases:

- **A literal number in the call**, like the `100` above. Petal edits that
  number.
- **A variable**, like `radius`. Petal edits the variable's definition. If
  other code also reads that variable, the edit moves those things too.
  Petal measures exactly what else moves and lists it as the alternative's
  *collateral*.
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

- **Most editable:** positions computed from `let` and `config let` values
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

`config let` marks a top-level knob:

```petal
import ui

config let offset = 10
let x = 20
draw_circle(x + offset, 110, 60, 200, 80, 80)
```

Dragging the circle above changes `offset`. `config let` values are
preferred, but other numbers are still offered further down the list (the
legacy v1 engine leaves the other `let` bindings alone entirely once a script
has any `config let`, and `--config-strict` does the same once it has any
`config let` or `tunable` knob). Every
literal in a `config let`'s data counts, so the `140` in
`config let BOX = {x: 140, y: 90}` is preferred and matches the `config`
selector as a scalar `config let` does. `config let` doesn't change how the
program runs. See [syntax/config-let.md](../syntax/config-let.md).

For numbers inside functions, or with a range, use `tunable`. To say "never
move this" use `rigid`:

```petal
let speed = 1.1
let t = 2.0
let angle = t * rigid(speed)            // the rate stays put
let radius = tunable(40, 10, 80)        // preferred, kept within 10..80
print(cos(angle) * radius)
```

Both return their value unchanged, so they cost nothing at run time. A
`rigid(...)` value doesn't pass changes through to anything upstream of it.
See [Builtins.md](../Builtins.md#direct-manipulation-hints).

The two marks have separate jobs. `config let` declares a setting of any type
that hosts list and set; `tunable` marks a number for direct manipulation,
with a range. To give a setting a range, combine them:
`config let radius = tunable(40, 10, 80)`. Hosts then see the setting `radius`
with the value `40` and the range 10..80, and a drag keeps within it
([Giving a setting a range](../syntax/config-let.md#giving-a-setting-a-range)).
For direct manipulation the two marks are one tier: a `config let` literal, a
`tunable` literal and one written both ways are all knobs and weigh the same,
so adding `tunable` to a setting only adds the range. The `config` selector
(alias `knob`), `--config-strict` and the `[config]` tag on an alternative all
mean "a knob" in this sense.

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
the value to go up by 12 rather than naming the target. To change several
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
$ cd core-libs/petal-ui
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
`petal::cli::dm_v2::propose_json(env, host, program, checkpoint, &request, entry_path)`
takes a JSON request with the CLI flags' fields (`{"channel": "shapes",
"emit": 0, "native_arg": 1, "by": 25, "deny": ["x"]}`, see
`opts_from_json`), re-runs the checkpoint speculatively with the host's
natives (a native whose effect escapes the run is not called), and answers
with the same document as `propose-edit --json`. C and C++ hosts get it as
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
