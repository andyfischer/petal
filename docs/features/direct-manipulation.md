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

When you grab something, Petal looks at how each of its values was written:

- **A literal number in the call**, like the `100` above. The simple case:
  Petal edits that number.
- **A variable**, like `radius`. Petal edits the variable's definition. If
  other code also reads that variable, the editor can warn you, since the
  edit will move those things too.
- **A calculation**, like `x + offset`. Petal can still solve for a new
  value, but there may be more than one answer: you could change `x`, or
  you could change `offset`. Petal offers both, and the editor asks which
  one you meant.

Some values can't be edited by dragging, such as a value returned from a
function call. In that case Petal makes no suggestion instead of guessing,
and the editor can point you to the code instead.

## Marking what's tunable with `config let`

You can tell Petal which variables are meant to be adjusted by declaring
them with `config let`:

```petal
import ui

config let offset = 10
let x = 20
draw_circle(x + offset, 110, 60, 200, 80, 80)
```

Once a script has any `config let` bindings, dragging prefers to change
those and leaves ordinary `let` bindings alone. Dragging the circle above
changes `offset`, with no question asked. Literal numbers written directly
in a call can always be edited. `config let` doesn't change how the
program runs. See [syntax/config-let.md](../syntax/config-let.md) for more.

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
      arg 0: computed
      arg 1: computed
  [1] push_output [line 5] <- "label"
      arg 0: computed
      arg 1: literal = "label" (edit line 5)
```

`petal propose-edit` asks: "what should I change so that this value is
different?"

```
$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 sketch.ptl
2 proposals:
  1. set `x` to 32.5 (line 3)
  2. set `offset` to 22.5 (line 2)
Narrow with --configurable <var> / --static <var>, or apply one by hand.
```

Add `--static x` to rule out `x`, which leaves one proposal, and `--apply`
to write it to the file. `--apply` only writes when there is exactly one
choice left. To change several values at once (like x and y for a single
drag), repeat `--arg <n> --to <value>`. All options are in the
[CLI reference](../CLI.md#propose-edit--propose-source-edits-that-change-an-emitted-value).

The same tools are available to AI agents through the dev MCP server, as
`TraceEmits` and `ProposeEdit` (see [../dev/mcp-server.md](../dev/mcp-server.md)).

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
