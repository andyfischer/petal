# Petal Language Goals

This document describes the high-level design philosophy and goals of the Petal programming language.

---

## Core Philosophy

Petal is designed around a central insight: **programs are graphs of data transformations, and making this structure explicit enables powerful capabilities** that traditional imperative languages struggle to provide.

The language is built on four foundational pillars:

1. **Dataflow-First Semantics** - Every construct maps naturally to a dataflow graph
2. **First-Class State** - Inline state management integrated at the language level
3. **Projectional Views** - Multiple representations of the same program for different purposes
4. **Live Editing** - Modify source code while programs are running

---

## Goal 1: Dataflow-First Language

### The Core Idea

In Petal, all language constructs are designed to conform to a **dataflow graph** representation. Unlike imperative languages where mutation creates implicit dependencies and side effects obscure data relationships, Petal makes the flow of data through a program explicit and traceable.

This is not merely a stylistic choice—it is a foundational design decision that enables several advanced capabilities.

### Data Provenance and Tracing

For any computed result in a Petal program, you can **trace back the complete chain of expressions** that produced that result. This is known as *data provenance* or *data lineage* in database and systems research.

In mutable-state languages, this tracing is difficult or impossible because:
- Values can be overwritten, destroying history
- Side effects create invisible dependencies
- Aliasing means the same data can be modified through multiple paths

Petal's dataflow semantics preserve the computational history, making it possible to answer questions like:
- "What inputs influenced this output?"
- "What transformations were applied to produce this result?"
- "If I change this input, what outputs are affected?"

### Automatic Differentiation and Back-Propagation

A key capability enabled by dataflow semantics is **automatic differentiation (AD)**—the ability to compute derivatives of programs with respect to their inputs.

Petal aims to support **differentiable programming**, where:

1. A program executes and produces a result
2. The user specifies a target (how they want the result to change)
3. The language applies the **chain rule** through the active computation graph
4. The system suggests changes to source values that would move the result toward the target

This is analogous to back-propagation in neural networks, but applied to general programs. The dataflow graph serves as the computation graph through which gradients flow.

#### Handling Ambiguity

Back-propagation through a general program is inherently ambiguous—there may be multiple inputs that could be adjusted to achieve a desired output change. Petal acknowledges this by designing for **human-in-the-loop refinement**:

- The system identifies candidate parameters that influence the output
- The user selects which parameters they want to adjust
- The system computes the sensitivity (partial derivatives) for those parameters
- The user guides the optimization based on their intent

This collaborative approach recognizes that automatic optimization alone cannot capture user intent—the language provides the *capability* for back-propagation while leaving *decisions* to the human.

### Relationship to Existing Work

Petal's dataflow-first approach draws inspiration from:

- **Dataflow programming languages** (Lucid, Lustre, LabVIEW) - where computation is modeled as data flowing through operators
- **Differentiable programming frameworks** (JAX, PyTorch, Swift for TensorFlow) - which enable gradient computation through programs
- **Functional reactive programming** (Elm, React) - where data dependencies are explicit
- **Provenance systems** (database lineage tracking) - which trace data origins

---

## Goal 2: First-Class State Management

### The Challenge of State

Most applications are inherently stateful—games, interactive tools, simulations, and user interfaces all maintain state across time. Yet state is notoriously difficult to manage well:

- Global mutable state creates hidden dependencies
- State scattered across objects makes reasoning difficult
- Separating state from logic leads to boilerplate

### Inline State: The `state` Keyword

Petal introduces **inline state** as a first-class language construct, similar to React's `useState` hook but integrated directly into the language semantics.

```petal
fn counter() {
    state count = 0  // Persists across invocations
    count += 1
    return count
}
```

This design has several advantages:

1. **Locality** - State is declared where it is used, not in separate data structures
2. **Encapsulation** - Each function manages its own state without exposing internals
3. **Composability** - Stateful functions compose naturally with the dataflow model
4. **Traceability** - State changes are part of the dataflow graph

### State and Control Flow Unified

In Petal, control flow and state are **co-located and linked**. State declarations can appear inside conditionals and loops, creating per-branch or per-iteration state:

```petal
fn animated_grid(width, height) {
    for y in range(0, height) {
        for x in range(0, width) {
            state cell_phase = random(0.0, 6.28)  // Each cell has unique state
            // ...
        }
    }
}
```

This eliminates the need for external state containers while maintaining the benefits of structured state management.

### State in the Dataflow Graph

State is not separate from the dataflow model—it is part of it. State creates **temporal edges** in the dataflow graph, connecting the output of one invocation to the input of the next. This means:

- State changes are traceable like any other data transformation
- Back-propagation can flow through state across time steps
- The provenance of stateful computations remains intact


Another way to think about the state is to think about a program's 'control flow graph', which is a graph of all possible branches of all control flow primitives.

