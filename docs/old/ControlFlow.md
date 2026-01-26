# Control Flow in Petal

This document covers all control flow constructs in the Petal programming language, including conditionals, loops, and flow control keywords.

## If Statements

### Basic If-Else

```petal
if x > 5 {
    print("x is greater than 5")
} else if x < 5 {
    print("x is less than 5")
} else {
    print("x equals 5")
}
```

### If Expressions

In Petal, `if` statements can be used as expressions that return values:

```petal
// If expressions
let status = if x > 0 { "positive" } else { "non-positive" }

// More complex if expression
let category = if score >= 90 {
    "excellent"
} else if score >= 70 {
    "good"
} else if score >= 50 {
    "pass"
} else {
    "fail"
}
```

### Nested If Statements

```petal
if user.is_authenticated {
    if user.has_permission("admin") {
        show_admin_panel()
    } else {
        show_user_dashboard()
    }
} else {
    redirect_to_login()
}
```

## Loops

### For Loops with Range Function

Petal uses the `range()` function for numeric iteration instead of `..` syntax:

```petal
// Basic range loop
for i in range(0, 10) {
    print(i)  // Prints 0 through 9
}

// Range with step
for i in range(0, 100, 5) {
    print(i)  // Prints 0, 5, 10, ..., 95
}

// Counting down
for i in range(10, 0, -1) {
    print(i)  // Prints 10, 9, 8, ..., 1
}
```

### For-Each Loops

Iterate over collections directly:

```petal
// Iterate over array
let items = [1, 2, 3, 4, 5]
for item in items {
    print(item * 2)
}

// Iterate with index
for (index, value) in items @ enumerate() {
    print("Item at ${index}: ${value}")
}

// Iterate over object keys
let person = {name: "Alice", age: 30, city: "Paris"}
for key in person @ keys() {
    print("${key}: ${person[key]}")
}
```

### While Loops

Execute code while a condition is true:

```petal
let count = 0
while count < 10 {
    print(count)
    count += 1
}

// While with complex condition
let searching = true
let attempts = 0
while searching && attempts < max_attempts {
    let result = try_search()
    if result != null {
        searching = false
    }
    attempts += 1
}
```

### Loop (Infinite Loop)

The `loop` keyword creates an infinite loop that must be exited with `break`:

```petal
// Basic infinite loop
loop {
    let input = get_user_input()
    
    if input == "quit" {
        break
    }
    
    process_input(input)
}

// Game loop example
loop {
    update_physics()
    handle_input()
    render_frame()
    
    if game_over {
        break
    }
}
```

## Break and Continue

### Break

Exit a loop early:

```petal
// Find first matching item
for item in items {
    if item.matches(criteria) {
        print("Found: " + item)
        break
    }
}

// Break from nested loops
for row in matrix {
    for cell in row {
        if cell == target {
            found = true
            break  // Only breaks inner loop
        }
    }
    if found {
        break  // Break outer loop
    }
}
```

### Continue

Skip to the next iteration:

```petal
// Process only valid items
for item in items {
    if !item.is_valid() {
        continue  // Skip invalid items
    }
    
    process_item(item)
}

// Continue with complex logic
for i in range(0, 100) {
    if i % 2 == 0 {
        continue  // Skip even numbers
    }
    
    if i % 3 == 0 {
        continue  // Skip multiples of 3
    }
    
    print(i)  // Print odd numbers not divisible by 3
}
```

## Early Returns

Functions can return early to simplify control flow:

```petal
func validate_input(input) {
    if input == null {
        return Error("Input cannot be null")
    }
    
    if input.length == 0 {
        return Error("Input cannot be empty")
    }
    
    if input.length > 100 {
        return Error("Input too long")
    }
    
    // Input is valid
    return Ok(input)
}
```

## Pattern-Based Control Flow

Using pattern matching for control flow:

```petal
// Match as control flow
match command {
    "start" -> {
        initialize_game()
        start_main_loop()
    }
    "load" -> {
        let save_file = choose_save_file()
        load_game(save_file)
    }
    "quit" -> {
        save_settings()
        exit_application()
    }
    _ -> {
        print("Unknown command: " + command)
    }
}
```

## Control Flow with Dataflow

Combining control flow with the `@` operator:

```petal
// Conditional pipeline
let result = data
    @ filter(func(x) => x > 0)
    @ map(func(x) => if x > 100 { 100 } else { x })
    @ take_while(func(x) => x < threshold)
    @ collect()

// Early exit in pipeline
fn process_with_validation(data) {
    return data
        @ validate()
        @ when_error(func(err) => {
            log_error(err)
            return Error(err)  // Early exit
        })
        @ transform()
        @ save()
}
```

## Nested Control Structures

Complex control flow patterns:

```petal
fn process_matrix(matrix) {
    for row_index in range(0, matrix.height) {
        for col_index in range(0, matrix.width) {
            let cell = matrix[row_index][col_index]
            
            if cell == null {
                continue
            }
            
            match cell.type {
                :empty -> continue
                :wall -> draw_wall(row_index, col_index)
                :player -> {
                    if cell.health <= 0 {
                        game_over = true
                        break
                    }
                    update_player(cell)
                }
                :enemy -> {
                    while cell.is_alive() {
                        cell.update()
                        if cell.can_see_player() {
                            cell.attack()
                            break
                        }
                    }
                }
            }
        }
        
        if game_over {
            break
        }
    }
}
```

## Control Flow Best Practices

- **Use pattern matching** for complex branching logic
- **Leverage if expressions** to avoid temporary variables
- **Use descriptive conditions** - extract complex conditions into well-named functions
- **Avoid deep nesting** - consider extracting nested logic into separate functions
- **Use the appropriate loop construct** - `for` for known iterations, `while` for conditional loops, `loop` for event loops
