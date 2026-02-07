# Metaprogramming

How to use `Program` as a first-class value to inspect, execute, transform, and compose programs from within Petal itself.

## Related Topics

- [[Execution]] - Running programs from the Rust host
- [[CodeManipulation]] - Modifying program graphs at the Rust level
- [[LiveEditing]] - State reconciliation during edits
- [[Backflow]] - Automatic differentiation
- [[Projection]] - Program slicing

## Overview

Petal's internal representation -- programs as graphs of [[Term|Terms]] organized into [[Block|Blocks]] -- is exposed directly to Petal code through the `Program` type and its associated types. This makes all four pillars (dataflow, state, projection, live editing) available as first-class operations within the language itself, rather than only through the Rust host API.

The key types:

| Type | What it represents |
|------|-------------------|
| `Program` | A parsed program (the IR graph). Immutable snapshot of code. |
| `Stack` | An execution context for a Program. Holds registers, frames, and state. |
| `Term` | A single node in the program graph (read-only view). |
| `Block` | A control flow scope containing a sequence of terms (read-only view). |
| `Projection` | A computed subset of a program's terms. |
| `Trace` | A recorded execution with full step-by-step provenance. |

---

## Creating Programs

### From source code

```petal
let prog = Program.parse("
    fn square(x) { x * x }
    square(5)
")
```

Parse errors do not prevent creation. The program is still inspectable, and error terms are included in the graph (see `program.errors()`).

### From file

```petal
let prog = Program.from_file("./examples/game_of_life.ptl")
```

### Self-reference

A running program can obtain a handle to itself or to the currently executing function:

```petal
let me = Program.current()
print("This program has", len(me.terms()), "terms")

fn introspective() {
    let self_fn = Program.current_function()
    print("I am called", self_fn.name)
    print("I have", len(self_fn.terms()), "nodes")
}
```

`Program.current()` returns the entire program. `Program.current_function()` returns a `Block` view scoped to the function being executed.

### Programmatic construction

Build a program node-by-node using the builder:

```petal
let prog = Program.build(fn(b) {
    let x = b.param("x")
    let y = b.param("y")
    let sum = b.add(x, y)
    let doubled = b.mul(sum, b.literal(2))
    b.return(doubled)
})

print(prog.run(3, 4))  // 14
```