The state of a program is a **subset** of the control flow graph. Some branches are stateful and some are not.

---

## Goal 3: Projectional Views

### The Problem of Complexity

Real-world programs are complex. A production application may have thousands of variables, hundreds of functions, and intricate data dependencies. When working on such systems:

- Understanding requires holding too much context in mind
- Finding relevant code requires searching through irrelevant code
- AI assistants struggle with large context windows
- Debugging requires isolating relevant computation

### Projection as Partial Evaluation

Petal aims to support **projectional views**—the ability to derive simplified representations of a program by focusing on specific aspects.

The analogy is to **partial derivatives** in calculus. Given a function `f(x, y, z)`:
- The partial derivative `∂f/∂x` shows how `f` varies with `x`, treating `y` and `z` as constants
- This "projects" the full function onto the dimension of interest

Similarly, a Petal projection might:
- Show only the data flow from a specific input to a specific output
- Hide expressions that don't influence the selected data path
- Collapse or simplify intermediate computations

### Program Slicing

This capability is related to **program slicing** from software engineering research. A program slice is a subset of a program that affects or is affected by a particular variable at a particular point.

Petal's dataflow semantics make slicing natural:
- Forward slicing: "What does this input influence?"
- Backward slicing: "What influences this output?"
- Dynamic slicing: "What was active for this specific execution?"

### Scenario-Based Views

A powerful application of projection is **scenario-based viewing**. Given specific input data, the projection shows:

- Only the branches that were taken
- Only the loop iterations that executed
- Only the expressions that contributed to the result

This transforms a complex, general-purpose program into a simple, linear trace specific to one scenario. The benefits include:

- **Understanding** - See exactly what happened for a specific case
- **Debugging** - Isolate the relevant computation path
- **AI assistance** - Provide focused context without irrelevant code
- **Communication** - Share simplified views with others

### Bidirectional Projection

Projections in Petal are intended to be **bidirectional**. Edits made on a projection can be mapped back to the original program. This is related to:

- **Bidirectional transformations** in programming language research
- **Projectional editing** in language workbenches (JetBrains MPS, Intentional Software)
- **Lenses** in functional programming (which provide bidirectional access to data)

The workflow might look like:
1. Take a complex program
2. Project it to a simplified view for a specific scenario
3. Make edits on the projection
4. Map those edits back to the original program

This enables working at the right level of abstraction for the task at hand.

### Abstract Programming and Cross-Language Editing

A powerful consequence of projectional editing is the ability to work with **foreign programming languages** through Petal's projectional lens.

Consider a program written in a completely different language—JavaScript, C, Python, or any other. Rather than editing the foreign syntax directly, we can:

1. **Mount** the foreign program through a projectional adapter
2. **View** it as a Petal-style representation (dataflow graph, simplified logic, etc.)
3. **Edit** the projection using Petal's tools and semantics
4. **Propagate** those changes back to the foreign syntax

This is *abstract programming*—working at a higher level of abstraction that isn't tied to any particular surface syntax. The projection becomes a universal interface through which any language can be manipulated.

#### Benefits of Cross-Language Projection

- **Unified tooling** - Use the same editing environment regardless of target language
- **Semantic editing** - Work with program meaning rather than textual syntax
- **Language migration** - Gradually transform codebases between languages
- **Polyglot systems** - Reason about systems spanning multiple languages through a single lens

The key insight is that many languages share underlying computational structures (dataflow, control flow, state management) even when their surface syntax differs dramatically. Projections can expose these common structures, enabling cross-language comprehension and manipulation.

### Projection and Differentiation Together

The projection system synergizes with back-propagation:

1. Select an output you want to change
2. Project the program to show only what influences that output
3. The projection reveals the "active" computation path
4. Back-propagation computes sensitivities along that path
5. The user adjusts parameters in the simplified view

This combination makes complex optimization tractable by reducing it to a focused, understandable subproblem.

---

## Goal 4: Live Editing

### The Dream of Hot-Reloading

One of the most powerful capabilities for interactive development is **live editing**—the ability to modify source code while a program is running and see changes take effect immediately, without restarting.

Live editing transforms the development experience:
- **Immediate feedback** - See the effect of changes instantly
- **State preservation** - Don't lose application state when making changes
- **Exploration** - Experiment freely without restart penalties
- **Creative flow** - Stay in the zone without interruptions

### The State Reconciliation Problem

The fundamental challenge of live editing is **state reconciliation**: how do we translate the live state from the running program into the new source code?

When source code changes, the runtime faces difficult questions:
- If a variable is renamed, does the old state carry over?
- If a new state variable is added, what should its initial value be?
- If state structure changes, how do we migrate existing state?
- If control flow changes, which state is still valid?

Traditional languages punt on this problem—either you lose all state on reload, or you're limited to superficial changes that don't affect state shape.

