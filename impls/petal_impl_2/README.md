# Petal Programming Language - Implementation 2

A working implementation of the Petal programming language in Rust, featuring dataflow-first semantics and inline state management.

## Quick Start

### Build

```bash
cargo build --release
```

### Run a Program

```bash
./target/release/petal examples/03_functions.ptl
```

### Start REPL

```bash
./target/release/petal repl
```

### Use the Test Script

```bash
# Run a file
./test.sh examples/01_hello.ptl

# Execute inline code
./test.sh "print(sqrt(16))"

# Start REPL
./test.sh
```

## Example Programs

The `examples/` directory contains 10 sample programs:

- `01_hello.ptl` - Hello world
- `02_arithmetic.ptl` - Basic arithmetic and type coercion
- `03_functions.ptl` - Functions, recursion, fibonacci
- `04_control_flow.ptl` - If/else conditionals
- `05_loops.ptl` - For loops with range
- `06_state.ptl` - Inline state management
- `07_state_in_loops.ptl` - Per-iteration persistent state
- `08_lists.ptl` - List operations and iteration
- `09_math.ptl` - Mathematical functions
- `10_animated_counter.ptl` - Complex state example

## Language Features

### Variables and Functions

```petal
let x = 10
let y = x * 2

fn factorial(n) {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}

print(factorial(5))  // 120
```

### Inline State

The `state` keyword creates persistent state within a function:

```petal
fn counter() {
    state count = 0
    count = count + 1
    count
}

counter()  // 1
counter()  // 2
counter()  // 3
```

### Control Flow

```petal
if x > 0 {
    print("positive")
} else if x < 0 {
    print("negative")
} else {
    print("zero")
}
```

### Lists

```petal
let numbers = [1, 2, 3, 4, 5]
print(numbers[0])  // 1
print(len(numbers))  // 5
```

### Built-in Functions

- `print(...)` - Print values
- `range(start, end)` - Create a list of integers
- `sqrt(x)` - Square root
- `sin(x)`, `cos(x)` - Trigonometry
- `floor(x)`, `ceil(x)` - Rounding
- `len(list)` - List length
- `random(min, max)` - Random number

## Architecture

- `src/lib.rs` - Public API exports
- `src/env.rs` - Environment (owns programs and stacks)
- `src/program.rs` - Program and function definitions
- `src/term.rs` - Term (expression nodes)
- `src/value.rs` - Runtime values
- `src/stack.rs` - Execution stack and frames
- `src/parse.rs` - Lexer and parser
- `src/eval.rs` - Interpreter/evaluator
- `src/error.rs` - Error types
- `src/bin/main.rs` - CLI binary

## Implementation Status

See [IMPLEMENTATION_REPORT.md](./IMPLEMENTATION_REPORT.md) for detailed status, pain points, and next steps.

**Working**: Variables, functions, arithmetic, comparisons, if/else, lists, builtins, recursion

**Partial**: Loops (variable binding), state persistence

**Not yet**: Live editing, projection, automatic differentiation, while loops, maps, break/continue

## Testing

Run all examples:
```bash
for f in examples/*.ptl; do
    echo "=== $f ==="
    ./target/release/petal "$f"
done
```

Test specific features:
```bash
./test.sh "print(2 + 2)"
./test.sh "fn double(x) { x * 2 }
double(21)"
```

## Syntax Reference

See [SYNTAX.md](./SYNTAX.md) for complete syntax specification.

## License

This implementation is part of the Petal language research project.
