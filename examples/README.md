# Examples

Example programs demonstrating Petal language features. Run any example with:

```bash
rust/target/debug/petal run examples/<name>.ptl
```

Or run all examples with pass/fail reporting:

```bash
./bin/test-each.sh
```

## Programs

| File | Description | Features |
|------|-------------|----------|
| `hello.ptl` | Hello world | `print` |
| `arithmetic.ptl` | Numeric operations | Variables, arithmetic, math builtins |
| `control_flow.ptl` | Conditionals and loops | `if`/`else`, `for`, `while`, logical operators |
| `fizzbuzz.ptl` | Classic FizzBuzz | Loops, conditionals, modulo |
| `functions.ptl` | Function declarations | Functions, recursion, implicit return |
| `lists.ptl` | List operations | List literals, indexing, `push`, destructuring |
| `records.ptl` | Record manipulation | Record literals, field access, nested records |
| `enums.ptl` | Enum types | Enum variants, associated data, pattern matching |
| `pattern_matching.ptl` | Match expressions | Guards, list destructuring, nested patterns |
| `closures.ptl` | Closures and HOFs | Closures, lambdas, `map`, `filter`, `reduce` |
| `state.ptl` | Persistent state | `state` keyword, counters, accumulators |
| `state_machine.ptl` | Traffic light controller | Enums + state, tick-based transitions |
| `fibonacci.ptl` | Three Fibonacci implementations | Recursion, iteration, stateful generator |
| `calculator.ptl` | Multi-op calculator | Enums, pattern matching, error handling |
| `reactive_ui.ptl` | React-like component model | State, records, simulated rendering |
| `game_of_life.ptl` | Conway's Game of Life | Nested loops, 2D lists, complex logic |
