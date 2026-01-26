# Petal Programming Language Specification

Petal is a functional programming language designed for creative coding. It features dataflow-oriented programming, expression-based evaluation, built-in state management, and optional type annotations.

## Key Characteristics

- **Dataflow-first**: The `@` operator creates visual, pipeline-like programming patterns
- **Expression-oriented**: Control flow constructs return values
- **Optional syntax**: Commas are optional in function calls and collections
- **Immutable by default**: No explicit mutability keywords
- **Built-in state system**: The `state` keyword creates retained data (similar to React's `useState`)
- **Pattern matching**: Extensive pattern matching with `match` expressions
- **Multi-target**: Can run on an interpreter, GPU, or be transpiled

---

## Table of Contents

1. [Lexical Elements & Tokens](#1-lexical-elements--tokens)
2. [Data Types & Literals](#2-data-types--literals)
3. [Variables & Constants](#3-variables--constants)
4. [Expressions](#4-expressions)
5. [Functions](#5-functions)
6. [Control Flow](#6-control-flow)
7. [Pattern Matching](#7-pattern-matching)
8. [Dataflow Programming](#8-dataflow-programming)
9. [Structures & Types](#9-structures--types)
10. [State Management](#10-state-management)
11. [Built-in Functions & Standard Library](#11-built-in-functions--standard-library)
12. [Property Access](#12-property-access)
13. [Bytecode & Virtual Machine](#13-bytecode--virtual-machine)
14. [Program Structure](#14-program-structure)
15. [Language Features Summary](#15-language-features-summary)
16. [Programming Patterns](#16-programming-patterns)

---

## 1. Lexical Elements & Tokens

### 1.1 Keywords

```
fn       - Function declaration
let      - Variable declaration
return   - Function return
if       - Conditional statement
else     - Else branch
while    - While loop
for      - For loop
true     - Boolean true
false    - Boolean false
null     - Null/empty value
struct   - Struct type definition
enum     - Enum type definition
state    - State variable declaration
match    - Pattern matching expression
```

### 1.2 Operators

| Category | Operators |
|----------|-----------|
| Arithmetic | `+`, `-`, `*`, `/`, `%`, `**` |
| Comparison | `==`, `!=`, `<`, `>`, `<=`, `>=` |
| Logical | `&&`, `\|\|`, `!` |
| Assignment | `=`, `+=`, `-=`, `*=`, `/=`, `%=`, `**=` |
| Dataflow | `@` (pipe operator) |
| Other | `.` (property access), `::` (enum/scope resolution), `:` (object key), `->` (return type), `=>` (lambda), `?` (optional chaining) |

### 1.3 Delimiters

```
( )     - Parentheses
{ }     - Braces (blocks, objects)
[ ]     - Brackets (arrays)
,       - Comma (optional in most contexts)
;       - Semicolon (for function declarations)
:       - Colon (object properties, symbols)
::      - Double colon (enum/module access)
@       - At sign (dataflow operator)
```

### 1.4 Comments

```petal
// Single-line comment

/* Multi-line comment
   spanning lines */

/** Documentation comment
    with details */
```

---

## 2. Data Types & Literals

### 2.1 Primitive Types

**Integers**
```petal
42                  // Decimal integer
0xFF                // Hexadecimal
0b1010              // Binary
-5                  // Negative integer
```

**Floating Point**
```petal
3.14159             // Float literal
1.23e-4             // Scientific notation
-2.5                // Negative float
```

**Strings**
```petal
"hello"             // String literal
""                  // Empty string
"""
    Multiline
    string
"""                 // Multiline string
"Hello, ${name}!"   // String interpolation
```

**Booleans**
```petal
true                // Boolean true
false               // Boolean false
```

**Null**
```petal
null                // Null value
```

**Colors**
```petal
:FF0000             // Red (RGB hex)
:00FF00FF           // Green with alpha
:FFFFFF80           // White, semi-transparent
```

**Symbols**
```petal
:symbol_name        // Symbol literal
:error              // Used for tagging
:fast               // Enumeration-like values
```

### 2.2 Collection Types

**Arrays**
```petal
[]                  // Empty array
[1, 2, 3]           // Array with commas
[1 2 3]             // Array without commas (optional)
[1, "hello", true]  // Mixed types
[[1, 2], [3, 4]]    // Nested arrays
```

**Objects**
```petal
{}                  // Empty object
{x: 10, y: 20}      // Object with properties
{
    name: "Alice"
    age: 30
    active: true
}                   // Multiline object
{x: obj.x, y: obj.y}  // Computed properties
```

---

## 3. Variables & Constants

### 3.1 Variable Declaration

```petal
let x = 42
let name = "Alice"
let flag = true

// With type annotations
let count: int = 100
let price: float = 99.99
let message: string = "Hello"
let active: bool = true

// Destructuring
let [x, y] = [10, 20]
let [first, ...rest] = [1, 2, 3, 4, 5]
let {name, age} = {name: "Alice", age: 30}
```

### 3.2 Assignment

```petal
x = 100             // Reassignment
x += 5              // Add and assign
x -= 3              // Subtract and assign
x *= 2              // Multiply and assign
x /= 4              // Divide and assign
```

### 3.3 Empty Declaration

```petal
let x;              // Initialized to null
```

---

## 4. Expressions

### 4.1 Literal Expressions

Values evaluate to themselves:
```petal
42
3.14
"hello"
true
null
:symbol
```

### 4.2 Binary Expressions

```petal
// Arithmetic
a + b               // Addition
a - b               // Subtraction
a * b               // Multiplication
a / b               // Division
a % b               // Modulo
a ** b              // Exponentiation

// Comparison
a == b              // Equal
a != b              // Not equal
a < b               // Less than
a <= b              // Less than or equal
a > b               // Greater than
a >= b              // Greater than or equal

// Logical
a && b              // Logical AND
a || b              // Logical OR
```

### 4.3 Unary Expressions

```petal
-a                  // Negation
!flag               // Logical NOT
+a                  // Unary plus
```

### 4.4 Operator Precedence (Highest to Lowest)

1. Parentheses: `()`
2. Exponentiation: `**`
3. Unary: `!`, `-`, `+`
4. Multiplication/Division: `*`, `/`, `%`
5. Addition/Subtraction: `+`, `-`
6. Comparison: `<`, `<=`, `>`, `>=`
7. Equality: `==`, `!=`
8. Logical AND: `&&`
9. Logical OR: `||`
10. Dataflow: `@`
11. Assignment: `=`, `+=`, `-=`, etc.

---

## 5. Functions

### 5.1 Function Declaration

```petal
// Basic function
fn greet(name) {
    print("Hello, " + name)
}

// With parameters and return type
fn add(a: int, b: int) -> int {
    return a + b
}

// Optional commas in parameters
fn multiply(x y z) {
    return x * y * z
}

// Single-expression function
fn square(x) -> x * x

// Variadic functions
fn log(...args) {
    send_effect(:logs, args)
}

// Function declaration (interface only)
fn external_api_call(url: string, data: object) -> result;
```

### 5.2 Function Calls

```petal
// Basic call
print("hello")
add(1, 2)
max(x, y)

// Optional commas
add(1 2 3)
create_rect(100 200 :0000FF)

// Named parameters
draw_line(x1: 0, y1: 0, x2: 100, y2: 100)
create_window(width: 800, height: 600, title: "Game")

// Method calls
player.move(10, 5)
text.length()
```

### 5.3 Lambda Functions (Anonymous Functions)

```petal
// Simple lambda
let square = fn(x) => x * x
let add = fn(a, b) => a + b

// Multi-line lambda
let complex = fn(x, y) => {
    let intermediate = x * x + y * y
    return sqrt(intermediate)
}

// With capture
let multiplier = 10
let scale = fn(x) => x * multiplier

// In higher-order functions
numbers @ filter(fn(n) => n % 2 == 0)
items @ map(fn(item) => item * 2)
```

### 5.4 Closures & Capturing

Lambdas can capture variables from their enclosing scope:

```petal
fn create_adder(n) {
    return fn(x) => x + n  // Captures 'n'
}

let add_five = create_adder(5)
let result = add_five(3)  // 8
```

---

## 6. Control Flow

### 6.1 If/Else Statements

```petal
// Basic if/else
if x > 5 {
    print("x is greater than 5")
}
else if x < 5 {
    print("x is less than 5")
}
else {
    print("x equals 5")
}

// If as expression (returns value)
let status = if x > 0 { "positive" } else { "non-positive" }

// Nested if
if user.is_authenticated {
    if user.has_permission("admin") {
        show_admin_panel()
    }
}
```

### 6.2 For Loops

```petal
// Range-based loop
for i in range(0, 10) {
    print(i)  // 0 through 9
}

// Range with step
for i in range(0, 100, 5) {
    print(i)  // 0, 5, 10, ..., 95
}

// Iterate over collection
let items = [1, 2, 3, 4, 5]
for item in items {
    print(item * 2)
}

// Iterate with index
for (index, value) in items @ enumerate() {
    print("Item at ${index}: ${value}")
}

// For as expression (returns array)
let squares = for i in range(1, 6) {
    i * i
}
```

### 6.3 While Loops

```petal
let count = 0
while count < 10 {
    print(count)
    count += 1
}

// Complex condition
while searching && attempts < max_attempts {
    let result = try_search()
    if result != null {
        searching = false
    }
    attempts += 1
}
```

### 6.4 Infinite Loop

```petal
loop {
    let input = get_user_input()

    if input == "quit" {
        break
    }

    process_input(input)
}
```

### 6.5 Break & Continue

```petal
// Break from loop
for item in items {
    if item.matches(criteria) {
        print("Found: " + item)
        break
    }
}

// Continue to next iteration
for item in items {
    if !item.is_valid() {
        continue  // Skip invalid items
    }
    process_item(item)
}
```

### 6.6 Early Return

```petal
fn validate_input(input) {
    if input == null {
        return Error("Input cannot be null")
    }

    if input.length == 0 {
        return Error("Input cannot be empty")
    }

    return Ok(input)
}
```

---

## 7. Pattern Matching

### 7.1 Basic Match

```petal
match value {
    0 -> "zero"
    1 -> "one"
    2 -> "two"
    _ -> "many"
}
```

### 7.2 Match with Variables (Binding)

```petal
match number {
    n if n < 0 -> "negative"
    n if n > 100 -> "large"
    n -> "normal: " + n
}
```

### 7.3 Destructuring Patterns

**Array destructuring:**
```petal
match point {
    [0, 0] -> "origin"
    [x, 0] -> "on x-axis at " + x
    [0, y] -> "on y-axis at " + y
    [x, y] -> "at (" + x + ", " + y + ")"
}

// Rest patterns
match coordinates {
    [] -> "empty"
    [x] -> "single"
    [x, ...rest] -> "first: " + x + ", rest: " + rest.length
}
```

**Object destructuring:**
```petal
match user {
    {name: "admin", role: "administrator"} -> grant_full_access()
    {name, role: "user", active: true} -> grant_user_access(name)
    {role: "guest"} -> grant_guest_access()
    _ -> deny_access("unknown user type")
}
```

### 7.4 Enum Pattern Matching

```petal
enum Shape {
    Circle(radius: float)
    Rectangle(width: float, height: float)
    Triangle(a: float, b: float, c: float)
}

fn calculate_area(shape) {
    match shape {
        Circle(r) -> 3.14159 * r * r
        Rectangle(w, h) -> w * h
        Triangle(a, b, c) -> {
            let s = (a + b + c) / 2.0
            sqrt(s * (s - a) * (s - b) * (s - c))
        }
    }
}
```

### 7.5 Nested Pattern Matching

```petal
match event {
    {type: "click", position: {x, y}} -> handle_click(x, y)
    {type: "key", key: key_code} -> handle_key(key_code)
    _ -> {}
}
```

---

## 8. Dataflow Programming

### 8.1 The @ Operator

The `@` operator enables pipeline-style programming where data flows through transformations:

```petal
// Basic pipeline
let result = [1, 2, 3, 4, 5]
    @ filter(fn(x) => x % 2 == 0)
    @ map(fn(x) => x * 2)
    @ sum()

// Complex processing
fn process_data(data) {
    return data
        @ validate()
        @ clean()
        @ transform()
        @ analyze()
        @ save()
}
```

### 8.2 Object Updates with @

```petal
// Simple update
let updated_player = player @ { health: 100 }

// Multiple field updates
let new_game_state = game @ {
    score: game.score + 100
    lives: game.lives - 1
    level: 2
}

// Nested updates
let updated_user = user @ {
    profile: user.profile @ {
        name: "New Name"
        email: "new@example.com"
    }
}
```

### 8.3 Mixed Dataflow

```petal
let result = initial_state
    @ { score: 0 }
    @ handle_input()
    @ update_physics()
    @ { frame_count: initial_state.frame_count + 1 }
```

---

## 9. Structures & Types

### 9.1 Struct Definition

```petal
struct Point {
    x: float
    y: float
}

struct Player {
    name: string
    position: Point
    health: int
    inventory: [string]
}

// Generic struct
struct Container<T> {
    items: [T]
    capacity: int
}
```

### 9.2 Struct Instantiation

```petal
let p1 = Point{x: 0.0, y: 0.0}
let p2 = Point{x: 10.0, y: 20.0}

let player = Player{
    name: "Alice"
    position: p1
    health: 100
    inventory: []
}
```

### 9.3 Member Functions

```petal
// Define member function outside struct
fn Point.distance_to(self, other) -> float {
    let dx = other.x - self.x
    let dy = other.y - self.y
    return sqrt(dx * dx + dy * dy)
}

fn Rectangle.area(self) -> float {
    return self.width * self.height
}

// Usage
let p1 = Point{x: 0.0, y: 0.0}
let p2 = Point{x: 3.0, y: 4.0}
let distance = p1.distance_to(p2)
```

### 9.4 Enums

**Simple enum:**
```petal
enum Color {
    Red
    Green
    Blue
}

let red = Color::Red
```

**Enum with data:**
```petal
enum Shape {
    Circle(radius: float)
    Rectangle(width: float, height: float)
    Triangle(a: float, b: float, c: float)
}

let circle = Shape::Circle(radius: 5.0)
let rect = Shape::Rectangle(width: 10.0, height: 20.0)
```

**Generic enum:**
```petal
enum Result<T, E> {
    Ok(T)
    Err(E)
}

enum Option<T> {
    Some(T)
    None
}
```

### 9.5 Type Annotations (Optional)

```petal
let x: int = 42
let name: string = "Alice"
let count: float = 3.14
let flag: bool = true

fn add(a: int, b: int) -> int {
    return a + b
}
```

### 9.6 Generic Types

```petal
struct Container<T> {
    items: [T]
    capacity: int
}

enum Maybe<T> {
    Just(T)
    Nothing
}

fn create_container<T>(capacity: int) -> Container<T> {
    return Container{ items: [], capacity: capacity }
}
```

---

## 10. State Management

The `state` keyword creates retained data that persists across function calls, similar to React's `useState` hook but integrated into the language.

### 10.1 Basic State

```petal
fn counter() {
    state count = 0  // Retained across function calls

    count += 1
    if count > 100 {
        count = 0
    }

    return count
}
```

### 10.2 Complex State Structures

```petal
fn particle_system() {
    state particles = []
    state emitter = {
        position: [0.0, 0.0]
        rate: 10.0
        timer: 0.0
    }

    let dt = get_delta_time()

    emitter.timer += dt

    if emitter.timer > (1.0 / emitter.rate) {
        let new_particle = create_particle(emitter.position)
        particles @ push(new_particle)
        emitter.timer = 0.0
    }

    particles = particles
        @ map(fn(p) => p @ update_particle(dt))
        @ filter(fn(p) => p.life > 0.0)

    return particles
}
```

### 10.3 State in Control Flow

**State in loops:**
```petal
fn animated_grid(width, height) {
    let grid = []

    for y in range(0, height) {
        let row = []
        for x in range(0, width) {
            state cell_phase = random(0.0, 6.28)  // Each cell has its own state
            state cell_amplitude = random(0.5, 1.5)

            cell_phase += get_delta_time() * 2.0
            let value = sin(cell_phase) * cell_amplitude

            row @ push(value)
        }
        grid @ push(row)
    }

    return grid
}
```

**State in conditionals:**
```petal
fn adaptive_behavior(mode) {
    if mode == :fast {
        state fast_counter = 0
        fast_counter += 2
        return fast_counter
    }
    else {
        state slow_counter = 0
        slow_counter += 1
        return slow_counter
    }
}
```

### 10.4 Animation with State

```petal
fn smooth_transition(target_value) {
    state current_value = target_value
    state velocity = 0.0

    let spring_force = 0.1
    let damping = 0.8
    let dt = get_delta_time()

    let force = (target_value - current_value) * spring_force
    velocity = (velocity + force) * damping
    current_value += velocity * dt

    return current_value
}
```

### 10.5 State Machines

```petal
fn complex_animation() {
    state phase = :idle
    state time_in_phase = 0.0
    state animation_data = {
        position: [0.0, 0.0]
        rotation: 0.0
        scale: 1.0
    }

    let dt = get_delta_time()
    time_in_phase += dt

    match phase {
        :idle -> {
            animation_data.scale = 1.0 + sin(time_in_phase * 2.0) * 0.05
            if time_in_phase > 3.0 {
                phase = :moving
                time_in_phase = 0.0
            }
        }
        :moving -> {
            let progress = time_in_phase / 2.0
            animation_data.position[0] = easing_out_cubic(progress) * 200.0
            if progress >= 1.0 {
                phase = :idle
                time_in_phase = 0.0
            }
        }
    }

    return animation_data
}
```

---

## 11. Built-in Functions & Standard Library

### 11.1 I/O Functions

```petal
print(value)        // Print value to console
```

### 11.2 Collection Operations

These are typically available through dataflow chaining:

```petal
data @ filter(fn(x) => x > 0)       // Filter elements
data @ map(fn(x) => x * 2)          // Transform elements
data @ sum()                        // Sum array elements
data @ average()                    // Average of elements
data @ sort()                       // Sort array
data @ reverse()                    // Reverse array
data @ take(n)                      // Take first n elements
data @ drop(n)                      // Drop first n elements
data @ enumerate()                  // Iterate with index
data @ keys()                       // Get object keys
data @ values()                     // Get object values
data @ entries()                    // Get object entries
```

### 11.3 Math Functions

```petal
sqrt(x)             // Square root
sin(x)              // Sine (radians)
cos(x)              // Cosine
pow(base, exp)      // Power function
abs(x)              // Absolute value
max(a, b)           // Maximum
min(a, b)           // Minimum
floor(x)            // Floor
ceil(x)             // Ceiling
round(x)            // Round to nearest
lerp(a, b, t)       // Linear interpolation
clamp(x, min, max)  // Clamp value
random(min, max)    // Random number in range
```

### 11.4 String Functions

```petal
text.length()       // String length
text.trim()         // Trim whitespace
text.to_lowercase() // Convert to lowercase
text.to_uppercase() // Convert to uppercase
text.split(sep)     // Split string
text.replace(a, b)  // Replace substring
text.contains(s)    // Check if contains
text.starts_with(s) // Check prefix
text.ends_with(s)   // Check suffix
```

### 11.5 Array Methods

```petal
array.length        // Array length
array.push(item)    // Add to end
array.pop()         // Remove from end
array[0]            // Index access
array[-1]           // Negative indexing
```

---

## 12. Property Access

### 12.1 Dot Notation

```petal
let name = person.name
let length = text.length
let city = user.address.city  // Chained
```

### 12.2 Bracket Notation

```petal
let value = person["name"]
let first = numbers[0]
let last = numbers[-1]        // Negative indexing
let cell = matrix[row][col]   // Multi-dimensional
```

### 12.3 Optional Chaining

```petal
let city = user?.address?.city  // Safe access (returns null if any part is null)
```

---

## 13. Bytecode & Virtual Machine

The language compiles to bytecode executed by a stack-based VM.

### 13.1 Key Bytecode Operations

**Execution Control:**
- `op_unreachable` - Marks unreachable code
- `op_nope` - No operation
- `op_stop` - Halt execution
- `op_call` - Call function
- `op_return` - Return from function
- `op_call_host` - Call host function

**Data Movement:**
- `op_move` - Move value between slots
- `op_copy` - Copy value
- `op_reserve_slots` - Reserve stack space

**Constants:**
- `op_const_i16` - Load 16-bit signed integer
- `op_const_u16` - Load 16-bit unsigned integer
- `op_const_u16_sym` - Load symbol ID

**Arithmetic (32-bit integers):**
- `op_i32_add` - Addition
- `op_i32_sub` - Subtraction
- `op_i32_mult` - Multiplication
- `op_i32_div_s` - Signed division
- `op_i32_inc` - Increment by 1

**Comparison:**
- `op_i32_eq` - Equality check
- `op_i32_ne` - Not equal
- `op_i32_lt` - Less than
- `op_i32_gt` - Greater than
- `op_i32_le` - Less than or equal
- `op_i32_ge` - Greater than or equal

**Control Flow:**
- `op_jump` - Unconditional jump
- `op_jump_if_true` - Jump if true
- `op_jump_if_false` - Jump if false

### 13.2 VM Data Structures

- **Program Counter (pc):** Current execution position in bytecode
- **Stack Slots:** Flat array of register slots for values
- **Stack Top:** Index marking current frame boundary
- **Frame Header:** Packed with return address and previous frame size

### 13.3 Function Call Protocol

1. Reserve slots for new frame
2. Copy/move input arguments to new frame slots
3. Execute `op_call` to:
   - Save frame header at local:0
   - Increase stack_top
   - Jump to function address
4. Function executes and overwrites input slots with outputs
5. Execute `op_return` to:
   - Restore PC from frame header
   - Reduce stack_top
   - Continue execution at call site

---

## 14. Program Structure

### 14.1 AST Elements

- **Program:** Top-level structure containing blocks
- **Block:** Control-flow block with:
  - Ordered list of Terms
  - Local variable scope
  - Parent and nested blocks
- **Term:** Expression/statement node with:
  - Function reference
  - Input terms
  - Optional output
  - Nested block (for control flow)

### 14.2 Name Interning

All names are stored as `SymbolId` (unsigned integers) mapped to strings via `NameMap` in `GlobalState`. This enables efficient symbol handling across the program.

### 14.3 Source Structure

```
src/
  ├── parser/          - Lexer and parser implementation
  ├── program/         - AST and program structures
  ├── bytecode/        - Bytecode compilation
  ├── runtime/         - Virtual machine
  ├── globals/         - Global state management
  ├── utils/           - Helper utilities
  ├── variant/         - Value variant type
  └── host/            - Host API integration

ts/
  ├── bytecodeOps.ts   - Bytecode operation definitions
  ├── generateCpp.ts   - Code generation
  └── test/            - Test suite

samples/
  ├── milestones/      - Progressive feature examples
  ├── explorations_v*/  - Exploration examples
  └── test/            - Simple test programs
```

---

## 15. Language Features Summary

| Feature | Status | Notes |
|---------|--------|-------|
| Variables & Assignment | ✓ | Immutable by default |
| Functions | ✓ | Named and anonymous |
| Control Flow | ✓ | if/else, for, while, loop, break, continue |
| Pattern Matching | ✓ | match expressions with guards and destructuring |
| Data Types | ✓ | Primitives, arrays, objects, structs, enums |
| Type Annotations | ✓ | Optional type declarations |
| Lambdas/Closures | ✓ | First-class functions with capture |
| Dataflow (@) | ✓ | Pipeline operator for composition |
| State Management | ✓ | state keyword for retained data |
| String Interpolation | ✓ | ${variable} syntax |
| Generics | ✓ | Generic structs and enums with `<T>` |
| Optional Chaining | ✓ | `user?.address?.city` notation |
| Multiline Strings | ✓ | `""" """` syntax |
| Comments | ✓ | `//`, `/* */`, `/** */` |
| Optional Commas | ✓ | Syntax flexibility |

---

## 16. Programming Patterns

### 16.1 Functional Programming

```petal
// Function composition
fn compose(f, g) {
    return fn(x) => f(g(x))
}

// Higher-order functions
fn map(array, f) {
    return for item in array { f(item) }
}

// Recursive functions
fn factorial(n) {
    match n {
        0 -> 1
        1 -> 1
        n -> n * factorial(n - 1)
    }
}
```

### 16.2 Dataflow Pipelines

```petal
let result = raw_data
    @ validate()
    @ filter(fn(x) => x > 0)
    @ map(fn(x) => x * 2)
    @ sum()
```

### 16.3 State Machines

```petal
fn state_machine() {
    state current_state = :initial

    match current_state {
        :initial -> current_state = :running
        :running -> current_state = :done
        :done -> current_state = :initial
    }

    return current_state
}
```

### 16.4 Game Loop Pattern

```petal
fn game_update() {
    state game = initialize_game()
    state time = 0.0

    time += get_delta_time()

    return game
        @ handle_input()
        @ update_physics(time)
        @ render()
}
```

### 16.5 Error Handling

```petal
enum Result<T, E> {
    Ok(T)
    Err(E)
}

fn safe_divide(a, b) {
    if b == 0 {
        return Result::Err("Division by zero")
    }
    return Result::Ok(a / b)
}

// Usage with pattern matching
match safe_divide(10, 2) {
    Ok(result) -> print("Result: " + result)
    Err(msg) -> print("Error: " + msg)
}
```

### 16.6 Builder Pattern

```petal
fn create_config() {
    return {}
        @ { debug: false }
        @ { log_level: "info" }
        @ { max_connections: 100 }
}

// With conditional additions
fn create_config(options) {
    let config = { debug: false }

    if options.verbose {
        config = config @ { log_level: "debug" }
    }

    return config
}
```

---

## Appendix A: Reserved Words

The following words are reserved and cannot be used as identifiers:

```
fn, let, return, if, else, while, for, true, false, null,
struct, enum, state, match, loop, break, continue, in
```

## Appendix B: File Extension

Petal source files use the `.petal` extension.

## Appendix C: Encoding

Petal source files are expected to be encoded in UTF-8.
