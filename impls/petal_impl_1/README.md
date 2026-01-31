# Petal Implementation v1

A Rust implementation of the Petal programming language.

## Overview

Petal is a dataflow-first language with inline state management. This implementation provides:

- **Lexer and Parser** - Converts source code to an AST-like term graph
- **Interpreter** - Evaluates programs with memoization
- **State Management** - First-class `state` keyword for persistent state
- **Heap with GC** - Garbage-collected heap for strings, lists, and maps
- **Built-in Functions** - Math, I/O, and collection operations

## Building

```bash
cargo build --release
```

## Usage

### Run a script

```bash
cargo run -- examples/hello.ptl
```

### REPL mode

```bash
cargo run
```

## Examples

See the `examples/` directory for sample programs:

- `hello.ptl` - Hello World
- `arithmetic.ptl` - Basic arithmetic operations
- `functions.ptl` - Functions and recursion (factorial, fibonacci)
- `control_flow.ptl` - If/else, for loops, while loops
- `lists.ptl` - List operations
- `maps.ptl` - Map (object) operations
- `math.ptl` - Math functions
- `state.ptl` - State management demonstration
- `fizzbuzz.ptl` - Classic FizzBuzz
- `calculator.ptl` - Simple calculator with functions
- `game_of_life.ptl` - Conway's Game of Life
- `particles.ptl` - Animation with persistent state

## Language Features

### State Management

```petal
fn counter() {
    state count = 0
    count = count + 1
    count
}
```

State variables persist across function invocations within the same execution stack.

### Expression-Oriented

Everything is an expression:

```petal
let sign = if x > 0 { "positive" } else { "negative" }
```

### First-Class Functions

```petal
fn apply_twice(f, x) {
    f(f(x))
}

fn double(n) { n * 2 }

print(apply_twice(double, 5))  // 20
```

## Architecture

The implementation follows the architecture outlined in the Petal docs:

- `env.rs` - Environment that owns programs and stacks
- `program.rs` - Program and Term representations
- `stack.rs` - Execution stack with state storage
- `value.rs` - Runtime value types
- `parse.rs` - Lexer and parser
- `eval.rs` - Tree-walking interpreter
- `heap.rs` - Garbage-collected heap

## Limitations

This is a v1 implementation focused on core functionality:

- No type checking (dynamic typing only)
- No live editing support yet
- No differentiation/backpropagation yet
- No projection/slicing yet
- Per-iteration state in nested loops uses shared state keys

## Future Work

- Type system with inference
- Live editing with state reconciliation
- Program projection and slicing
- Automatic differentiation
- WebAssembly compilation target
