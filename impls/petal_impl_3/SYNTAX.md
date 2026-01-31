# Petal Language Syntax

This document defines the syntax for the Petal programming language.

## Design Philosophy

Petal's syntax is designed to:
1. Make dataflow explicit through visual operators
2. Support inline state declarations naturally
3. Be expression-oriented (every construct returns a value)
4. Support live editing through stable term identification

## Syntax

### Comments

```petal
// Single line comment
/* Multi-line
   comment */
```

### Literals

```petal
42          // Integer
3.14159     // Float
true        // Boolean
false       // Boolean
"hello"     // String
nil         // Null value
```

### Variables

```petal
let x = 42              // Immutable binding
let mut x = 42          // Mutable binding
state counter = 0       // Persistent state (inline state)
```

**State Declaration**: The `state` keyword declares persistent state that persists across function invocations. This is Petal's first-class state management.

```petal
fn counter() {
    state count = 0
    count = count + 1
    count
}

counter()  // Returns 1
counter()  // Returns 2
counter()  // Returns 3
```

### The Dataflow Operator `@`

Petal uses the `@` operator to make dataflow explicit:

```petal
// Without @: traditional function call syntax
let result = add(2, 3)

// With @: explicit dataflow
let result = add @ [2, 3]

// @ can be chained for clearer data pipelines
let data = read_file @ "data.txt"
let clean = parse_csv @ data
let filtered = filter @ [clean, |row| row.price > 100]
let result = sum @ [filtered]
```

The `@` operator creates a dataflow edge in the program graph, making dependencies explicit.

### Expressions

Petal is expression-oriented - everything returns a value:

```petal
// If expression
let max = if x > y { x } else { y }

// Block expression
let value = {
    let temp = x * 2
    temp + 1
}  // Returns the last expression
```

### Functions

```petal
// Function definition
fn add(a, b) {
    a + b
}

// Function with state
fn accumulator() {
    state sum = 0
    state count = 0
    sum = sum + input
    count = count + 1
    sum / count  // Return average
}

// Function with named parameters
fn draw_circle @ center: Point, radius: Float, color: String {
    // ...
}

// Calling functions
let result = add @ [5, 3]
let avg = accumulator @ 10.0
```

### Control Flow

```petal
// If expression with blocks
let result = if condition {
    branch1
} else if other_condition {
    branch2
} else {
    branch3
}

// While loop
let mut i = 0
while i < 10 {
    print @ i
    i = i + 1
}

// For loop with range
for i in 0..10 {
    print @ i
}

// For loop with collection
for item in items {
    process @ item
}
```

### State in Control Flow

State declarations work naturally inside control flow, creating scope-specific state:

```petal
// Each iteration has separate state
for i in 0..width {
    for j in 0..height {
        state cell_value = random()  // Unique state per cell
        // ...
    }
}

// Branch-local state
if use_cache {
    state cache = {}
} else {
    state temp_data = []
}
```

### Lists and Maps

```petal
// List literal
let numbers = [1, 2, 3, 4, 5]

// Map literal
let person = {
    name: "Alice",
    age: 30,
    active: true
}

// Access
let first = numbers[0]
let name = person.name
person.age = 31
```

### Data Structures with State

```petal
// Stateful list accumulator
fn collect_results() {
    state results = []
    results = append @ [results, new_value]
    results
}

// Stateful map with caching
fn cached_compute(key, fn) {
    state cache = {}

    if !contains @ [cache, key] {
        cache[key] = fn(key)
    }

    cache[key]
}
```

### Pattern Matching

```petal
match value {
    0 => "zero",
    n if n > 0 => "positive",
    _ => "negative"
}
```

### Ranges

```petal
0..10      // Exclusive range (0 to 9)
0..=10     // Inclusive range (0 to 10)
x..y       // Variable range
```

### Lambdas

```petal
let add_one = |x| x + 1
let result = map @ [[1, 2, 3], add_one]  // [2, 3, 4]

// Multi-parameter lambda
let sum = |a, b, c| a + b + c

// With type annotations
let square = |x: Float| x * x
```

### Comments and Documentation

```petal
// Single line comment
/* Multi-line comment */

/// Documentation for a function
///
/// # Arguments
/// * `x` - The input value
///
/// # Returns
/// The transformed value
fn documented_function(x) { /* ... */ }
```

## Operator Precedence

From highest to lowest precedence:

1. Member access: `.` `[]`
2. Function call: `@`
3. Unary: `-` `!`
4. Multiplication: `*` `/` `%`
5. Addition: `+` `-`
6. Comparison: `<` `<=` `>` `>=`
7. Equality: `==` `!=`
8. Logical AND: `&&`
9. Logical OR: `||`
10. Assignment: `=`

## Examples

### Fibonacci with State

```petal
fn fib() {
    state a = 0
    state b = 1
    let result = a
    a = b
    b = result + b
    result
}

// Generate first 10 Fibonacci numbers
for i in 0..10 {
    print @ fib()
}
```

### Interactive Counter

```petal
fn main() {
    state count = 0

    let action = get_input()  // "inc", "dec", "show", "exit"

    match action {
        "inc" => count = count + 1,
        "dec" => count = count - 1,
        "show" => print @ count,
        "exit" => return nil
    }

    main()  // Loop recursively
}
```

### Data Processing Pipeline

```petal
fn process_data(filename) {
    // Each step is connected by explicit dataflow
    let raw = read_file @ filename
    let parsed = parse_json @ raw
    let filtered = filter @ [parsed, |item| item.active]
    let transformed = map @ [filtered, |item| {
        name: item.name,
        score: item.value * 2
    }]
    let sorted = sort_by @ [transformed, |a, b| a.score > b.score]
    sorted
}
```

### Gradient Descent Optimization

```petal
fn optimize(loss_fn, init_params, learning_rate, steps) {
    state params = init_params

    for i in 0..steps {
        let loss = loss_fn @ params
        let gradients = grad @ [loss_fn, params]

        // Update parameters
        params = map @ [zip @ [params, gradients], |(p, g)| {
            p - learning_rate * g
        }]

        if i % 100 == 0 {
            print @ {step: i, loss: loss}
        }
    }

    params
}
```

This syntax supports all four of Petal's design goals:
1. **Dataflow-first**: The `@` operator makes dataflow explicit
2. **First-class state**: The `state` keyword integrates state management
3. **Projectional views**: The graph structure enables slicing and projection
4. **Live editing**: Expression-oriented design and stable term IDs enable hot-reloading
