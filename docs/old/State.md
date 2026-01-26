# State Management in Petal

The `state` keyword is one of Petal's unique features that creates retained data persisting across function calls. This enables animations, interactive elements, and stateful computations in a functional programming context.

## Overview

In Petal, retained state can be created with the `state` keyword. This creates a piece of data that is retained across multiple calls
to the program. Since programs are often used for interactive UIs, this retains data across frames/redraws.

Stateful values are separated and stored in a nested data structure that matches the program's control flow graph. In other words,
if a stateful function is called 3 times in a program, then it will have 3 separate objects for state. If there is a stateful for-loop,
then the state data will include a list, with one element for each loop iteration.

In other words, Petal's state is NOT the same as C's `static` keyword (which shares a single variable for all calls to the same function);

If you've used React.js then the system is very similar to the `useState` hook. But, Petal's system is built into the language, and it
works fine to create state inside if-blocks or for-loops. There's no "rule of hooks" here.

The state system really shines when using Petal for live coding. The program can be modified, and the runtime will try its best to 
match the old stateful tree into the new program. This usually works, and it means that the active state of the program is maintained
across a code change.

## Basic State Usage

The `state` keyword declares a variable that retains its value between function calls:

```petal
// Basic state usage
func animated_counter() {
    state count = 0  // Retained across function calls
    
    count += 1
    if count > 100 {
        count = 0  // Reset after reaching 100
    }
    
    return count
}
```

Each time `animated_counter()` is called, the `count` variable remembers its previous value.

## State Initialization

State variables are initialized on the first function call and retain their values on subsequent calls:

```petal
func timer_with_state() {
    state elapsed = 0.0  // Initialize to 0.0 on first call
    state last_time = current_time()
    
    let current = current_time()
    let delta = current - last_time
    elapsed += delta
    last_time = current
    
    return elapsed
}
```

## Complex State Structures

State can hold complex data structures that evolve over time:

```petal
func particle_system() {
    state particles = []
    state emitter = {
        position: [0.0, 0.0]
        rate: 10.0
        timer: 0.0
    }
    
    let dt = get_delta_time()
    
    // Update emitter timer
    emitter.timer += dt
    
    // Emit new particles
    if emitter.timer > (1.0 / emitter.rate) {
        let new_particle = create_particle(emitter.position)
        particles @ push(new_particle)
        emitter.timer = 0.0
    }
    
    // Update existing particles
    particles = particles
        @ map(func(p) => p @ update_particle(dt))
        @ filter(func(p) => p.life > 0.0)
    
    return particles
}
```

## State in Control Flow

### State in Loops

When state is declared inside loops, each iteration gets its own state instance:

```petal
func animated_grid(width, height) {
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

### State in Conditionals

State declared in conditional branches maintains separate instances:

```petal
func adaptive_behavior(mode) {
    if mode == :fast {
        state fast_counter = 0
        fast_counter += 2
        return fast_counter
    } else {
        state slow_counter = 0
        slow_counter += 1
        return slow_counter
    }
}
```

## Animation and Transitions

State is particularly useful for smooth animations and transitions:

```petal
func smooth_transition(target_value) {
    state current_value = target_value  // Initialize to target on first call
    state velocity = 0.0
    
    let spring_force = 0.1
    let damping = 0.8
    let dt = get_delta_time()
    
    let force = (target_value - current_value) * spring_force
    velocity = (velocity + force) * damping
    current_value += velocity * dt
    
    return current_value
}