The builder mirrors the [[Term#TermOp|TermOp]] vocabulary:

| Builder method | TermOp |
|---------------|--------|
| `b.literal(value)` | Constant |
| `b.param(name)` | (parameter binding) |
| `b.add(a, b)` | Add |
| `b.sub(a, b)` | Sub |
| `b.mul(a, b)` | Mul |
| `b.div(a, b)` | Div |
| `b.eq(a, b)` | Eq |
| `b.lt(a, b)` | Lt |
| `b.gt(a, b)` | Gt |
| `b.call(fn, args)` | Call |
| `b.branch(cond, then_b, else_b)` | Branch |
| `b.state(name, init)` | StateRead / StateWrite |
| `b.list(items)` | AllocList |
| `b.map(entries)` | AllocMap |
| `b.get_field(obj, field)` | GetField |
| `b.return(value)` | Return |

Nested blocks (if bodies, loops) use closures:

```petal
let prog = Program.build(fn(b) {
    let n = b.param("n")
    b.branch(b.gt(n, b.literal(0)),
        fn(then) { then.return(then.literal("positive")) },
        fn(else_) { else_.return(else_.literal("non-positive")) }
    )
})
```

---

## Program Introspection

### Source code

```petal
program.source()         // the original source text (string)
program.has_errors?()    // true if parse errors exist
program.errors()         // list of { message, span } records
```

### Structure

```petal
program.terms()          // list of all Term values
program.blocks()         // list of all Block values
program.root()           // the root Block
program.names()          // list of all bound names (strings)
program.functions()      // list of function definition Blocks
```

### Finding terms

```petal
// By name
let x_term = program.find("x")

// All terms matching a predicate
let adds = program.find_all(fn(t) { t.op == Add })

// By source position
let term = program.term_at(line: 5, column: 10)
```

---

## Term Introspection

A `Term` is a read-only view of a node in the program graph. It exposes the fields documented in [[Term]]:

```petal
term.id              // TermId (opaque, usable as a key)
term.op              // TermOp enum value: Add, Mul, Constant, Branch, ...
term.name            // bound name, or nil if unnamed
term.inputs          // list of input Term values (dataflow predecessors)
term.block           // the Block this term belongs to
term.source_span     // { start: { line, column, offset }, end: { ... } }
```

### Derived queries

```petal
term.dependents      // terms that use this term as input (dataflow successors)
term.has_state?()    // whether this term has an associated state key
term.is_error?()     // whether this is a parse error node
term.is_binding?()   // whether this term binds a name (let, param, state)
```

### Walking the graph

```petal
// Walk all transitive inputs (recursively)
fn walk_inputs(term, visitor) {
    visitor(term)
    for input in term.inputs {
        walk_inputs(input, visitor)
    }
}

let x_term = program.find("result")
walk_inputs(x_term, fn(t) {
    if t.name != nil {
        print(" ", t.name, "->", t.op)
    }
})
```

---

## Block Introspection

A `Block` is a read-only view of a control flow scope:

```petal
block.id             // BlockId
block.parent         // the parent Term that introduces this block (nil for root)
block.terms()        // ordered list of terms in this block
block.children()     // nested Blocks (if-bodies, loop-bodies, etc.)
block.name           // function name if this is a function body, nil otherwise
```

---

## Execution

### Simple run

```petal
let result = program.run()
let result = program.run(arg1, arg2)   // pass arguments to the entry function
```

State persists across `.run()` calls on the same `Program` value.

### Stack-based execution

For finer control, create a `Stack` -- an independent execution context:

```petal
let stack = program.create_stack()
```

Multiple stacks can run the same program with independent state:

```petal
let s1 = program.create_stack()
let s2 = program.create_stack()
s1.run()   // independent state
s2.run()   // independent state
```

### Stepping

```petal
loop {
    match stack.step() {
        Continue -> { }
        Complete(value) -> {
            print("Done:", value)
            break
        }
        Breakpoint(term) -> {
            print("Hit breakpoint at", term.name)
        }
        Error(msg) -> {
            print("Error:", msg)
            break
        }
    }
}
```

`StepResult` is an enum:

```petal
enum StepResult {
    Continue
    Complete(value)
    Breakpoint(term)
    Error(message)
}
```

### Inspecting execution state

```petal
stack.current_term()          // the Term about to execute
stack.value(name)             // read a variable's current value by name
stack.value(term)             // read a value by Term reference
stack.registers()             // record of all register values in the current frame
stack.frames()                // list of active frames (call stack)
stack.is_complete?()          // whether execution has finished
```

### Breakpoints

```petal
stack.set_breakpoint(term)             // break before this term executes
stack.set_breakpoint_on("x")           // break when "x" is evaluated
stack.set_breakpoint_on_state_write()  // break on any state write
stack.clear_breakpoints()
```

### Reset

```petal
stack.reset()          // reset to beginning, preserve state
stack.reset_state()    // clear all state, reset to beginning
```

---

## State Inspection and Control

State declared with the `state` keyword persists in the Stack. The metaprogramming API makes it queryable:

```petal
stack.state()                // record of all state keys -> values
stack.state("count")         // value of a specific state variable
stack.set_state("count", 0)  // override a state value
stack.reset_state()          // clear all state

// Iterate over state
for entry in stack.state_entries() {
    print(entry.key, "=", entry.value, "at", entry.source_span)
}
```

State entries include their source location, which connects the runtime state back to the `state` declaration in source code.

---

## Live Editing

Modify a program's source while preserving state. All edit operations return a `Reconciliation` record describing what happened to state:

### Text replacement

```petal
let recon = program.edit("count + 1", "count + 10")
```

### Positional edit

```petal
let recon = program.edit_at(offset: 42, length: 9, text: "count + 10")
```

### Full source replacement

```petal
let recon = program.replace_source("
    fn counter() {
        state count = 0
        count = count + 10
        count
    }
    counter()
")
```

### The Reconciliation record

```petal
recon.preserved       // number of state variables kept intact
recon.initialized     // number of new state variables created with defaults
recon.removed         // number of orphaned state variables dropped

recon.details         // list of per-variable detail records:
// [
//   { key: "count", status: Preserved, value: 3 },
//   { key: "new_var", status: Initialized, default: 0 },
//   { key: "old_var", status: Removed, last_value: 42 },
// ]

enum ReconciliationStatus { Preserved, Initialized, Removed }
```

### Edit with custom migration

For complex state changes, provide a migration function:

```petal
let recon = program.edit_with_migration(
    "state pos = { x: 0, y: 0 }",
    "state pos = { x: 0.0, y: 0.0, z: 0.0 }",
    fn(old_state) {
        // Convert old 2D position to new 3D position
        { x: float(old_state.x), y: float(old_state.y), z: 0.0 }
    }
)
```

### Structural diff

After an edit, inspect what changed in the program graph:

```petal
let diff = program.diff(old_source, new_source)
diff.added_terms       // list of new Terms
diff.removed_terms     // list of removed Terms
diff.modified_terms    // list of Terms whose operation changed
```

---

## Differentiation

### Function-level (recommended for most uses)

```petal
// grad(f) returns a new function that computes df/d(first_arg)
let f_prime = grad(f)
print(f_prime(3.0))

// gradients returns value + per-argument gradients
let result = gradients(f, 3.0, 4.0)
result.value           // f(3.0, 4.0)
result.grads           // [df/da, df/db]
```

### Program-level (more control)

```petal
// Compute gradient between specific terms
let g = program.gradient("output", "x")
print("d(output)/d(x) =", g)

// Full backpropagation through a Stack's execution
let grads = program.backpropagate(stack, "output", 1.0)
for entry in grads {
    print(entry.term.name, ":", entry.gradient)
}

// Forward-mode differentiation (efficient for few inputs, many outputs)
let perturbations = program.forward_diff(stack, "input", 1.0)

// Symbolic derivative: returns a new Program that computes the derivative
let deriv_prog = program.symbolic_derivative("output", with_respect_to: "x")
print(deriv_prog.run(3.0))
```

### Applying gradients

```petal
let grads = program.backpropagate(stack, "loss", 1.0)
let learning_rate = 0.01

for entry in grads {
    if entry.term.is_binding?() && entry.gradient != 0.0 {
        let old = stack.value(entry.term)
        stack.set_value(entry.term, old - learning_rate * entry.gradient)
    }
}
```

---

## Projection

### Creating projections

```petal
// Backward slice: what influences target?
let deps = program.slice("result")

// Forward slice: what does source influence?
let effects = program.impact("user_input")

// Dynamic slice: what actually executed for specific inputs?
let dyn = program.dynamic_slice(stack, "output")
```

### Projection introspection

```petal
projection.terms()            // list of Terms in the slice
projection.edges()            // list of { from: Term, to: Term } dataflow edges
projection.source()           // the slice rendered as simplified source code
projection.contains?("x")    // whether name "x" is in the slice
projection.size()             // number of terms
```

### Combining projections

```petal
// What code is shared between two outputs?
let proj_a = program.slice("output_a")
let proj_b = program.slice("output_b")
let shared = proj_a.intersect(proj_b)

// What code is relevant to either output?
let combined = proj_a.union(proj_b)

// What code is unique to output_a?
let unique = proj_a.subtract(proj_b)
```

### Extracting sub-programs

A projection can be extracted as a standalone program:

```petal
let sub = projection.extract()
// sub is a new Program containing only the projected terms
// Inputs to the slice become parameters
print(sub.source())
print(sub.run(5, 10))
```

This is powerful for isolating behavior: take a complex program, slice it to the parts that matter for a specific output, and get a simplified standalone program.

---

## Provenance and Tracing

### Recording a trace

```petal
let trace = program.trace(arg1, arg2)
// or from a Stack:
stack.enable_tracing()
stack.run()
let trace = stack.trace()
```

### Trace introspection

```petal
trace.result              // the final value
trace.steps               // list of TraceStep records
trace.step_count()        // total steps executed

// Each step:
step.term                 // the Term that executed
step.inputs               // list of input values
step.output               // the produced value
step.timestamp            // execution order index
```

### Provenance queries

```petal
// What influenced a particular variable?
let chain = trace.influences("result")
// Returns list of Terms that contributed to the value of "result"

// Path between two variables
let path = trace.path("input", "output")
// Returns the chain of terms from input to output

// Did a particular term contribute?
trace.contributed?("unused_var", to: "result")  // false
```

### Formatted output

```petal
trace.print()
// Output:
//   a = 10
//   b = 20
//   sum = a + b -> 30
//   result = sum * 2 -> 60
//   return: 60
```

---

## Program Transformation

### Term-level transforms

Apply a function to each term, producing a new program:

```petal
// Replace all additions with multiplications
let transformed = program.transform(fn(term) {
    match term.op {
        Add -> term.with_op(Mul)
        _ -> term
    }
})
```

The `term.with_op(new_op)` method returns a modified copy of the term. Available `with_*` methods:

```petal
term.with_op(new_op)               // change the operation
term.with_inputs(new_inputs)       // change input wiring
term.with_name(new_name)           // change the bound name
```

### Constant folding

```petal
// Evaluate compile-time-known expressions
let optimized = program.fold_constants()
```

### Inlining

```petal
// Inline a specific function call
let inlined = program.inline("helper_function")

// Inline all small functions
let inlined = program.inline_all(max_terms: 10)
```

### Composition

```petal
// Pipe: output of a becomes input of b
let pipeline = prog_a.pipe(prog_b)

// Merge: combine into one program (shared namespace)
let combined = prog_a.merge(prog_b)
```

---

## Program Comparison

### Structural diff

```petal
let diff = program_v1.diff(program_v2)

diff.added       // terms in v2 but not v1
diff.removed     // terms in v1 but not v2
diff.modified    // terms present in both but with different ops
diff.unchanged   // terms identical in both

diff.summary()   // human-readable diff
```

### Equivalence

```petal
// Structural equality (same graph shape and operations)
prog_a.equivalent?(prog_b)

// Semantic equality (same outputs for all inputs -- tested, not proven)
prog_a.semantically_equal?(prog_b, test_inputs: [[1], [2], [3]])
```

---

## Self-Modifying Programs

Combining `Program.current()` with live editing enables programs that modify themselves at runtime:

```petal
fn evolving_counter() {
    state count = 0
    count = count + 1

    // After 10 calls, upgrade myself to count by 10
    if count == 10 {
        let me = Program.current()
        me.edit("count + 1", "count + 10")
        // State is preserved -- count stays at 10
        // Next call will increment by 10
    }

    count
}
```

### Inspecting the call graph

```petal
let me = Program.current()

// What functions exist?
for func in me.functions() {
    print(func.name, ":", len(func.terms()), "terms")
}

// What does the current function depend on?
let deps = me.slice(Program.current_function().name)
print("Dependencies:", deps.source())
```

### Generating code at runtime

```petal
// Build a specialized function based on runtime data
fn make_polynomial(coefficients) {
    Program.build(fn(b) {
        let x = b.param("x")
        let result = b.literal(0.0)
        for i in range(0, len(coefficients)) {
            // term = coeff * x^i
            let power = b.literal(1.0)
            for _ in range(0, i) {
                power = b.mul(power, x)
            }
            let term = b.mul(b.literal(coefficients[i]), power)
            result = b.add(result, term)
        }
        b.return(result)
    })
}

// 3x^2 + 2x + 1
let poly = make_polynomial([1.0, 2.0, 3.0])
print(poly.run(5.0))  // 86.0

// And we can differentiate the generated program
let poly_prime = grad(poly.run)
print(poly_prime(5.0))  // 32.0  (6x + 2 at x=5)
```

---

## Design Notes

### Immutability of Program values

A `Program` value is a snapshot. Methods like `.edit()` and `.transform()` modify the `Program` in place and return reconciliation info, rather than creating a new `Program`. This is because the `Program` holds associated state (via its stacks) and identity matters for live editing.

To get a copy, use:
```petal
let copy = program.clone()
```

### Performance considerations

- `program.terms()` and `program.blocks()` return lightweight views, not deep copies.
- `Term` and `Block` values are references into the program graph. They become invalid if the program is edited (access after edit returns nil).
- Traces can be large. Use `stack.enable_tracing()` selectively, or use `program.trace()` which enables tracing only for that one run.
- Projections are computed lazily where possible. Combining projections (intersect, union) operates on term ID sets without re-analyzing the graph.

### Relationship to the Rust host API

The Petal metaprogramming API wraps the same [[Env]] operations documented in [[Outline#Public API|the public API]]. The mapping is:

| Petal | Rust |
|-------|------|
| `Program.parse(src)` | `env.load_program(src)` |
| `program.run()` | `env.run(stack_key)` |
| `program.create_stack()` | `env.create_stack(program_id)` |
| `stack.step()` | `env.step(stack_key)` |
| `program.edit(old, new)` | `env.live_edit(program_id, edit)` + `env.reconcile_state(stack_key)` |
| `program.slice(name)` | `env.project(program_id, Backward(term_id))` |
| `program.backpropagate(...)` | `env.backpropagate(stack_key, term_id, seed)` |
| `program.trace(args)` | `env.enable_provenance(stack_key)` + `env.run(stack_key)` |

The Petal layer adds convenience (name-based lookup instead of raw TermIds), composability (projection algebra), and safety (invalid references return nil instead of panicking).

---

See also: [[Outline|Implementation Plan]]
