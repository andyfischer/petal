# Petal Programming Language - Rust Implementation

A Rust implementation of the Petal programming language, featuring a lexer, parser, and tree-walking interpreter.

## Building

```bash
cargo build --release
```

## Running

### Run a file
```bash
./target/release/petal path/to/program.petal
```

### Interactive REPL
```bash
./target/release/petal
```

## Features Implemented

- **Data Types**: Integers, floats, strings, booleans, null, symbols, arrays, objects
- **Variables**: `let` declarations with optional initialization
- **Operators**: Arithmetic (`+`, `-`, `*`, `/`, `%`, `**`), comparison, logical
- **Control Flow**: `if`/`else`, `while`, `for`, `loop`, `break`, `continue`
- **Functions**: Named functions, single-expression functions, recursion
- **Lambdas**: Anonymous functions with closure support
- **Pattern Matching**: `match` expressions with guards, literal, and object patterns
- **Dataflow**: The `@` operator for object updates and function pipelines
- **Symbols**: Lightweight identifiers (`:symbol_name`)
- **String Interpolation**: `"Hello, ${name}!"`
- **Built-in Functions**: `print`, `println`, math functions, array operations

## Sample Programs

The `samples/` directory contains example programs demonstrating various language features:

| Sample | Description |
|--------|-------------|
| `01_hello_world.petal` | Basic printing |
| `02_variables.petal` | Variables and types |
| `03_arithmetic.petal` | Math operations |
| `04_control_flow.petal` | If/else, loops, break/continue |
| `05_functions.petal` | Functions, recursion, closures |
| `06_collections.petal` | Arrays and objects |
| `07_pattern_matching.petal` | Match expressions |
| `08_lambdas.petal` | Lambdas and functional programming |
| `09_dataflow.petal` | The `@` operator |
| `10_strings.petal` | String operations |
| `11_algorithms.petal` | Classic algorithms |
| `12_symbols_and_enums.petal` | Symbols and enum-like patterns |

Run all samples:
```bash
for f in samples/*.petal; do echo "=== $f ===" && ./target/release/petal "$f" && echo; done
```

## Built-in Functions

### I/O
- `print(value...)` - Print without newline
- `println(value...)` - Print with newline

### Math
- `sqrt(x)`, `sin(x)`, `cos(x)` - Trigonometry
- `abs(x)`, `floor(x)`, `ceil(x)`, `round(x)` - Rounding
- `min(a, b)`, `max(a, b)`, `pow(base, exp)` - Arithmetic

### Collections
- `len(collection)` - Length of array, string, or object
- `push(array, item)` - Add item to array (returns new array)
- `pop(array)` - Remove last item (returns new array)
- `filter(array, fn)` - Filter array by predicate
- `map(array, fn)` - Transform array elements
- `reduce(array, fn, initial)` - Reduce array to single value
- `sum(array)` - Sum of numeric array

### Type Conversion
- `type_of(value)` - Get type name as string
- `to_string(value)` - Convert to string
- `to_int(value)` - Convert to integer
- `to_float(value)` - Convert to float

### Iteration
- `range(start, end)` - Create integer range [start, end)
- `range(start, end, step)` - Create integer range with step

## Example Program

```petal
// Fibonacci sequence using recursion
fn fibonacci(n) {
    if n <= 1 {
        return n
    }
    return fibonacci(n - 1) + fibonacci(n - 2)
}

// Print first 10 Fibonacci numbers
println("Fibonacci sequence:")
for i in range(0, 10) {
    print(fibonacci(i))
    print(" ")
}
println("")

// Using lambdas and dataflow
let numbers = [1, 2, 3, 4, 5]
let result = numbers
    @ filter(fn(x) => x % 2 == 0)
    @ map(fn(x) => x * x)
    @ sum()

println("Sum of squares of evens: " + result)
```

## Project Structure

```
rust-impl/
├── Cargo.toml          # Rust project configuration
├── src/
│   ├── main.rs         # Entry point and REPL
│   ├── token.rs        # Token definitions
│   ├── lexer.rs        # Lexical analyzer
│   ├── ast.rs          # Abstract syntax tree
│   ├── parser.rs       # Parser
│   └── interpreter.rs  # Tree-walking interpreter
└── samples/            # Example programs
```

## License

See the main project LICENSE file.
