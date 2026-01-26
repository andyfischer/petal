
# Expressions in Petal

Expressions are the fundamental building blocks of Petal programs. They represent computations that produce values and can be combined in various ways to create complex behaviors.

## Table of Contents

1. [Basic Expressions](#basic-expressions)
2. [Arithmetic and Logical Expressions](#arithmetic-and-logical-expressions)
3. [Function Call Expressions](#function-call-expressions)
4. [Control Flow Expressions](#control-flow-expressions)
5. [Pattern Matching Expressions](#pattern-matching-expressions)
6. [Dataflow Expressions](#dataflow-expressions)
7. [Lambda Expressions](#lambda-expressions)
8. [Collection Expressions](#collection-expressions)
9. [Property Access Expressions](#property-access-expressions)
10. [Object Update Expressions](#object-update-expressions)
11. [Operator Precedence](#operator-precedence)

## Basic Expressions

### Primitive Values

| example | description |
| ------- | ----------- |
| `42` | integer value |
| `3.14159` | float value |
| `0xFF` | hexadecimal value |
| `:something` | enum/symbol value |
| `"Hello, world!"` | string value |
| `true` | boolean true |
| `false` | boolean false |
| `null` | null/empty value |
| `:FF0000` | color literal (red) |
| `:FF000080` | color with alpha (transparent red) |

### Infix Expressions

| example | precedence level | description |
| ------- | ---------------- | ----------- |
| `a ** b` | 2 | exponentiation |
| `a * b` | 4 | multiplication |
| `a / b` | 4 | division |
| `a % b` | 4 | modulo/remainder |
| `a + b` | 5 | addition |
| `a - b` | 5 | subtraction |
| `a < b` | 6 | less than |
| `a <= b` | 6 | less than or equal |
| `a > b` | 6 | greater than |
| `a >= b` | 6 | greater than or equal |
| `a == b` | 7 | equality |
| `a != b` | 7 | inequality |
| `a and b` | 8 | logical AND |
| `a or b` | 9 | logical OR |
| `a += b` | 11 | addition assignment |
| `a -= b` | 11 | subtraction assignment |
| `a *= b` | 11 | multiplication assignment |
| `a /= b` | 11 | division assignment |

### Unary Expressions

| example | precedence level | description |
| ------- | ---------------- | ----------- |
| `not a` | 3 | logical NOT |
| `-a` | 3 | unary minus |
| `+a` | 3 | unary plus |

## Function Call Expressions

### Basic Function Calls

```petal
// Standard function call
let result = add(1, 2, 3)

// Optional commas
let same_result = add(1 2 3)

// Named parameters
let rect = create_rectangle(width: 100, height: 50, color: :FF0000)

// Mixed positional and named
let circle = create_circle(50, color: :0000FF, stroke: 2)
```

### Method Call Expressions

```petal
// Dot notation for method calls
let distance = point1.distance_to(point2)
let area = rectangle.area()

// Chained method calls
let processed = text
    .trim()
    .to_lowercase()
    .replace(" ", "_")
```

## Control Flow Expressions

### If Expressions

If statements in Petal can be used as expressions that return values:

```petal
// Simple if expression
let status = if x > 0 { "positive" } else { "non-positive" }

// Multi-branch if expression
let grade = if score >= 90 {
    "A"
} else if score >= 80 {
    "B"
} else if score >= 70 {
    "C"
} else {
    "F"
}

// If expression in assignment
let max_value = if use_limit { 100 } else { 1000 }

// If expression as function argument
print_message(if debug_mode { "Debug: " + message } else { message })
```

### For Expressions

For loops can also return values by collecting results:

```petal
// For expression creating a list
let squares = for i in range(1, 6) {
    i * i  // Returns [1, 4, 9, 16, 25]
}

// For expression with filtering
let even_squares = for i in range(1, 11) {
    if i % 2 == 0 {
        i * i
    }
    // Returns [4, 16, 36, 64, 100]
}

// For expression with complex computation
let processed_items = for item in items {
    item
        @ validate()
        @ transform()
        @ enhance()
}
```

### While Expressions

```petal
// While expression accumulating results
let fibonacci = {
    let results = [0, 1]
    while results.length < count {
        let next = results[-1] + results[-2]
        results.push(next)
    }
    results
}
```

## Pattern Matching Expressions

### Match Expressions

```petal
// Basic match expression
let description = match value {
    0 -> "zero"
    1 -> "one"
    2 -> "two"
    _ -> "many"
}

// Match with destructuring
let point_type = match point {
    {x: 0, y: 0} -> "origin"
    {x: 0, y} -> "y-axis"
    {x, y: 0} -> "x-axis"
    {x, y} -> "general"
}

// Match with guards
let category = match number {
    n if n < 0 -> "negative"
    n if n == 0 -> "zero"
    n if n > 100 -> "large"
    n -> "normal"
}

// Match with complex patterns
let area = match shape {
    Circle(radius) -> 3.14159 * radius * radius
    Rectangle(width, height) -> width * height
    Triangle(a, b, c) -> {
        let s = (a + b + c) / 2.0
        sqrt(s * (s - a) * (s - b) * (s - c))
    }
}
```

## Dataflow Expressions

### The @ Operator

The `@` operator enables pipeline-style programming using a series of expressions:

```petal
// Basic dataflow pipeline
let result = data
    @ filter(func(x) => x > 0)
    @ map(func(x) => x * 2)
    @ sum()

// Complex data processing
let processed = input
    @ validate()
    @ clean()
    @ transform()
    @ analyze()
    @ save()

// Dataflow with branching
let enhanced = image
    @ resize(800, 600)
    @ if brightness < 0.5 { _ @ brighten(0.2) } else { _ }
    @ apply_filter("sharpen")
    @ compress(0.8)
```

### Rename Operator

The `@` operator can also be used for renaming variables in a pipeline:

```petal
items = range(0, 10)
filter(@items, func(x) => x > 0)
map(@items, func(x) => x * 2)
items.sum()
```

The line `filter(@items, func(x) => x > 0)` is equivalent to:
```petal
items = filter(items, func(x) => x > 0)
```

## Lambda Expressions

### Anonymous Functions

```petal
// Simple lambda
let square = func(x) => x * x
let add = func(a, b) => a + b

// Multi-line lambda
let complex_calculation = func(x, y) => {
    let intermediate = x * x + y * y
    let result = sqrt(intermediate)
    return result * 2
}

// Lambda with capture
let multiplier = 5
let scale = func(x) => x * multiplier

// Lambda in higher-order functions
let filtered = numbers
    @ filter(func(n) => n % 2 == 0)
    @ map(func(n) => n * 3)
```

### Function Composition

```petal
// Compose functions using dataflow
let process = func(data) => data
    @ validate
    @ normalize
    @ analyze

// Partial application
let add_ten = func(x) => add(x, 10)
let multiply_by_two = func(x) => x * 2

let transform = func(x) => x @ add_ten @ multiply_by_two
```

## Collection Expressions

### Array Expressions

```petal
// Array literals
let numbers = [1, 2, 3, 4, 5]
let mixed = [1, "hello", true, 3.14]

// Array with optional commas
let spaced = [1 2 3 4 5]

// Array comprehensions using for expressions
let squares = for i in range(1, 6) { i * i }
let evens = for i in range(1, 11) { if i % 2 == 0 { i } }

// Nested arrays
let matrix = [
    [1, 2, 3],
    [4, 5, 6],
    [7, 8, 9]
]
```

### Object Expressions

```petal
// Object literals
let person = {
    name: "Alice",
    age: 30,
    email: "alice@example.com"
}

// Object with computed properties
let key = "dynamic_key"
let obj = {
    [key]: "dynamic_value",
    static_key: "static_value"
}

// Object with methods
let calculator = {
    value: 0,
    add: func(self, n) => self.value + n,
    multiply: func(self, n) => self.value * n
}
```

## Property Access Expressions

### Dot Notation

```petal
// Simple property access
let name = person.name
let length = text.length

// Chained property access
let city = user.address.city
let red = color.rgb.red
```

### Bracket Notation

```petal
// Dynamic property access
let property = "name"
let value = person[property]

// Array indexing
let first = numbers[0]
let last = numbers[-1]  // Negative indexing

// Computed property access
let column = matrix[row][col]
```

### Optional Chaining

```petal
// Safe property access (returns null if any part is null)
let city = user?.address?.city
let length = text?.length
```

## Object Update Expressions

Object update expressions provide a concise way to create new objects with modified fields while keeping the original object unchanged. This syntax supports immutable updates, a core principle in functional programming.

### Basic Object Updates

The `@` operator can be used to create updated copies of objects:

```petal
// Basic object update syntax
let updated_player = player @ { health: 100 }

// Update multiple fields
let new_game_state = game @ {
    score: game.score + 100
    lives: game.lives - 1
    level: 2
}

// Nested object updates
let updated_user = user @ {
    profile: user.profile @ {
        name: "New Name"
        email: "new@example.com"
    }
}
```

### Language Inspiration

This syntax is inspired by record update syntax from functional programming languages:

```petal
// Petal syntax
game @ { ship: updated_ship }

// Similar to F# record updates:
// { game with ship = updatedShip }

// Similar to Elm record updates:
// { game | ship = updatedShip }

// Similar to OCaml record updates:  
// { game with ship = updated_ship }
```

### Practical Examples

```petal
// Game state management
func handle_player_damage(game: GameState, damage: int) -> GameState {
    let current_health = game.player.health
    let new_health = Math.max(0, current_health - damage)
    
    return game @ {
        player: game.player @ { health: new_health }
        events: game.events @ push("player_damaged")
    }
}

// Configuration updates
func update_settings(config: Config, new_volume: float) -> Config {
    return config @ {
        audio: config.audio @ {
            volume: new_volume
            last_changed: current_time()
        }
    }
}

// Chaining updates with dataflow
let final_state = initial_state
    @ { score: 0 }
    @ handle_input()
    @ update_physics()
    @ { frame_count: initial_state.frame_count + 1 }
```

### Benefits of Immutable Updates

```petal
// ✅ Good: Immutable updates preserve original data
let original = { x: 10, y: 20 }
let updated = original @ { x: 30 }
// original.x is still 10, updated.x is 30

// ✅ Good: Safe concurrent access
let player1_state = shared_state @ { player_id: 1 }
let player2_state = shared_state @ { player_id: 2 }
// No risk of one player's changes affecting the other

// ✅ Good: Easy to track changes
let state_history = [
    initial_state,
    initial_state @ { score: 100 },
    initial_state @ { score: 100, level: 2 }
]
```

### Complex Update Patterns

```petal
// Conditional updates
let new_player = if player.health <= 0 {
    player @ { 
        health: 0
        status: "dead"
        position: respawn_point
    }
} else {
    player @ { last_action: current_time() }
}

// Updates based on existing values
let leveled_up_player = player @ {
    level: player.level + 1
    experience: player.experience - experience_needed
    skill_points: player.skill_points + 5
    stats: player.stats @ {
        max_health: player.stats.max_health + 10
        max_mana: player.stats.max_mana + 5
    }
}

// Functional updates with validation
func safe_update_position(entity: Entity, new_pos: [float, float]) -> Entity {
    let valid_position = clamp_to_bounds(new_pos)
    return entity @ {
        position: valid_position
        last_position: entity.position
        movement_history: entity.movement_history @ push(valid_position)
    }
}
```

### Relationship to Dataflow Operator

The `@` symbol serves dual purposes in Petal, creating a unified syntax for transformation:

```petal
// Dataflow: data flows through functions
let result = data
    @ filter(func(x) => x > 0)
    @ map(func(x) => x * 2)
    @ sum()

// Object update: object flows through an update
let updated = object @ { field: new_value }

// Combined usage: update then transform
let final_result = game_state
    @ { score: game_state.score + points }
    @ save_to_database()
    @ log_state_change()
```

Both uses represent the concept of "flowing data through a transformation," whether that transformation is a function call or an object update.

## Operator Precedence

Petal follows standard mathematical precedence rules:

1. **Parentheses**: `()`
2. **Exponentiation**: `**`
3. **Unary operators**: `!`, `-`, `+`
4. **Multiplication/Division**: `*`, `/`, `%`
5. **Addition/Subtraction**: `+`, `-`
6. **Comparison**: `<`, `<=`, `>`, `>=`
7. **Equality**: `==`, `!=`
8. **Logical AND**: `&&`
9. **Logical OR**: `||`
10. **Dataflow**: `@`
11. **Assignment**: `=`, `+=`, `-=`, etc.

### Examples of Precedence

```petal
// Parentheses override precedence
let result1 = 2 + 3 * 4      // 14 (multiplication first)
let result2 = (2 + 3) * 4    // 20 (parentheses first)

// Dataflow has low precedence
let processed = data @ filter(func(x) => x > 0) @ map(func(x) => x * 2)

// Mixing operators
let complex = a + b * c ** d > threshold && flag || backup_condition
```

## Best Practices

### Expression Clarity

```petal
// Good: Clear and readable
let is_adult = age >= 18
let full_name = first_name + " " + last_name

// Better: Use meaningful variable names
let can_vote = age >= voting_age
let display_name = given_name + " " + family_name
```

### Dataflow Usage

```petal
// Good: Use dataflow for transformation pipelines
let processed = raw_data
    @ validate()
    @ clean()
    @ transform()
    @ analyze()

// Avoid: Overly complex single expressions
let bad_example = data @ filter(func(x) => x > 0) @ map(func(x) => x * 2) @ reduce(func(a, b) => a + b, 0) @ format_result()
```

### Control Flow Expressions

```petal
// Good: Use if expressions for simple conditionals
let message = if error { "Error occurred" } else { "Success" }

// Good: Use match for complex branching
let response = match status_code {
    200 -> "OK"
    404 -> "Not Found"
    500 -> "Server Error"
    _ -> "Unknown Status"
}
```
