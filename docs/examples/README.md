# Examples

Hypothetical Petal programs demonstrating language features. These are written against the current language design and may evolve as the syntax and runtime stabilize.

## Basics

| File | Feature |
|------|---------|
| [hello.ptl](hello.ptl) | Hello World |
| [arithmetic.ptl](arithmetic.ptl) | Numbers, operators, math builtins |
| [control_flow.ptl](control_flow.ptl) | If/else expressions, for/while loops |
| [functions.ptl](functions.ptl) | Declaration, recursion, `?` identifiers |
| [lists.ptl](lists.ptl) | Indexing, iteration, destructuring |
| [records.ptl](records.ptl) | Field access, nesting, pattern matching |
| [enums.ptl](enums.ptl) | Variants, associated data, Result/Option |
| [pattern_matching.ptl](pattern_matching.ptl) | Guards, nested destructuring, expression trees |
| [fizzbuzz.ptl](fizzbuzz.ptl) | Classic exercise |
| [calculator.ptl](calculator.ptl) | Enums as expression trees |

## State

| File | Feature |
|------|---------|
| [state.ptl](state.ptl) | `state` keyword, persistence across invocations, branch-scoped state |
| [particles.ptl](particles.ptl) | Per-iteration state for multiple independent objects |
| [state_machine.ptl](state_machine.ptl) | Traffic light with enum states and transition rules |
| [reactive_ui.ptl](reactive_ui.ptl) | React-like component model using inline state |

## Algorithms

| File | Feature |
|------|---------|
| [game_of_life.ptl](game_of_life.ptl) | Conway's Game of Life with 2D grids |

## Advanced (Petal-Unique Features)

These examples showcase capabilities built into Petal's runtime and exposed through the `Program` metaprogramming API (see [tech_outline/topics/Metaprogramming.md](../tech_outline/topics/Metaprogramming.md)). In a previous implementation these required Rust host code; here they are expressed entirely in Petal.

| File | Feature | Key API |
|------|---------|---------|
| [differentiation.ptl](differentiation.ptl) | Automatic differentiation through the dataflow graph | `grad`, `gradients` |
| [gradient_descent.ptl](gradient_descent.ptl) | Parameter optimization using backflow | `grad`, `optimize` |
| [live_editing.ptl](live_editing.ptl) | Modifying a running program while preserving state | `Program.parse`, `.run`, `.edit` |
| [projection.ptl](projection.ptl) | Program slicing -- what code influences a value? | `program.slice`, `.impact`, `.dynamic_slice` |
| [provenance.ptl](provenance.ptl) | Execution tracing and data lineage | `program.trace`, `trace.influences` |
| [metaprogramming.ptl](metaprogramming.ptl) | Self-inspection, code generation, transformation, composition | `Program.current`, `.build`, `.transform`, `.pipe` |

## Metaprogramming API Summary

`Program` is a first-class type in Petal. A `Program` value represents a parsed program graph that can be inspected, executed, edited, differentiated, sliced, and composed. The full API is documented in [tech_outline/topics/Metaprogramming.md](../tech_outline/topics/Metaprogramming.md). Key entry points:

### Creating Programs

| Method | Description |
|--------|-------------|
| `Program.parse(source)` | Parse source code into a Program |
| `Program.from_file(path)` | Load from a file |
| `Program.current()` | Handle to the currently running program |
| `Program.build(fn(b) { ... })` | Construct a program node-by-node |

### Execution

| Method | Description |
|--------|-------------|
| `program.run(args...)` | Run to completion |
| `program.create_stack()` | Create an independent execution context |
| `stack.step()` | Single-step execution |
| `stack.value(name)` | Read a variable's value mid-execution |
| `stack.set_breakpoint(term)` | Set a breakpoint |

### Introspection

| Method | Description |
|--------|-------------|
| `program.terms()` | All terms in the program graph |
| `program.blocks()` | All control flow blocks |
| `program.functions()` | All function definitions |
| `program.find(name)` | Find a term by name |
| `term.op` / `term.inputs` / `term.dependents` | Navigate the dataflow graph |

### Live Editing

| Method | Description |
|--------|-------------|
| `program.edit(old, new)` | Text replacement, returns Reconciliation |
| `program.edit_at(offset, len, text)` | Positional edit |
| `program.replace_source(source)` | Full source replacement |
| `program.edit_with_migration(old, new, fn)` | Edit with custom state migration |

### Differentiation

| Method | Description |
|--------|-------------|
| `grad(f)` | Returns the gradient function of f |
| `gradients(f, args...)` | Value + per-argument gradients |
| `program.backpropagate(stack, output, seed)` | Full backprop through execution |
| `program.symbolic_derivative(output, with_respect_to)` | New Program computing the derivative |

### Projection

| Method | Description |
|--------|-------------|
| `program.slice(target)` | Backward slice: what influences target? |
| `program.impact(source)` | Forward slice: what does source affect? |
| `program.dynamic_slice(stack, target)` | Execution-based slice |
| `projection.intersect(other)` / `.union` / `.subtract` | Projection algebra |
| `projection.extract()` | Extract slice as standalone Program |

### Provenance

| Method | Description |
|--------|-------------|
| `program.trace(args...)` | Run with full step-by-step recording |
| `trace.influences(name)` | What contributed to this variable? |
| `trace.path(from, to)` | Dataflow path between variables |
| `trace.contributed?(a, to: b)` | Did a contribute to b? |

### Transformation

| Method | Description |
|--------|-------------|
| `program.transform(fn(term) { ... })` | Apply a function to each term |
| `program.fold_constants()` | Evaluate compile-time-known expressions |
| `program.pipe(other)` | Output of this becomes input of other |
| `program.clone()` | Copy a program |
| `program.diff(other)` | Structural diff between two programs |
