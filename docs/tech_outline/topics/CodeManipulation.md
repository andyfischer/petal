# Code Manipulation

How to modify a compiled [[Program|Program]].

## Related Topics

- [[LiveEditing]] - Modifying programs while they run
- [[Execution]] - Running modified programs

## The Program Graph

A [[Program|Program]] is a collection of [[Term|Terms]] connected by dataflow edges. Each term has:
- An operation (`TermOp`)
- Input references to other terms
- Control flow links (next/prev)

## Reading Program Structure

```rust
// Get a reference to the program
let program = env.get_program(program_key)?;

// Iterate over all terms
for (term_id, term) in program.terms() {
    println!("Term {:?}: {:?}", term_id, term.op);
    println!("  Inputs: {:?}", term.inputs);
}

// Get the entry point
let entry = program.entry();
```

## Modifying Terms

Terms can be modified in place:

```rust
// Change a literal value
env.modify_term(program_key, term_id, |term| {
    term.op = TermOp::IntLiteral(42);
})?;

// Rewire inputs
env.modify_term(program_key, term_id, |term| {
    term.inputs[0] = other_term_id;
})?;
```

## Adding New Terms

```rust
// Add a new term to the program
let new_term_id = env.add_term(program_key, Term {
    id: TermId(0), // Will be assigned
    op: TermOp::Add,
    inputs: smallvec![left_term, right_term],
    control_flow_next: None,
    control_flow_prev: Some(prev_term),
    register: RegisterIndex(0), // Will be assigned
    state_key: None,
})?;
```

## Removing Terms

```rust
// Remove a term (disconnects it from the graph)
env.remove_term(program_key, term_id)?;
```

Note: Removing a term that other terms depend on will leave dangling references. Use `env.validate_program(program_key)` to check for issues.

## Recomputing Metadata

After modifications, you may need to update derived data:

```rust
// Recompute register assignments
env.assign_registers(program_key)?;

// Update the source map (if source positions changed)
env.rebuild_source_map(program_key)?;

// Validate the program graph
let errors = env.validate_program(program_key)?;
```

## Cloning Programs

To experiment with modifications without affecting the original:

```rust
let cloned_program = env.clone_program(program_key)?;

// Modify the clone freely
env.modify_term(cloned_program, term_id, |term| {
    term.op = TermOp::Mul; // Change add to multiply
})?;
```

---

See also: [[Outline|Implementation Plan]]
