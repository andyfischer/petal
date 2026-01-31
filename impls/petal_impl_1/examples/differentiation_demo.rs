//! Demo of automatic differentiation in Petal
//!
//! This demonstrates how to compute gradients through a computation graph.

use petal::Env;

fn main() {
    let mut env = Env::new();

    // Simple computation: f(x) = x * x + 2 * x
    // where x = 3
    // f(3) = 9 + 6 = 15
    // f'(x) = 2x + 2
    // f'(3) = 8
    let source = r#"
let x = 3
let x_squared = x * x
let two_x = 2 * x
x_squared + two_x
"#;

    println!("=== Automatic Differentiation Demo ===");
    println!();
    println!("Program:");
    println!("{}", source);

    // Load and run with tracing
    let program_key = env.load_program(source).expect("Parse failed");
    let stack_key = env.create_stack(program_key).expect("Stack creation failed");

    let (result, trace, gradients) = env
        .run_with_gradients(stack_key, 1.0)
        .expect("Run failed");

    println!("Result: {}", result);
    println!();
    println!("Computation Trace ({} steps):", trace.all_steps().len());
    for step in trace.all_steps().iter().take(10) {
        println!(
            "  Term {:?}: inputs={:?} -> output={:?}",
            step.term_id, step.inputs, step.output
        );
    }

    println!();
    println!("Gradients (df/d[term]):");
    for (term_id, grad) in &gradients.term_gradients {
        if *grad != 0.0 {
            println!("  Term {:?}: gradient = {}", term_id, grad);
        }
    }

    println!();
    println!("Explanation:");
    println!("  For f(x) = x^2 + 2x at x=3:");
    println!("  f(3) = 9 + 6 = 15");
    println!("  f'(x) = 2x + 2");
    println!("  f'(3) = 8");
    println!();
    println!("  The gradients show how each term contributes to the final result.");
}
