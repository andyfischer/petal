# Petal Language - Quick Start Guide

## Installation

```bash
cargo build --release
```

The binary is at `./target/release/petal`

## Your First Program

Create `hello.ptl`:
```petal
print("Hello, Petal!")
```

Run it:
```bash
./target/release/petal hello.ptl
```

Output:
```
Hello, Petal!
```

## Interactive REPL

```bash
./target/release/petal repl
> print(2 + 3)
5
> range(0, 5)
[0, 1, 2, 3, 4]
> exit
```

## Basic Examples

### Math
```petal
print(2 + 3 * 4)       # 14 (correct precedence)
print(20 / 4)          # 5
print(10 % 3)          # 1
```

### Comparisons
```petal
print(5 > 3)           # true
print(5 == 5)          # true
print(3 != 4)          # true
```

### Lists
```petal
print([1, 2, 3])       # [1, 2, 3]
print(len([1, 2, 3]))  # 3
print(range(1, 5))     # [1, 2, 3, 4]
```

### Strings
```petal
print("Hello " + "World")    # Hello World
print(len("Petal"))          # 5
```

### Conditional Logic
```petal
if 5 > 0 {
    print("yes")
} else {
    print("no")
}
```

### Type Conversions
```petal
print(to_string(42))   # "42"
print(to_int("100"))   # 100
print(to_float(5))     # 5.0
```

### Logical Operators
```petal
print(true && true)    # true
print(true || false)   # true
print(!false)          # true
```

### Variable Bindings
```petal
let x = 5
let y = 3
print(x + y)           # 8
```

### State Management
```petal
state counter = 0
print(counter)         # 0
```

### User-Defined Functions
```petal
fn add(x, y) {
    x + y
}

fn factorial(n) {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}

print(add(3, 4))         # 7
print(factorial(5))      # 120
```

## Running All Samples

```bash
./test_samples.sh
```

This runs all 12 example programs in `samples/` and shows their output.

## Language Features

| Feature | Status | Example |
|---------|--------|---------|
| Arithmetic | ✅ | `2 + 3 * 4` |
| Comparisons | ✅ | `5 > 3` |
| Logic | ✅ | `true && false` |
| If-Else | ✅ | `if x { ... } else { ... }` |
| Lists | ✅ | `[1, 2, 3]` |
| Strings | ✅ | `"hello" + " world"` |
| Type Conversion | ✅ | `to_string(42)` |
| Comments | ✅ | `# This is a comment` |
| Range | ✅ | `range(0, 10)` |
| Operators | ✅ | `+, -, *, /, %, ==, !=, <, >, &&, \|\|, !` |
| Built-in Functions | ✅ | `print, len, range, to_string, to_int, to_float, push, pop` |
| **Variable Binding** | ✅ | `let x = 5 print(x)` |
| **State** | ✅ | `state counter = 0` |
| **User Functions** | ✅ | `fn add(x,y) { x + y }` |
| **Recursion** | ✅ | `fn fib(n) { if n<=1 n else fib(n-1)+fib(n-2) }` |
| Loops | ⚠️ | Use `range()` instead |

## Common Patterns

### Print Multiple Values
```petal
print("Count: ")
print(42)
print(" Items")
```

### Generate a List
```petal
print(range(0, 10))  # [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
```

### Check Multiple Conditions
```petal
if (5 > 0) && (5 < 10) {
    print("5 is between 0 and 10")
}
```

### Nested Conditionals
```petal
if 5 > 0 {
    if 5 < 10 {
        print("5 is between 0 and 10")
    }
}
```

## Documentation

- **README.md** - Complete language reference and architecture
- **COMPLETION_SUMMARY.md** - Implementation status and feedback on docs

## Troubleshooting

**"Binary not found"**
- Run `cargo build --release` first

**"Unexpected token"**
- Check syntax against examples in `samples/`
- Ensure strings are quoted with `"`
- Parentheses required for function calls: `to_string(42)`, not `to_string 42`

**"Unknown function"**
- Built-in functions: print, len, range, to_string, to_int, to_float, push, pop
- User-defined functions not yet supported

**Script produces wrong output**
- Check operator precedence: `*` and `/` bind tighter than `+` and `-`
- Type conversions are needed for mixed int/float: `3 / 2.0` works, `3 / 2` gives integer division

## Next Steps

- Explore `samples/` directory for 12 complete working examples
- Read README.md for detailed language specification
- Check COMPLETION_SUMMARY.md for architecture and planned features

Happy coding in Petal! 🌸
