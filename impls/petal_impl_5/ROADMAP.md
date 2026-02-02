# Petal Implementation Roadmap

## Completed ✅

### Phase 1: Core Language (Complete)
- [x] Lexer with 30+ token types
- [x] Recursive descent parser
- [x] Expression evaluation with operator precedence
- [x] 8 value types (Nil, Bool, Int, Float, String, List, Map, Function)
- [x] All arithmetic, comparison, and logical operators
- [x] If-else control flow
- [x] Collections (lists, maps) with indexing
- [x] Type conversions
- [x] 11 built-in functions
- [x] REPL and script execution
- [x] 12 sample programs

### Phase 2: Variables, State, Functions (Complete)
- [x] Variable binding with `let` and lexical scoping
- [x] State management with `state` and persistence
- [x] User-defined functions with parameters
- [x] Full recursion support
- [x] Function nesting and composition
- [x] Parameter binding and shadowing
- [x] 6 new sample programs
- [x] Proper borrow-safe evaluation

## In Progress 🔄

### Phase 3: Loops (Recommended Next)

**Estimated Effort:** 1-2 hours
**Impact:** Enable iteration patterns, unlock many use cases

#### Features to Implement

**For Loops**
```petal
for i in range(0, 10) {
    print(i)
}

for item in [1, 2, 3, 4, 5] {
    print(item * 2)
}
```

**While Loops**
```petal
let x = 0
while x < 5 {
    print(x)
    # Would need mutation: x = x + 1
}
```

#### Implementation Tasks

1. **Parser Changes**
   - Add `TermOp::For { var, iter, body }`
   - Add `TermOp::While { cond, body }`
   - Recognize `for...in` and `while` syntax

2. **Evaluator Changes**
   - For loops: iterate over list, bind loop var, execute body
   - While loops: check condition, execute body, repeat
   - Loop variable scoping (similar to function parameters)

3. **Semantics**
   - Loop variables shadow outer scope
   - Break/continue support (optional first pass)
   - Loop-local state initialization

4. **Samples**
   - 19_loops_for.ptl - Basic for loop
   - 20_loops_while.ptl - While loop pattern
   - 21_nested_loops.ptl - Nested iteration

#### Key Design Decisions

- **Loop variables** handled like function parameters (save/restore bindings)
- **Iteration protocol**: For loops work with any list (range, map values, etc.)
- **Body execution**: Each iteration runs the body term
- **State sharing**: Loop-local state could be tied to loop variable values

---

## Not Yet Implemented 🔮

### Phase 4: Mutation & Advanced State (3-4 hours)

**Enables:** Stateful algorithms, game loops, simulations

#### Features
- Mutation operators: `+=`, `-=`, `*=`, `/=`
- Array mutations: `push`, `pop`, indexing assignment
- Map mutations: field assignment
- State update patterns

#### Example
```petal
state count = 0
state data = [1, 2, 3]

# Mutation (once implemented)
count += 1
data[0] = 10
```

#### Implementation
- Add `TermOp::UpdateVar` and `TermOp::UpdateState`
- Modify state HashMap during evaluation
- Handle compound operators through desugaring

---

### Phase 5: Execution Tracing & Provenance (4-5 hours)

**Enables:** Debugging, data lineage, program understanding

#### Features
- Execution trace recording
- Term activation tracking
- Data provenance queries
- Forward slicing: "What does this input influence?"
- Backward slicing: "What influenced this output?"

#### Implementation
- Extend `Stack` with `ExecutionTrace`
- Record which terms executed
- Track data flow between terms
- Implement slicing algorithms

#### Example
```petal
# After execution
provenance = env.get_provenance(output_term)
# Returns: [term_id1, term_id2, ...] influencing output
```

---

### Phase 6: Automatic Differentiation (5-6 hours)

**Enables:** Gradient-based optimization, sensitivity analysis

#### Features
- Forward-mode or reverse-mode AD
- Gradient computation through program
- Differentiable operations
- Non-differentiable operation handling

#### Example (Future)
```petal
# Define a simple model
fn model(x) {
    let a = x * 2
    let b = a + 3
    b * b
}

# Compute gradients (once implemented)
grads = env.backpropagate(
    model_term,
    output_term,
    target_gradient
)
```

#### Implementation Steps
1. Extend `Value` with gradient tracking
2. Add `DifferentiableOp` wrapper to terms
3. Implement AD through term graph
4. Choose strategy (forward vs. reverse mode)
5. Handle non-differentiable operations

---

### Phase 7: Projections & Program Slicing (6-7 hours)

**Enables:** Focus on relevant code, cross-language editing, complexity reduction

#### Features
- Static program slicing (data/control flow)
- Dynamic slicing (for specific execution)
- Scenario-based projection (simplification for specific inputs)
- Bidirectional projection editing

#### Example (Future)
```petal
# Project program to show only what affects output
projection = env.project(
    program,
    ProjectionFocus::Backward(output_term)
)

# Result: simplified program showing relevant terms only
```

#### Implementation
1. Compute dependency graph
2. Implement forward/backward slice algorithms
3. Store projection as subset of terms
4. Map edits back to original program
5. Support cross-language mounting

---

### Phase 8: Live Editing (4-5 hours)

