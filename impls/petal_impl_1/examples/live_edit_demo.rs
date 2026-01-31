//! Demo of live editing in Petal
//!
//! This demonstrates how to modify a running program while preserving state.

use petal::Env;
use petal::live_edit::SourceEdit;

fn main() {
    let mut env = Env::new();

    // Original program with state
    let source_v1 = r#"fn counter() {
    state count = 0
    count = count + 1
    count
}
counter()"#;

    println!("=== Live Edit Demo ===");
    println!();

    // Load and run the original program
    let program_key = env.load_program(source_v1).expect("Parse failed");
    let stack_key = env.create_stack(program_key).expect("Stack creation failed");

    println!("Original program:");
    println!("{}", source_v1);

    // Run a few times to build up state
    for i in 1..=3 {
        env.reset_stack(stack_key).unwrap();
        let result = env.run(stack_key).expect("Run failed");
        println!("Call {}: count = {}", i, result);
    }

    println!();
    println!("--- Applying Live Edit ---");
    println!();

    // Edit the program: change increment from 1 to 10
    // Find "count + 1" and replace with "count + 10"
    let search = "count + 1";
    let replace = "count + 10";
    let start = source_v1.find(search).expect("Pattern not found");
    let end = start + search.len();

    let edit = SourceEdit::replace(start, end, replace.to_string());

    let source_v2 = edit.apply(source_v1);
    println!("Modified program:");
    println!("{}", source_v2);

    // Apply the live edit
    let (_new_program_key, reconciliation) = env
        .live_edit(program_key, stack_key, source_v1, &edit)
        .expect("Live edit failed");

    println!();
    println!("State reconciliation:");
    println!("  Preserved: {} state variables", reconciliation.preserved.len());
    println!("  Needs init: {} state variables", reconciliation.needs_init.len());
    println!("  Orphaned: {} state variables", reconciliation.orphaned.len());
    println!();

    // Continue running - state should be preserved!
    for i in 4..=6 {
        env.reset_stack(stack_key).unwrap();
        let result = env.run(stack_key).expect("Run failed");
        println!("Call {}: count = {}", i, result);
    }

    println!();
    println!("Notice: The count continued from where it left off!");
    println!("State was preserved across the live edit.");
}
