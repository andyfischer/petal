# Petal Language Goals

This document describes the high-level design philosophy and goals of the Petal programming language. While the [language specification](./PETAL_SPEC.md) defines syntax and semantics, this document explores the *why* behind those decisions.

---

## Core Philosophy

Petal is designed around a central insight: **programs are graphs of data transformations, and making this structure explicit enables powerful capabilities** that traditional imperative languages struggle to provide.

The language is built on four foundational pillars:

1. **Dataflow-First Semantics** - Every construct maps naturally to a dataflow graph
2. **First-Class State** - Inline state management integrated at the language level
3. **Projectional Views** - Multiple representations of the same program for different purposes
4. **Live Editing** - Modify running programs with automatic state reconciliation

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

### Projection and Differentiation Together

The projection system synergizes with back-propagation:

1. Select an output you want to change
2. Project the program to show only what influences that output
3. The projection reveals the "active" computation path
4. Back-propagation computes sensitivities along that path
5. The user adjusts parameters in the simplified view

This combination makes complex optimization tractable by reducing it to a focused, understandable subproblem.

### Abstract Programming and Cross-Language Targeting

A powerful application of projectional editing is **abstract programming**—working with code at a higher level of abstraction that can target multiple underlying representations.

Because projections are bidirectional, Petal can serve as an **abstract editing layer** over foreign programming languages:

1. **Mount** a foreign source file (JavaScript, C, Python, etc.) through a projectional lens
2. The projection presents the code in Petal's abstract representation
3. **Edit** the projection using Petal's tools and semantics
4. Changes are **mapped back** to valid syntax in the foreign language

This enables several workflows:

- **Unified editing experience** - Work with multiple languages through a consistent interface
- **Cross-language refactoring** - Apply Petal's dataflow analysis to foreign codebases
- **Gradual migration** - Edit legacy code through Petal projections while maintaining the original language
- **Polyglot programs** - Seamlessly work across language boundaries in a single project

The key insight is that many programming concepts are universal—functions, loops, conditionals, data structures—even when their syntax differs. A projectional layer can abstract over syntactic differences while preserving semantic intent.

This is related to:
- **Language-parametric tooling** in IDE research
- **Abstract syntax trees** as language-independent representations
- **Transpilation** and source-to-source compilation
- **Universal AST** projects that aim to unify code representation

---

## Goal 4: Live Editing

### The Challenge of Live Modification

**Live editing** (or "hot reloading") allows developers to modify source code while a program is running, seeing changes take effect immediately without restarting. This creates a fluid, interactive development experience that dramatically tightens the feedback loop.

However, live editing faces a fundamental challenge: **state reconciliation**. When source code changes, how do we handle the live state from the running program?

- Variables may be added, removed, or renamed
- Data structures may change shape
- Initialization logic may differ
- The relationship between old state and new code may be ambiguous

Traditional approaches either:
- Discard all state on reload (losing work and context)
- Attempt brittle heuristics to preserve state (often failing unpredictably)
- Require explicit migration code (adding developer burden)

### Inline State as the Solution

Petal's **inline state system** provides a natural solution to state reconciliation. Because state is declared inline with `state` keywords at specific locations in the code, the language has explicit knowledge of:

- **Where** state exists in the program structure
- **What** each piece of state represents
- **How** state relates to control flow (loops, conditionals)

This structural information enables intelligent state reconciliation:

```petal
fn game_loop() {
    state player_x = 100      // Identified by location + name
    state player_y = 200
    state score = 0

    for enemy in enemies {
        state enemy_health = 100  // Per-iteration state, keyed by loop identity
        // ...
    }
}
```

### Automatic State Reconciliation

When source code is modified during execution, Petal can automatically handle state changes:

**State Preservation**
- State declarations that remain unchanged keep their current values
- The structural position (function + control flow path + name) serves as a stable identity

**State Insertion**
- New `state` declarations are initialized with their default values
- Existing execution continues with the new state seamlessly added

**State Removal**
- Removed `state` declarations are cleaned up automatically
- No orphaned state or memory leaks

**State Modification**
- If a state's type or initial value changes, the system can:
  - Keep the old value if types are compatible
  - Re-initialize if types are incompatible
  - Apply user-defined migration functions for complex cases

### Structural Diffing

The key mechanism is **structural diffing** of the state topology:

1. Before: Record the set of active state locations and their values
2. Edit: User modifies source code
3. After: Parse the new code and identify state declarations
4. Diff: Match old state locations to new ones by structural identity
5. Reconcile: Preserve matched state, initialize new state, remove orphaned state

Because state locations are explicit in the syntax, this diffing is precise and predictable—unlike approaches that rely on variable names alone or runtime heuristics.

### Live Editing Workflow

The intended workflow:

1. **Run** a Petal program (game, visualization, interactive tool)
2. **Edit** source code in an editor while the program runs
3. **See** changes take effect immediately—new logic executes, but state persists
4. **Iterate** rapidly without losing application context

For example, while a game is running:
- Modify enemy behavior → enemies immediately use new AI
- Adjust physics parameters → physics changes without resetting positions
- Add new game mechanic → appears without restarting the level
- Fix a bug → fixed immediately in the running game

### Relationship to Dataflow

Live editing integrates with Petal's dataflow model:

- The dataflow graph is **incrementally updated** when code changes
- Only affected computation paths are re-evaluated
- State nodes in the graph maintain their values across edits
- Provenance information remains valid for unchanged portions

This means live editing is not just "restart with preserved state" but true **incremental recomputation**—only the changed parts of the program re-execute.

### Relationship to Existing Work

Petal's live editing approach draws from:

- **Smalltalk** and **Lisp** environments - pioneering live programming
- **Hot module replacement** (Webpack, Vite) - live reloading in web development
- **React Fast Refresh** - preserving component state across edits
- **Elm's time-travel debugger** - structured approach to state and history
- **Live programming research** (Sean McDirmid, Bret Victor) - immediate feedback loops

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
| Projectional Views | Complexity management, focused editing, AI assistance, cross-language programming | Program slicing, bidirectional transformations, lenses, abstract syntax |
| Live Editing | Rapid iteration, interactive development, state-preserving hot reload | Hot module replacement, live programming, incremental computation |

Together, these goals position Petal as a language where programs are not opaque procedures but **transparent, queryable, and manipulable computation graphs**. This foundation enables capabilities—tracing, differentiation, projection—that are difficult or impossible to add to languages designed around imperative mutation.

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
- **Functional Reactive Programming**: Fran, Elm architecture, signal graphs
- **Live Programming**: Sean McDirmid's work, Bret Victor's "Inventing on Principle"
- **Hot Reloading**: Webpack HMR, React Fast Refresh, Erlang hot code swapping
- **Incremental Computation**: Self-adjusting computation, incremental view maintenance
- **Language-Parametric Tooling**: Spoofax, universal AST representations
