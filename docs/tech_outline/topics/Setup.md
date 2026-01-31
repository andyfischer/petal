# Setup

How to set up an [[Env|Env]] and get started with Petal.

## Related Topics

- [[Execution]] - Running programs after setup
- [[WebAssembly]] - Setup via the WASM API

## Creating an Environment

The [[Env|Env]] is the foundational data structure. All programs and stacks live inside an Env.

```rust
use petal::Env;

fn main() {
    // Create a new environment
    let mut env = Env::new();

    // The environment is now ready to load programs
}
```

## Loading a Program

Programs are loaded from source code strings:

```rust
let source = r#"
    let x = 1 + 2
    let y = x * 3
    y
"#;

let program_key = env.load_program(source)?;
```

The `load_program` method:
1. Parses the source into [[Term|Terms]]
2. Builds the [[SourceMap|SourceMap]] for debugging
3. Registers the [[Program|Program]] in the Env
4. Returns a `ProgramKey` for future reference

## Creating a Stack

To execute a program, create a [[Stack|Stack]]:

```rust
let stack_key = env.create_stack(program_key)?;
```

The stack holds:
- The current execution state (frames, registers)
- Persistent state storage for `state` declarations
- A reference back to the program being executed

## Complete Example

```rust
use petal::{Env, Value};

fn main() -> Result<(), petal::Error> {
    // 1. Create environment
    let mut env = Env::new();

    // 2. Load program
    let program = env.load_program("1 + 2 * 3")?;

    // 3. Create execution stack
    let stack = env.create_stack(program)?;

    // 4. Run to completion
    let result = env.run(stack)?;

    // 5. Inspect result
    match result {
        Value::Int(n) => println!("Result: {}", n),
        _ => println!("Unexpected result type"),
    }

    Ok(())
}
```

## Registering Built-in Functions

Before loading programs, you can register built-in functions:

```rust
env.register_builtin("print", |args| {
    for arg in args {
        println!("{:?}", arg);
    }
    Ok(Value::Nil)
});

env.register_builtin("sqrt", |args| {
    match &args[0] {
        Value::Float(f) => Ok(Value::Float(f.sqrt())),
        Value::Int(i) => Ok(Value::Float((*i as f64).sqrt())),
        _ => Err(Error::TypeError("sqrt requires a number")),
    }
});
```

---

See also: [[Outline|Implementation Plan]]
