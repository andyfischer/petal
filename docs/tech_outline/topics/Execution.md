# Execution

How to run a [[Program|Program]] with the interpreter.

## Related Topics

- [[Setup]] - Creating an Env and loading programs
- [[LiveEditing]] - Modifying programs during execution

## Running to Completion

The simplest way to execute a program:

```rust
let result = env.run(stack_key)?;
```

This runs until the program completes or an error occurs.

## Stepping Through Execution

For debugging or interactive use, step one term at a time:

```rust
loop {
    match env.step(stack_key)? {
        StepResult::Continue => {
            // Program still running, continue stepping
        }
        StepResult::Complete(value) => {
            println!("Done: {:?}", value);
            break;
        }
        StepResult::Breakpoint(term_id) => {
            println!("Hit breakpoint at {:?}", term_id);
            // Inspect state, then continue or abort
        }
        StepResult::Error(err) => {
            println!("Error: {:?}", err);
            break;
        }
    }
}
```

## Inspecting Execution State

During execution, you can inspect the [[Stack|Stack]]:

```rust
// Get current term being executed
let current = env.current_term(stack_key)?;

// Get value of any term that has been evaluated
if let Some(value) = env.get_term_value(stack_key, term_id) {
    println!("Term {:?} = {:?}", term_id, value);
}

// Get the current frame's registers
let registers = env.get_registers(stack_key)?;
```

## Setting Breakpoints

```rust
// Break when a specific term is about to execute
env.set_breakpoint(stack_key, term_id)?;

// Break on any state write
env.set_breakpoint_on_state_write(stack_key)?;

// Clear breakpoints
env.clear_breakpoints(stack_key)?;
```

## State Persistence

For programs with `state` declarations, the state persists in the [[Stack|Stack]]:

```rust
// First run
let result1 = env.run(stack_key)?;

// Reset to entry point but keep state
env.reset_stack(stack_key)?;

// Second run sees the state from the first run
let result2 = env.run(stack_key)?;
```

## Multiple Stacks

You can create multiple stacks for the same program:

```rust
let stack1 = env.create_stack(program_key)?;
let stack2 = env.create_stack(program_key)?;

// Each stack has independent state
env.run(stack1)?;
env.run(stack2)?;
```

## Execution with Timeout

```rust
use std::time::Duration;

// Run with a timeout
match env.run_with_timeout(stack_key, Duration::from_secs(5))? {
    RunResult::Completed(value) => println!("Done: {:?}", value),
    RunResult::TimedOut => println!("Execution timed out"),
}
```

## Provenance Tracking

Enable provenance to track what influenced each value:

```rust
// Enable provenance mode (slower but tracks dependencies)
env.enable_provenance(stack_key)?;

env.run(stack_key)?;

// What terms influenced this result?
let influences = env.get_provenance(program_key, output_term)?;
```

See [[ExecutionTrace|ExecutionTrace]] for the underlying data structure.

---

See also: [[Outline|Implementation Plan]]
