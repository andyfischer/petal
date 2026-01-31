# Projection

How to create and use a projection (program slice).

## Related Topics

- [[Execution]] - Running programs to gather dynamic info
- [[Backflow]] - Using projections for targeted differentiation

## Overview

A **projection** is a subset of a program's terms that are relevant to a particular focus. Projections help you:

- Understand what code affects a value (backward slice)
- Understand what a piece of code influences (forward slice)
- Debug by seeing only relevant code paths (dynamic slice)

## Creating Projections

```rust
let projection = env.project(program, focus)?;
```

The `ProjectionFocus` determines what's included:

```rust
pub enum ProjectionFocus {
    /// What does this term influence?
    Forward(TermId),

    /// What influences this term?
    Backward(TermId),

    /// What was actually executed to produce this value?
    Dynamic { stack_id: StackKey, target_term: TermId },
}
```

## Backward Projection (Dependency Slice)

Find all terms that could affect a target:

```rust
let projection = env.project(
    program,
    ProjectionFocus::Backward(output_term),
)?;

println!("Terms that influence the output:");
for term_id in &projection.included_terms {
    let term = env.get_term(program, *term_id)?;
    println!("  {:?}: {:?}", term_id, term.op);
}
```

## Forward Projection (Impact Slice)

Find all terms that a change could affect:

```rust
let projection = env.project(
    program,
    ProjectionFocus::Forward(input_term),
)?;

println!("Terms affected by this input:");
for term_id in &projection.included_terms {
    println!("  {:?}", term_id);
}
```

## Dynamic Projection (Execution Slice)

Based on actual execution, not just static analysis:

```rust
// Run the program first
env.enable_provenance(stack)?;
env.run(stack)?;

// Now get the dynamic slice
let projection = env.project(
    program,
    ProjectionFocus::Dynamic {
        stack_id: stack,
        target_term: output_term
    },
)?;
```

Dynamic slices are smaller than static slices because they exclude:
- Branches not taken
- Loop iterations that didn't contribute
- Dead code

## The Projection Structure

```rust
pub struct Projection {
    /// Terms included in this projection
    pub included_terms: HashSet<TermId>,

    /// Simplified dataflow edges (only within projection)
    pub dataflow_edges: Vec<(TermId, TermId)>,
}
```

## Using Projections

### Visualizing Dependencies

```rust
// Generate a DOT graph of the projection
let dot = projection.to_dot_graph(&env, program)?;
std::fs::write("slice.dot", dot)?;
// Run: dot -Tpng slice.dot -o slice.png
```

### Focused Debugging

```rust
// Set breakpoints only on projected terms
for term_id in &projection.included_terms {
    env.set_breakpoint(stack, *term_id)?;
}
```

### Extracting a Sub-Program

```rust
// Create a new program containing only the projected terms
let sub_program = env.extract_subprogram(program, &projection)?;
```

## Projection Intersection

Combine projections to find common dependencies:

```rust
let proj_a = env.project(program, ProjectionFocus::Backward(term_a))?;
let proj_b = env.project(program, ProjectionFocus::Backward(term_b))?;

// Terms that influence both A and B
let common: HashSet<_> = proj_a.included_terms
    .intersection(&proj_b.included_terms)
    .collect();
```

## Incremental Updates

When the program changes, update projections efficiently:

```rust
// After a live edit
let edit_result = env.live_edit(program, edit)?;

// Update existing projection
projection.update_after_edit(&edit_result)?;
```

---

See also: [[Outline|Implementation Plan]]
