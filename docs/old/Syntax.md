# Petal Programming Language Syntax

This document provides a comprehensive reference for the Petal programming language syntax, focusing on dataflow-oriented programming and high expressivity.

## Table of Contents

1. [Design Philosophy](#design-philosophy)
2. [Basic Types and Literals](#basic-types-and-literals)
3. [Variables and Constants](#variables-and-constants)
4. [Functions](#functions)
5. [Data Structures](#data-structures)
6. [Pattern Matching](#pattern-matching)
7. [Dataflow Programming](#dataflow-programming)
8. [Error Handling](#error-handling)
9. [Classes and Structs](#classes-and-structs)
10. [Operators](#operators)
11. [Comments](#comments)

## Design Philosophy

Petal is designed to balance high expressivity with dataflow-oriented programming patterns. Key principles include:

- **Dataflow First**: The `@` operator enables clear dataflow programming that resembles visual dataflow diagrams
- **Optional Syntax**: Commas are optional in function calls and lists for cleaner syntax
- **No Mutability Keywords**: No `mut`, `const`, or mutability as part of the language
- **Pattern Matching**: Extensive pattern matching capabilities with `match`
- **Null Safety**: Uses `null` (not `None`) for empty values
- **Built-in State**: The `state` keyword provides retained data across function calls (see [State.md](State.md) for details)
- **Flexible Control Flow**: Rich control flow constructs including loops, conditionals, and pattern matching (see [ControlFlow.md](ControlFlow.md) for details)

## Basic Types and Literals

### Numeric Types
```petal
// Integers
let x = 42
let negative = -10
let hex = 0xFF
let binary = 0b1010

// Floating point
let pi = 3.14159
let scientific = 1.23e-4
```

### String Types
```petal
// String literals
let simple = "Hello, world!"
let multiline = """
    This is a
    multiline string
"""

// String interpolation
let name = "Alice"
let greeting = "Hello, ${name}!"
```

### Boolean and Null
```petal
let flag = true
let disabled = false
let empty = null

// Empty declaration has null value and type
let x;  // x is null
```

### Colors
```petal
let red = :FF0000
let blue = :0000FF
let rgba = :FF000080  // With alpha
let white = :FFFFFF
let black = :000000
```

## Variables and Constants

### Variable Declaration
```petal
let x = 10
let name = "Petal"

// Type annotations
let count: int = 100
let price: float = 99.99
let message: string = "Hello"
let active: bool = true
```

### Destructuring
```petal
// Array destructuring
let [x, y] = [10, 20]
let [first, ...rest] = [1, 2, 3, 4, 5]

// Object destructuring
let {name, age} = {name: "Alice", age: 30}
```

## Functions

### Basic Function Syntax
```petal
// Function definition with 'fn' keyword
fn greet(name) {
    print("Hello, " + name)
}

// Function with return type
fn add(a: int, b: int) -> int {
    return a + b
}

// Function declarations (no body, ends with semicolon)
fn calculate(x, y) -> float;
fn process_data(input) -> result;
fn helper_function();

// Optional commas in parameters
fn multiply(x y z) {
    return x * y * z
}

// Variadic functions
fn log(...args) {
    send_effect(:logs, args)
}

fn sum(...numbers) {
    let total = 0
    for num in numbers {
        total += num
    }
    return total
}
```

### Function Declarations vs Definitions

Petal supports both function declarations (interface only) and function definitions (with implementation):

```petal
// Function declarations - no body, end with semicolon
fn external_api_call(url: string, data: object) -> result;
fn math_sqrt(x: float) -> float;
fn process_file(path: string);

// Function definitions - have body with implementation
fn add(a: int, b: int) -> int {
    return a + b
}

fn greet(name: string) {
    print("Hello, " + name + "!")
}

// Return type annotation is optional for both
fn helper_function();
fn simple_task() {
    print("Task completed")
}
```

### Anonymous Functions
```petal
// Lambda expressions
let square = fn(x) => x * x
let cube = fn(x) => x ** 3

// Multi-line anonymous functions
let complex = fn(x, y) {
    let result = x * x + y * y
    return sqrt(result)
}
```

## Data Structures

### Arrays
```petal
// Optional commas in array literals
let numbers = [1 2 3 4 5]  // No commas required
let mixed = [1, "hello", true, 3.14]  // Commas optional

// Array operations
let first = numbers[0]
let last = numbers[-1]  // Negative indexing
numbers[1] = 10
```

### Objects/Maps
```petal
// Object literals
let person = {
    name: "Alice"
    age: 30
    email: "alice@example.com"
}

// Property access
let name = person.name
let age = person["age"]
```

### Structs
```petal
struct Point {
    x: float
    y: float
}

// Struct instantiation
let p1 = Point{x: 0.0, y: 0.0}
let p2 = Point{x: 3.0, y: 4.0}
```

### Enums
```petal
// Simple enum
enum Color {
    Red
    Green
    Blue
}

// Enum with data
enum Shape {
    Circle(radius: float)
    Rectangle(width: float, height: float)
    Triangle(a: float, b: float, c: float)
}

// Using enums
let red = Color::Red
let circle = Shape::Circle(radius: 5.0)
```

## Pattern Matching

### Match Expressions
```petal
match value {
    0 -> print("zero")
    1 -> print("one")
    42 -> print("the answer")
    _ -> print("something else")
}

// Match with destructuring
let point = {x: 10, y: 20}
match point {
    {x: 0, y: 0} -> print("origin")
    {x: 0, y} -> print("on y-axis at " + y)
    {x, y: 0} -> print("on x-axis at " + x)
    {x, y} -> print("at (" + x + ", " + y + ")")
}

// Match with guards
match number {
    n if n < 0 -> print("negative")
    n if n > 100 -> print("large")
    n -> print("normal: " + n)
}
```

### Enum Pattern Matching
```petal
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

## Dataflow Programming

### The @ Operator
The `@` operator is the cornerstone of Petal's dataflow programming model, allowing data to flow through transformations in a visual, pipeline-like manner.

```petal
// Basic dataflow pipeline
let result = [1, 2, 3, 4, 5]
    @ filter(func(x) => x % 2 == 0)
    @ map(func(x) => x * 2)
    @ sum()

// Complex data processing
fn process_data(data) {
    return data
        @ validate()
        @ clean()
        @ transform()
        @ analyze()
        @ save()
}
```

### Graphics Programming with Dataflow
```petal
// Graphics pipeline
fn render_frame(scene, camera, time) {
    return scene
        @ cull_objects(camera)
        @ sort_by_depth()
        @ map(fn(obj) => obj @ animate(time) @ render(camera))
        @ composite()
        @ apply_bloom()
        @ tone_map()
}

// Drawing operations
fn draw_scene() {
    return Canvas.new(800, 600)
        @ fill(:2A2A2A)
        @ draw_circle(x: 100, y: 100, radius: 50, fill: :FF0000)
        @ draw_rectangle(x: 200, y: 150, width: 100, height: 60, fill: :0000FF)
}
```

### Method Chaining with @
```petal
// Object method chaining
let canvas = Canvas.new(800, 600)
    @ set_background(:FFFFFF)
    @ set_stroke_color(:000000)
    @ set_stroke_width(2)
    @ draw_line(0, 0, 800, 600)
```

## Error Handling

### Result Type
```petal
enum Result<T, E> {
    Ok(T)
    Err(E)
}

// Function that might fail
fn divide(a: float, b: float) -> Result<float, string> {
    if b == 0.0 {
        return Result::Err("Division by zero")
    }
    return Result::Ok(a / b)
}

// Handling results with pattern matching
match divide(10.0, 2.0) {
    Ok(value) -> print("Result: " + value)
    Err(error) -> print("Error: " + error)
}
```

### Option Type
```petal
enum Option<T> {
    Some(T)
    None
}

// Chaining operations with dataflow
let result = Maybe::Just(10)
    @ flat_map(fn(x) => safe_divide(x, 2))
    @ map(fn(x) => x * 3)
    @ flat_map(fn(x) => safe_divide(x, 5))
```

## Classes and Structs

### Struct Declaration
```petal
struct Point {
    x: float
    y: float
}

struct Rectangle {
    top_left: Point
    width: float
    height: float
}
```

### Member Functions (No 'impl' keyword)
```petal
// Member functions declared outside struct
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

### Generic Structs
```petal
struct Container<T> {
    items: [T]
    capacity: int
}

fn Container.new<T>(capacity: int) -> Container<T> {
    return Container{
        items: []
        capacity: capacity
    }
}
```

## Operators

### Arithmetic Operators
```petal
let sum = a + b        // Addition
let diff = a - b       // Subtraction
let product = a * b    // Multiplication
let quotient = a / b   // Division
let remainder = a % b  // Modulo
let power = a ** b     // Exponentiation
```

### Comparison and Logical Operators
```petal
let equal = a == b          // Equal
let not_equal = a != b      // Not equal
let less = a < b            // Less than
let greater = a > b         // Greater than
let and_result = p && q     // Logical AND
let or_result = p || q      // Logical OR
let not_result = !p         // Logical NOT
```

### Assignment Operators
```petal
n += 5      // Add and assign
n -= 3      // Subtract and assign
n *= 2      // Multiply and assign
n /= 4      // Divide and assign
n %= 4      // Modulo and assign
n **= 3     // Power and assign
```

### The Dataflow Operator (@)
```petal
// The @ operator enables dataflow programming
let result = data
    @ transform()
    @ filter(predicate)
    @ map(transformation)
    @ collect()

// Can be used with any expression
let processed = input @ validate() @ sanitize() @ save()
```

## Comments

```petal
// Single-line comment

/*
 * Multi-line comment
 * spanning multiple lines
 */

/**
 * Documentation comment
 * with detailed information
 */
fn documented_function() {
    // Implementation
}
```

## Function Calls with Optional Commas

Commas are optional in function calls and collections:

```petal
// Both styles are valid
let result1 = add(1, 2, 3)      // With commas
let result2 = add(1 2 3)        // Without commas

// Also works with arrays
let numbers1 = [1, 2, 3, 4, 5]  // With commas
let numbers2 = [1 2 3 4 5]      // Without commas

```