**Enables:** Interactive development, hot-reloading with state preservation

#### Features
- State reconciliation after code changes
- Term ID mapping through edits
- Live state transfer
- Structural similarity detection

#### Implementation
1. Implement state key tracking
2. Create edit diff algorithm
3. Map old state to new structure
4. Handle added/removed/modified terms
5. Handle structural changes (control flow modifications)

---

### Phase 9: WebAssembly FFI (3-4 hours)

**Enables:** Browser execution, embedding in web apps

#### Features
- WASM compilation support
- Object registry for FFI
- Memory management for WASM
- JavaScript interop

#### Implementation
1. Add wasm.rs with object registry
2. Export FFI functions
3. Memory allocation/deallocation
4. JSON serialization for data passing
5. WASM build target support

---

### Phase 10: Standard Library (2-3 hours per category)

**Enables:** Practical programming

#### Math Library
- Trigonometry: sin, cos, tan, atan2
- Rounding: floor, ceil, round
- Random: rand, seed

#### String Library
- split, join, substring
- trim, uppercase, lowercase
- regex (optional)

#### List Library
- map, filter, reduce, fold
- sort, reverse, unique
- zip, flatten

#### File I/O Library
- read_file, write_file
- read_line, parse_json

---

## Phases by Priority

### High Priority (Core Language)
1. ✅ Phase 1: Core Language
2. ✅ Phase 2: Variables, State, Functions
3. 🔄 **Phase 3: Loops** ← Recommended Next
4. Phase 5: Execution Tracing

### Medium Priority (Advanced Features)
5. Phase 4: Mutation & State
6. Phase 6: Automatic Differentiation
7. Phase 7: Projections & Slicing

### Lower Priority (Infrastructure)
8. Phase 8: Live Editing
9. Phase 9: WebAssembly FFI
10. Phase 10: Standard Library

---

## Detailed Implementation Guide for Phase 3: Loops

### Step 1: Add IR Terms

**File: src/lib.rs**

```rust
pub enum TermOp {
    // ... existing ...
    For {
        var: String,
        iter: usize,     // The iterable term
        body: usize,     // The loop body
    },
    While {
        cond: usize,     // The condition term
        body: usize,     // The loop body
    },
    Break,
    Continue,
}
```

### Step 2: Extend Parser

**File: src/parse.rs**

- Add `parse_for()` method recognizing `for ... in ... { ... }`
- Add `parse_while()` method recognizing `while ... { ... }`
- Integrate into `parse_statement()`

### Step 3: Implement Evaluator

**File: src/eval.rs**

```rust
TermOp::For { var, iter, body } => {
    // 1. Evaluate the iterable
    let iterable = eval_term(env, stack_key, *iter, program)?;

    // 2. Extract items from iterable
    let items = match iterable {
        Value::List(list) => list.borrow().clone(),
        // Other iterables
        _ => return Err("Not iterable"),
    };

    // 3. Iterate, binding loop variable
    let mut last_value = Value::Nil;
    for item in items {
        // Save bindings
        let saved = {
            let stack = env.get_stack(stack_key)?;
            stack.bindings.insert(var.clone(), item);
            true
        };

        // Execute body
        last_value = eval_term(env, stack_key, *body, program)?;

        // Restore bindings
    }

    Ok(last_value)
}
```

### Step 4: Add Sample Programs

- **19_loops_for.ptl** - Simple for loop
- **20_loops_sum.ptl** - For loop with accumulator pattern
- **21_loops_while.ptl** - While loop example
- **22_loops_nested.ptl** - Nested loops

### Step 5: Test

```bash
cargo build --release
./test_samples.sh  # Should pass all tests
```

---

## Architecture Notes

### Current Design Supports:

✅ Scoped bindings (variables)
✅ Persistent storage (state)
✅ Function abstraction
✅ Recursion for control flow

### Ready to Add:

- Loop constructs (just need iteration binding)
- Tracing (extend Stack with trace recording)
- Differentiation (add gradient tracking to Values)
- Slicing (compute term dependencies)
- Live editing (term ID mapping + state reconciliation)

### Foundation for Advanced Features:

- **Projection**: Already have term graph structure
- **Differentiation**: Already have recursive evaluation
- **Tracing**: Just need to record execution history
- **Live Editing**: Already have state isolation per stack

---

## Development Tips

### Testing Strategy
1. Write sample program first
2. Run with expected behavior
3. Implement feature
4. Add to test suite

### Debugging
```bash
# Run with RUST_BACKTRACE for error details
RUST_BACKTRACE=1 ./target/release/petal script.ptl

# Use simpler test cases to isolate issues
echo 'let x = 5; print(x)' > test.ptl
./target/release/petal test.ptl
```

### Performance Monitoring
```bash
# Check build time
time cargo build --release

# Check runtime
time ./target/release/petal samples/16_recursion.ptl
```

---

## Conclusion

The Petal implementation has a solid foundation with:
- ✅ Core language complete
- ✅ Variables and state working
- ✅ Functions with recursion
- 🔄 Ready for loops (Phase 3)
- 🔮 Architecture supports all planned features

**Recommended next step:** Implement Phase 3 (Loops) to unlock iteration patterns and enable more practical programming examples.

Estimated time to complete Phase 3: **1-2 hours**
