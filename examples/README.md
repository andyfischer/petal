# Examples

Runnable Petal programs, grouped by what they are:

| Directory | What's in it |
|-----------|--------------|
| [`console/`](console/) | Console programs demonstrating language features — the golden-tested corpus |
| [`games/`](games/) | Games — pong, breakout, snake, asteroids, flappy, a [`memory`](games/memory/) matching game, a [`tower-defense`](games/tower-defense/) mini-game, a [`physics-playground`](games/physics-playground/), an interactive solar system, [`hopper`](games/hopper/) (a small SDL-hosted side-scroller built for time travel and direct manipulation), and five headless console programs written as language stress tests: [`2048`](games/2048/), [`boids`](games/boids/), [`minesweeper`](games/minesweeper/), [`terrain`](games/terrain/), [`tetris`](games/tetris/) |
| [`productivity/`](productivity/) | Applications — calculator, todo, notes, calendar, email client, kanban, CRM, spreadsheet, paint, vector editor, photo adjust, node editor, markdown editor, command palette, a [`music-sequencer`](productivity/music-sequencer/) |
| [`dashboards/`](dashboards/) | Data-visualization demos — analytics, server monitoring, finance, a [`network-graph`](dashboards/network-graph/) explorer, a [`timeline-explorer`](dashboards/timeline-explorer/) |
| [`ui/`](ui/) | Component-library showcases — [`bloom-gallery`](ui/bloom-gallery/), every component in [bloom](../petal-libs/bloom/) |
| [`custom-apps/`](custom-apps/) | Full applications that run outside Garden, each on an integration — [`diagram-canvas`](custom-apps/diagram-canvas/) (petal-web-canvas), and [`petal-fps`](custom-apps/petal-fps/), [`petal-fantasy-nes`](custom-apps/petal-fantasy-nes/) and the [`side-scroller`](custom-apps/side-scroller/) platformer (petal-desktop-sdl) |

The apps under `games/`, `productivity/`, `dashboards/`, and `ui/` are Garden
panel apps: pure Petal, each with its own README and a `layout.ptl`;
`tools/run-example.ts <slug>` starts Garden on it. See [AUTHORING.md](AUTHORING.md) for how they are
built and tested. The exceptions are [`games/hopper/`](games/hopper/), which runs on the `petal-desktop-sdl`
integration via its own launch script, and the five console programs
(`2048`, `boids`, `minesweeper`, `terrain`, `tetris`), which print to stdout
and are deterministic: two runs print the same output.

## Console programs

Run any one with:

```bash
./tools/run-petal.ts run examples/console/<name>.ptl
```

`run-petal.ts` rebuilds the compiler if any Rust source is newer than the
binary, then forwards its arguments to `petal`. It is the recommended way to
run Petal locally. The console games run the same way, e.g.
`./tools/run-petal.ts run examples/games/tetris/tetris.ptl`.

Run the whole corpus with:

```bash
./tools/test-examples.ts          # add --full for complete output
```

This runs every `console/*.ptl` at both optimizer levels, checks the two
outputs match, and checks both against the frozen golden in
`test/example-golden/`. The vitest suite (`npm test`) also runs every console
example and asserts it exits cleanly.

| File | Description | Features |
|------|-------------|----------|
| `hello.ptl` | Hello world | `print` |
| `arithmetic.ptl` | Numeric operations | Variables, arithmetic, math builtins |
| `control_flow.ptl` | Conditionals and loops | `if`/`else`, `for`, `while`, logical operators |
| `for_expression.ptl` | For loops as mapping expressions | `x = for … do … end`, `continue`/`break`, nested maps |
| `fizzbuzz.ptl` | Classic FizzBuzz | Loops, conditionals, modulo |
| `functions.ptl` | Function declarations | Functions, recursion, implicit return |
| `named_arguments.ptl` | Named call arguments | `f(x, limit: 10)`, binding by parameter name |
| `default_parameters.ptl` | Default parameter values | `fn f(a, b = expr)`, evaluated on every call that omits the argument; skipping by name; defaults with overloads |
| `argless_lambdas.ptl` | Lambdas with no parameters | `fn … end` without `()` |
| `typed.ptl` | Optional type annotations | `let x: int`, param/return types, `str` alias, int→float promotion |
| `lists.ptl` | List operations | List literals, indexing, `push`, destructuring |
| `records.ptl` | Record manipulation | Record literals, field access, nested records |
| `enums.ptl` | Enum types | Enum variants, associated data, pattern matching |
| `classes.ptl` | Classes and methods | `class … end`, typed fields, `fn Class.method`, the built-in `Rect` |
| `pattern_matching.ptl` | Match expressions | Guards, list destructuring, nested patterns |
| `closures.ptl` | Closures and HOFs | Closures, lambdas, `map`, `filter`, `reduce` |
| `state.ptl` | Persistent state | `state` keyword, counters, accumulators |
| `mutable_cells.ptl` | Mutable slots and where they're needed | `var`/`set`, writes from callbacks, shared boxes, `state var` |
| `particles.ptl` | Multi-object simulation | Per-iteration keyed `state`, bounce physics |
| `state_machine.ptl` | Traffic light controller | Enums + state, tick-based transitions |
| `fibonacci.ptl` | Recursive and iterative Fibonacci | Recursion, iteration, string interpolation |
| `reactive_ui.ptl` | React-like component model | State, records, event-driven render |
| `game_of_life.ptl` | Conway's Game of Life | Nested loops, 2D lists, complex logic |
| `string_interp.ptl` | String interpolation | `"text {expr}"` syntax |
| `noise_field.ptl` | 2D Perlin noise | `noise`, `map_range` |
| `vec2_demo.ptl` | 2D vectors and physics | `vec2`, `normalize`, `limit`, operator overloads |
| `vec3_demo.ptl` | 3D vectors: a bouncing ball and a camera basis | `vec3`, `cross`, `dot`, `normalize`, `lerp`, `limit`, operator overloads |
| `color_gradient.ptl` | HSV + color interpolation | `hsv`, `color_lerp`, `lerp` |
| `map_range_demo.ptl` | Remapping values | `map_range`, `clamp` |
| `differentiation.ptl` | Gradient descent with dual numbers | `dual`, `value_of`, `deriv_of` |
| `imports.ptl` | Module imports | `import`, qualified/selective forms |
| `text_utils.ptl` | Library module imported by `imports.ptl` | `fn` exports (no standalone output) |

`text_utils.ptl` is not a standalone program. It is the import target for
`imports.ptl` and lives here so the golden corpus covers it.

Do not add new files to `console/` casually: every file there is
golden-tested, so a new one needs a matching entry in `test/example-golden/`
(run `tools/gen-example-golden.ts`).