// Easing function with state
func ease_to_target(target, easing_speed) {
    state current = target
    
    let dt = get_delta_time()
    current = lerp(current, target, easing_speed * dt)
    
    return current
}
```

## State Machines

State can be used to implement complex state machines:

```petal
func complex_animation() {
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
            animation_data.rotation = progress * 360.0
            
            if progress >= 1.0 {
                phase = :scaling
                time_in_phase = 0.0
            }
        }
        :scaling -> {
            let progress = time_in_phase / 1.5
            animation_data.scale = 1.0 + sin(progress * 3.14159) * 0.5
            
            if progress >= 1.0 {
                phase = :idle
                time_in_phase = 0.0
                animation_data = {position: [0.0, 0.0], rotation: 0.0, scale: 1.0}
            }
        }
    }
    
    return animation_data
}
```

## Interactive Elements

State enables reactive user interface elements:

```petal
func interactive_button(label, clicked) {
    state hover_amount = 0.0
    state press_amount = 0.0
    state was_pressed = false
    
    let dt = get_delta_time()
    let is_hovered = check_mouse_hover()
    
    // Animate hover state
    let hover_target = if is_hovered { 1.0 } else { 0.0 }
    hover_amount = lerp(hover_amount, hover_target, 5.0 * dt)
    
    // Animate press state
    let press_target = if clicked && !was_pressed { 1.0 } else { 0.0 }
    press_amount = lerp(press_amount, press_target, 10.0 * dt)
    was_pressed = clicked
    
    return {
        color: base_color @ brightness(1.0 + hover_amount * 0.2)
        scale: 1.0 - press_amount * 0.05
        text: label
    }
}
```

## Advanced State Patterns

### Caching and Memoization

State can be used to cache expensive computations:

```petal
func cached_computation(input) {
    state cache = Map([])
    
    match cache @ get(input) {
        Some(result) -> result
        None -> {
            let result = expensive_calculation(input)
            cache @ set(input, result)
            return result
        }
    }
}
```

### Resource Management

State can manage resources with lifecycle:

```petal
func resource_pool() {
    state resources = []
    state available = []
    state in_use = Map([])
    
    func acquire() {
        if available.length > 0 {
            return available @ pop()
        } else {
            let resource = create_resource()
            resources @ push(resource)
            return resource
        }
    }
    
    func release(resource) {
        in_use @ remove(resource.id)
        available @ push(resource)
    }
    
    return {acquire: acquire, release: release}
}
```

### Learning and Adaptation

State enables systems that learn and adapt over time:

```petal
func adaptive_filter() {
    state weights = [0.5, 0.3, 0.2]
    state learning_rate = 0.01
    state history = []
    
    func process(input) {
        history @ push(input)
        if history.length > weights.length {
            history = history[1:]  // Keep fixed window
        }
        
        // Apply filter
        let output = 0.0
        for i in range(0, min(history.length, weights.length)) {
            output += history[i] * weights[i]
        }
        
        return output
    }
    
    func train(target) {
        // Gradient descent to adjust weights
        for i in range(0, weights.length) {
            if i < history.length {
                let error = target - process(history)
                weights[i] += learning_rate * error * history[i]
            }
        }
    }
    
    return {process: process, train: train}
}
```

## Code Modification and State

One of Petal's unique features is that when code is modified, the interpreter makes a best effort to match the old state graph into the new code. This enables live coding scenarios where you can modify running programs without losing state.

```petal
func evolving_system() {
    state generation = 0
    state entities = []  // This array persists even if you modify the code
    
    generation += 1
    
    // You can modify this logic while the program runs
    // and the state will be preserved
    entities = entities @ evolve() @ mutate()
    
    return {generation: generation, population: entities.length}
}
```

## State vs. Regular Variables

Use state when you need:
- Data to persist across function calls
- Animation or transition values
- Interactive element states
- Caching or memoization
- Learning/adaptive behavior

Use regular variables when you need:
- Temporary calculations
- Function parameters
- Loop counters
- One-time computations

## Integration with Dataflow

State works seamlessly with Petal's dataflow programming model:

```petal
func stateful_pipeline(input) {
    state buffer = []
    state processed_count = 0
    
    return input
        @ validate()
        @ tap(func(item) => buffer @ push(item))
        @ transform()
        @ tap(func(_) => processed_count += 1)
        @ filter(func(item) => item.score > buffer @ average_score())
}
```

The combination of state and dataflow operators enables powerful patterns for interactive and adaptive systems.
