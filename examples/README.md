# Examples

Example programs demonstrating Petal language features. Run any example with:

```bash
./ts/bin/run-petal.ts run examples/<name>.ptl
```

`run-petal.ts` rebuilds the compiler if any Rust source is newer than the
binary, then forwards its arguments to `petal`. It's the recommended way to
test Petal locally.

Or run all examples with pass/fail reporting:

```bash
./ts/bin/test-examples.ts
```

## Programs

| File | Description | Features |
|------|-------------|----------|
| `hello.ptl` | Hello world | `print` |
| `arithmetic.ptl` | Numeric operations | Variables, arithmetic, math builtins |
| `control_flow.ptl` | Conditionals and loops | `if`/`else`, `for`, `while`, logical operators |
| `for_expression.ptl` | For loops as mapping expressions | `x = for … do … end`, `continue`/`break`, nested maps |
| `fizzbuzz.ptl` | Classic FizzBuzz | Loops, conditionals, modulo |
| `functions.ptl` | Function declarations | Functions, recursion, implicit return |
| `lists.ptl` | List operations | List literals, indexing, `push`, destructuring |
| `records.ptl` | Record manipulation | Record literals, field access, nested records |
| `enums.ptl` | Enum types | Enum variants, associated data, pattern matching |
| `pattern_matching.ptl` | Match expressions | Guards, list destructuring, nested patterns |
| `closures.ptl` | Closures and HOFs | Closures, lambdas, `map`, `filter`, `reduce` |
| `state.ptl` | Persistent state | `state` keyword, counters, accumulators |
| `particles.ptl` | Multi-object simulation | Per-iteration keyed `state`, bounce physics |
| `state_machine.ptl` | Traffic light controller | Enums + state, tick-based transitions |
| `fibonacci.ptl` | Recursive and iterative Fibonacci | Recursion, iteration, string interpolation |
| `reactive_ui.ptl` | React-like component model | State, records, event-driven render |
| `game_of_life.ptl` | Conway's Game of Life | Nested loops, 2D lists, complex logic |
| `string_interp.ptl` | String interpolation | `"text {expr}"` syntax |
| `noise_field.ptl` | 2D Perlin noise | `noise`, `map_range` |
| `vec2_demo.ptl` | 2D vectors and physics | `vec2`, `normalize`, `limit`, operator overloads |
| `color_gradient.ptl` | HSV + color interpolation | `hsv`, `color_lerp`, `lerp` |
| `map_range_demo.ptl` | Remapping values | `map_range`, `clamp` |
| `differentiation.ptl` | Gradient descent with dual numbers | `dual`, `value_of`, `deriv_of` |
| `imports.ptl` | Module imports | `import`, qualified/selective forms |
| `text_utils.ptl` | Library module imported by `imports.ptl` | `fn` exports (no standalone output) |
