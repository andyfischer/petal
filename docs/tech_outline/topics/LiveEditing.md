# Live Editing

How to transfer state from a running program to a modified program.

## Related Topics

- [[CodeManipulation]] - Modifying program structure
- [[Execution]] - Running the modified program

## Overview

Live editing allows you to modify a program's source code while it's running, preserving as much state as possible. This is similar to hot module reloading in web development, but at a finer granularity.

## The Live Edit Workflow

```rust
// 1. Program is running with some state
let stack = env.create_stack(program)?;
env.run(stack)?;  // Runs, accumulates state
env.reset_stack(stack)?;

// 2. User edits the source
let edit = SourceEdit {
    range: SourceRange { start: 10, end: 20 },
    new_text: "new_expression".to_string(),
};

// 3. Apply the edit
let edit_result = env.live_edit(program, edit)?;

// 4. Reconcile state
let reconciliation = env.reconcile_state(stack)?;

// 5. Continue execution with preserved state
env.run(stack)?;
```

## Understanding Edit Results

The `LiveEditResult` tells you what changed:

```rust
pub struct LiveEditResult {
    pub added_terms: Vec<TermId>,    // New terms created
    pub removed_terms: Vec<TermId>,  // Terms that no longer exist
    pub modified_terms: Vec<TermId>, // Terms with changed operations
}
```

## State Reconciliation

The [[StateSchema|StateSchema]] determines how state maps from old to new program versions:

```rust
pub struct StateReconciliation {
    pub preserved: Vec<StateKey>,   // State that transferred directly
    pub initialized: Vec<StateKey>, // New state needing initialization
    pub removed: Vec<StateKey>,     // Old state with no match
}
```

State is matched by:
1. **Term ID** - If the declaring term still exists
2. **Name** - If the state variable has the same name
3. **Position** - As a fallback, by source location

## Handling Reconciliation

```rust
let reconciliation = env.reconcile_state(stack)?;

// Log what happened
println!("Preserved {} state values", reconciliation.preserved.len());
println!("Initialized {} new state values", reconciliation.initialized.len());
println!("Removed {} orphaned state values", reconciliation.removed.len());

// Optionally provide custom initialization for new state
for state_key in &reconciliation.initialized {
    env.set_state(stack, *state_key, initial_value)?;
}
```

## State Keys in Loops

For state inside loops, the [[StateSchema|StateSchema]] uses iteration keys:

```rust
// Index-based: state[0], state[1], state[2]...
// If loop bounds change, state may be lost

// Key-based: state[item.id]
// State follows the key across reorderings
```

## Stable Term IDs

For live editing to work well, term IDs should be stable across edits. Petal uses content-addressed hashing:

```rust
// Term ID is derived from:
// - Operation type
// - Source location
// - A disambiguator for duplicates
```

This means renaming a variable doesn't change the term ID of expressions using it.

## Limitations

- **Type changes**: If a state variable's type changes, it must be reinitialized
- **Structural changes**: Moving state into/out of loops requires reinitialization
- **Dependencies**: If a state's initializer changes, you may want to reinitialize

## Custom State Migration

For complex cases, provide a migration function:

```rust
env.live_edit_with_migration(program, edit, |old_state, new_schema| {
    // Custom logic to transform old state to new schema
    let mut new_state = HashMap::new();

    // Example: rename a state key
    if let Some(value) = old_state.get(&old_key) {
        new_state.insert(new_key, value.clone());
    }

    new_state
})?;
```

---

See also: [[Outline|Implementation Plan]]
