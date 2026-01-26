# Petal Game and Graphics Examples (Version 3)

This directory contains comprehensive examples of Petal programming, showcasing the language's capabilities for game development and graphics programming using the latest syntax.

## Featured Examples

### 🚀 **asteroids.ca** - Classic Asteroids Game
A complete implementation of the classic space shooter game featuring:
- **Physics-based movement** with thrust and rotation
- **Asteroid splitting mechanics** - large asteroids break into smaller ones
- **Particle systems** for explosions and thrust effects
- **Collision detection** between ships, bullets, and asteroids
- **Score system** with lives and level progression
- **Wrap-around screen boundaries** for authentic gameplay
- **Audio integration** for sound effects

**Key Petal Features Demonstrated:**
- State management with the `state` keyword
- Dataflow programming with `@` operator
- Pattern matching with `match` statements using `->` syntax
- Function definitions with `func` keyword
- Collection operations and filtering
- Object destructuring and updates

### 🧩 **tetris.ca** - Classic Tetris Implementation
A full-featured Tetris game including:
- **Seven piece types** (I, O, T, S, Z, J, L) with proper rotations
- **Line clearing mechanics** with scoring for single, double, triple, and Tetris
- **Ghost piece preview** showing drop position
- **Next piece preview** system
- **Progressive difficulty** with increasing fall speed
- **Pause functionality** and game over detection
- **Hold and hard drop** controls

**Key Petal Features Demonstrated:**
- Complex state management for game board
- 2D array manipulation and processing
- Control flow expressions (`if` expressions returning values)
- Comprehensive input handling
- Modular function design
- Animation and timing systems

### 🏃 **side_scroller.ca** - Platformer Adventure
A side-scrolling platformer game featuring:
- **Physics-based player movement** with gravity and jumping
- **Tile-based level system** with platforms, spikes, and goals
- **Enemy AI** with different behavior patterns (walker, flying)
- **Collectible items** (coins, gems, power-ups) with scoring
- **Parallax scrolling background** with multiple layers
- **Particle effects** for collections and explosions
- **Camera system** that smoothly follows the player
- **Collision detection** for platforms, enemies, and hazards

**Key Petal Features Demonstrated:**
- Enum definitions and pattern matching
- Complex object structures and updates
- Real-time collision detection algorithms
- Smooth camera interpolation
- Particle system implementation
- Level data representation

### 🎨 **graphics_demo.ca** - Interactive Graphics Showcase
A comprehensive demonstration of graphics capabilities including:
- **Six different demo modes** showcasing various techniques:
  1. **Particle Demo** - Animated shapes and particle systems
  2. **Geometric Shapes** - Rotating geometric patterns
  3. **Color Gradients** - Animated color transitions and HSV manipulation
  4. **Fractal Trees** - Recursive tree generation with configurable depth
  5. **Wave Patterns** - Sine wave animations and interference patterns
  6. **Interactive Canvas** - Mouse-responsive grid effects

**Advanced Graphics Features:**
- **HSV to RGB color conversion** for smooth color transitions
- **Fractal recursion** with configurable depth
- **Mouse interaction** and trail effects
- **Parallax background rendering**
- **Transform matrix operations** (push/pop, translate, rotate, scale)
- **Multiple render layers** and alpha blending
- **Real-time animation** with time-based interpolation

## Latest Petal Syntax Features

All examples use the updated Petal syntax and follow functional programming principles:

### Function Definitions
```petal
// Functions use 'func' keyword
func create_player(spawn_point: [float, float]) -> Player {
    return Player {
        position: spawn_point
        velocity: [0.0, 0.0]
        // ...
    }
}
```

### No Mutability Keywords
```petal
// Petal has no 'mut' or mutability markers
// Variables are reassigned, not mutated
let game_state = initial_state
game_state = game_state @ update_function()  // Reassignment
```