### Inline State Enables Live Editing

Petal's **inline state system** provides a principled solution to state reconciliation. Because state is declared inline with explicit structure, the runtime can automatically handle state changes across code edits.

When source code is modified while a program is running:

1. **State additions** - New `state` declarations are initialized with their default values
2. **State removals** - Removed `state` declarations are garbage collected
3. **State modifications** - Changed `state` declarations can be migrated based on structural similarity

The key insight is that inline state creates a **correspondence** between source locations and runtime state. This mapping enables the runtime to intelligently reconcile old state with new code.

```petal
fn game_loop() {
    state player_x = 0.0
    state player_y = 0.0
    state score = 0        // Add this line while running
    // state lives = 3     // Remove this line while running

    // ... game logic
}
```

When the user adds the `score` line during live editing, it initializes to `0` without disturbing `player_x` or `player_y`. When `lives` is removed, its state is cleaned up.

### Control Flow and State Correspondence

The connection between control flow and state (from Goal 2) becomes crucial for live editing. Since state declarations are tied to their control flow context, the runtime can:

- Track which branch of a conditional each state belongs to
- Preserve loop-iteration state when loop bounds change
- Handle structural refactoring while maintaining state identity

```petal
fn animated_particles(count) {
    for i in range(0, count) {
        state x = random(0.0, 1.0)
        state y = random(0.0, 1.0)
        state velocity = random(0.1, 0.5)
        // ...
    }
}
```

If `count` changes from 10 to 15 during live editing, particles 0-9 keep their existing state, and particles 10-14 are initialized fresh.

### Live Editing and the Dataflow Graph

Live editing integrates with Petal's dataflow-first design. When code changes:

- The dataflow graph is incrementally updated
- Only affected nodes are recomputed
- State flows through the new graph structure
- Back-propagation paths update accordingly

This means live editing isn't just cosmetic—it preserves the full computational semantics while enabling real-time modification.

### Related Concepts

Petal's live editing draws inspiration from:

- **Hot module replacement** (Webpack, Vite) - But with semantic state preservation
- **Smalltalk environments** - Live objects with modifiable classes
- **REPLs and notebooks** - Interactive execution with state
- **Live coding** (Sonic Pi, Tidal Cycles) - Real-time creative coding
- **Edit-and-continue debugging** - But as a first-class language feature

---

## Design Implications

These goals have concrete implications for the language design:

### Syntax

- The `@` dataflow operator makes data flow visually explicit
- Expression-oriented design (everything returns a value)
- Immutability by default to preserve traceability

### Semantics

- No hidden side effects that would break provenance
- State changes are explicit and tracked
- Function calls are referentially transparent (same inputs = same outputs, modulo state)

### Tooling

- The compiler maintains dataflow graph metadata
- Tools can query provenance: "what influenced this value?"
- Projections can be computed statically or dynamically
- Differentiation can be automatic or guided

### Runtime

- The VM preserves enough information for tracing
- State management is built into the execution model
- Execution traces can be captured for analysis

---

## Summary

| Goal | Enables | Related Concepts |
|------|---------|------------------|
| Dataflow-First | Provenance, back-propagation, reasoning | Dataflow programming, automatic differentiation, FRP |
| First-Class State | Clean stateful applications, temporal dataflow | React hooks, state machines, temporal logic |
| Projectional Views | Complexity management, focused editing, cross-language programming | Program slicing, bidirectional transformations, lenses |
| Live Editing | Real-time code modification, state preservation | Hot reloading, Smalltalk, live coding environments |

Together, these goals position Petal as a language where programs are not opaque procedures but **transparent, queryable, and manipulable computation graphs**. This foundation enables capabilities—tracing, differentiation, projection, live editing—that are difficult or impossible to add to languages designed around imperative mutation.

---

## Further Reading

Concepts and research areas related to Petal's design goals:

- **Dataflow Programming**: Lucid, Lustre, synchronous languages
- **Automatic Differentiation**: Forward-mode AD, reverse-mode AD, dual numbers
- **Differentiable Programming**: JAX, PyTorch autograd, Swift for TensorFlow
- **Program Slicing**: Weiser's original work, dynamic slicing, thin slicing
- **Data Provenance**: Database lineage, why-provenance, how-provenance
- **Bidirectional Transformations**: Lenses, symmetric lenses, asymmetric lenses
- **Projectional Editing**: JetBrains MPS, Intentional Software, language workbenches
- **Cross-Language Transformation**: Language-agnostic ASTs, universal syntax trees
- **Live Coding**: Sonic Pi, Tidal Cycles, Extempore, live programming environments
- **Hot Reloading**: Smalltalk image persistence, Erlang hot code swapping, React Fast Refresh
- **Functional Reactive Programming**: Fran, Elm architecture, signal graphs
