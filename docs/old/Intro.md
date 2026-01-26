# Petal Programming Language Documentation

Petal is a functional language designed for creative coding, featuring dataflow-oriented programming, built-in state management, and high expressivity.

## Key Features:

 - Dataflow-oriented. There are no mutable variables, instead there are names and expressions.
 - Imperative style control flow, with if/for/loop/etc.
 - Expression-oriented. Control flow blocks return values.
 - Functional-inspired with support for pattern matching with `match`.
 - Optional type declarations.
 - Has a type system that supports: optional values, generics, enums, structs.
 - Has a builtin retained-state system with the `state` keyword. Simiar to React's useState, but part of the langauge.
 - Multi-target, programs can run in an interpreter, on a GPU, or transpiled.
 - Batteries included for creative coding, with libraries for graphics and sound.
 - Supports differential programming and programming by direct manipulation.

## More Details

### Documentation Pages:

- [Syntax](Syntax.md) - Language syntax overview
- [Expressions](Expressions.md) - Expression system and evaluation
- [Control Flow](ControlFlow.md) - Control flow structures
- [State](State.md) - State management system
- [Effects](Effects.md) - Effect system and external interactions

Explore the comprehensive examples in the `samples/explorations/` directory:

- `basic_syntax.ca` - Fundamental syntax examples
- `dataflow_syntax.ca` - Dataflow programming patterns
- `graphics_programming.ca` - Graphics and rendering
- `input_handling.ca` - Event handling and user input
- `pattern_matching.ca` - Advanced pattern matching
- `control_flow.ca` - Control flow examples
- `data_structures.ca` - Working with data structures
- `functional_programming.ca` - Functional programming patterns
- `error_handling.ca` - Error handling strategies
- `game_programming.ca` - Game development patterns
- `state_management.ca` - State management examples