### Pattern Matching
```petal
// Match expressions use '->' for patterns
match shape.type {
    ShapeType::Circle -> render_circle(shape)
    ShapeType::Square -> render_square(shape)
    ShapeType::Triangle -> render_triangle(shape)
    _ -> render_default(shape)
}
```

### Control Flow Expressions
```petal
// If and for statements can return values
let status = if player.health > 0 {
    "alive"
} else {
    "dead"
}

let squares = for i in range(1, 6) {
    i * i  // Returns [1, 4, 9, 16, 25]
}
```

### Dataflow Programming
```petal
// Pipeline operations with @ operator
let processed_data = input
    @ filter(func(x) => x > 0)
    @ map(func(x) => x * 2)
    @ collect()
```

### Lambda Expressions
```petal
// Anonymous functions with func keyword
let transform = func(x) => x * 2 + 1
let filtered = items @ filter(func(item) => item.active)
```

## Running the Examples

To run any of these examples:

1. **Build the Petal runtime** (if not already done):
   ```bash
   make
   ```

2. **Run an example**:
   ```bash
   dist/cli/main samples/explorations_v3/asteroids.ca
   dist/cli/main samples/explorations_v3/tetris.ca
   dist/cli/main samples/explorations_v3/side_scroller.ca
   dist/cli/main samples/explorations_v3/graphics_demo.ca
   ```

## Controls

### Asteroids
- **WASD** or **Arrow Keys**: Rotate and thrust
- **Space**: Shoot
- **R**: Restart (when game over)

### Tetris
- **A/D** or **←/→**: Move piece left/right
- **S** or **↓**: Soft drop
- **W/Space** or **↑**: Rotate piece
- **Enter**: Hard drop
- **P**: Pause/unpause
- **R**: Restart (when game over)

### Side Scroller
- **WASD** or **Arrow Keys**: Move and jump
- **R**: Restart (when game over or level complete)

### Graphics Demo
- **1-6**: Switch between demo modes
- **↑/↓**: Adjust fractal depth (in fractal mode)
- **←/→**: Adjust animation speed
- **Mouse**: Interactive effects (click to create particles)

## Architecture Highlights

These examples demonstrate several key architectural patterns in Petal:

### State Management
```petal
// Persistent state across function calls
func main() {
    state game = create_initial_game()
    
    let dt = get_delta_time()
    game = game @ handle_input(dt) @ update_game(dt)
    render_game(game)
}
```

### Functional Updates
```petal
// Immutable updates using object spread
func update_player(game: GameState, dt: float) -> GameState {
    let updated_player = game.player @ {
        position: new_position
        velocity: new_velocity
    }
    
    return game @ { player: updated_player }
}
```

### Pipeline Processing
```petal
// Data flows through transformation pipelines
let updated_enemies = game.enemies
    @ map(func(enemy) => update_enemy(enemy, dt))
    @ filter(func(enemy) => enemy.health > 0)
```

### Pattern-Driven Logic
```petal
// Complex behavior via pattern matching
match enemy.type {
    EnemyType::Walker -> update_walking_behavior(enemy, dt)
    EnemyType::Flying -> update_flying_behavior(enemy, dt)
    EnemyType::Jumping -> update_jumping_behavior(enemy, dt)
}
```

## Learning Path

1. **Start with graphics_demo.ca** to understand basic rendering and animation
2. **Move to side_scroller.ca** to see physics and collision detection
3. **Study tetris.ca** for complex state management and game logic
4. **Explore asteroids.ca** for complete game architecture and particle systems

Each example builds upon concepts from the previous ones while introducing new techniques and patterns specific to different types of applications.

## Extension Ideas

These examples provide excellent starting points for further development:

- **Add new enemy types** to the side scroller
- **Implement power-ups** in Asteroids
- **Create new Tetris game modes** (time attack, puzzle mode)
- **Add new graphics effects** to the demo (shaders, lighting)
- **Combine elements** from different examples to create hybrid games

The modular architecture and functional programming style make these extensions straightforward to implement while maintaining clean, readable code.