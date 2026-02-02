# Petal Programming Language - Rust Implementation

A working implementation of the Petal programming language in Rust, designed around dataflow-first semantics, first-class state management, projectional views, and live editing capabilities.

## Overview

This is a functional interpreter for Petal that demonstrates the core language concepts. The implementation includes:

- **Lexer & Parser** - Full tokenization and abstract syntax tree construction
- **Term-Based IR** - Programs represented as directed acyclic graphs (DAGs) of terms
- **Evaluator** - Recursive evaluation with proper operator precedence and type coercion
- **Built-in Functions** - Core library functions for I/O, type conversion, list operations
- **Type System** - Dynamic typing with int, float, string, bool, list, and map types
- **REPL** - Interactive read-eval-print loop for exploration

## Building

```bash
cargo build --release
```

The binary is created at `./target/release/petal`

## Usage

### Running Scripts
```bash
./target/release/petal samples/01_hello.ptl
```

### Interactive REPL
```bash
./target/release/petal repl
```

### Test All Samples
```bash
./test_samples.sh
```

## Language Syntax

### Basic Values

```petal
# Integers
42
-17

# Floating point
3.14
-2.5

# Booleans
true
false

# Strings (with escape sequences)
"Hello, World!"
"Line 1\nLine 2"

# Special value
nil

# Lists
[1, 2, 3]
[1, "two", true]

# Maps
{key: "value", count: 42}
```

### Operators

**Arithmetic**: `+`, `-`, `*`, `/`, `%`
```petal
print(2 + 3)      # 5
print(10 - 4)     # 6
print(3 * 7)      # 21
print(20 / 4)     # 5
print(17 % 5)     # 2
```

**Comparison**: `==`, `!=`, `<`, `>`, `<=`, `>=`
```petal
print(5 == 5)     # true
print(3 < 5)      # true
print(10 >= 10)   # true
```

**Logical**: `&&`, `||`, `!`
```petal
print(true && true)    # true
print(false || true)   # true
print(!false)          # true
```

### Control Flow

**If-Else**:
```petal
if x > 0 {
    print("positive")
} else {
    print("non-positive")
}
```

**Range** (creates a list of integers):
```petal
print(range(0, 5))    # [0, 1, 2, 3, 4]
```

### Built-in Functions

**I/O**:
- `print(value)` - Print value to stdout

**Type Conversion**:
- `to_string(x)` - Convert to string
- `to_int(x)` - Convert to integer
- `to_float(x)` - Convert to float

**Collections**:
- `len(x)` - Length of string, list, or map
- `range(start, end)` - Create list of integers from start to end (exclusive)
- `push(list, value)` - Add element to list
- `pop(list)` - Remove and return last element

**List Indexing**:
```petal
let items = [10, 20, 30]
print(items[0])       # 10
print(items[2])       # 30
```

## Features Implemented

### ✅ Core Language Features

- [x] **Lexer & Tokenization** - Full lexical analysis with keywords, operators, literals
- [x] **Parser** - Recursive descent parser building term-based IR
- [x] **Expression Evaluation** - All operators with correct precedence
- [x] **Type System** - Dynamic typing with 8 value types
- [x] **Arithmetic** - All basic math operations with type coercion
- [x] **Comparisons** - Equality and ordering comparisons
- [x] **Logic** - Boolean operators with short-circuit evaluation
- [x] **Collections** - Lists and maps with indexing and field access
- [x] **Control Flow** - If-else branching
- [x] **String Operations** - Concatenation, length
- [x] **Type Conversions** - Explicit conversion between types
- [x] **Comments** - Line comments with `#`

### ✅ Built-in Functions

- [x] `print()` - Output values
- [x] `len()` - Collection/string length
- [x] `range()` - Integer range generation
- [x] `to_string()`, `to_int()`, `to_float()` - Type conversion
- [x] `push()`, `pop()` - List manipulation
- [x] Field access - `.field` notation
- [x] Index access - `[index]` notation

### ⚠️ Partial Implementation

- [ ] **Variable Binding** - `let` keyword parsed but not scoped
- [ ] **State** - `state` keyword recognized but not persistently stored
- [ ] **Functions** - `fn` keyword recognized but user-defined functions not fully implemented
- [ ] **Loops** - `for`, `while` keywords recognized but not fully implemented
- [ ] **Dataflow Tracking** - IR supports dataflow but no provenance system yet

### 🔮 Not Yet Implemented

- [ ] Automatic Differentiation / Back-propagation
- [ ] Projectional Views / Program Slicing
- [ ] Live Editing with State Reconciliation
- [ ] Execution Tracing for Provenance
- [ ] WebAssembly FFI Layer
- [ ] Standard Library (beyond core builtins)

## Implementation Architecture

### Core Data Structures

**Value** - Runtime representation
```rust
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<HashMap<String, Value>>>),
    Function(Function),
}
```

**Term** - IR node in the program graph
```rust
pub struct Term {
    pub id: usize,
    pub op: TermOp,           // The operation
    pub inputs: Vec<usize>,   // Input terms (dataflow edges)
}
```

**Program** - Collection of terms
```rust
pub struct Program {
    pub terms: Vec<Term>,
    pub entry_term: usize,
}
```

**Env** - Global environment
- Stores all programs and stacks
- Manages program loading and execution
- Provides APIs for runtime inspection

### Evaluation Strategy

The evaluator uses a **recursive descent approach** with eager evaluation:

1. Each term is evaluated recursively
2. Input terms are evaluated on-demand
3. Results are computed directly without explicit stack frames
4. Control flow (if/else) follows the dataflow graph

## Sample Programs

See `samples/` directory for complete working examples:

- `01_hello.ptl` - Hello World
- `02_arithmetic.ptl` - All arithmetic operations
- `03_comparisons.ptl` - Comparison operators
- `03_variables.ptl` - Direct evaluation (without variable binding)
- `04_if_else.ptl` - Conditional branching
- `05_logic.ptl` - Logical operators
- `06_lists.ptl` - Lists and range generation
- `07_types.ptl` - Type conversions
- `08_floats.ptl` - Floating-point operations
- `09_strings.ptl` - String concatenation
- `10_complex_expr.ptl` - Operator precedence
- `11_nested_if.ptl` - Nested conditionals
- `12_comprehensive.ptl` - Full feature demonstration

## Testing

All samples can be tested with:
```bash
./test_samples.sh
```

Individual tests:
```bash
./target/release/petal samples/01_hello.ptl
./target/release/petal samples/02_arithmetic.ptl
# ... etc
```

## Pain Points & Challenges

### 1. **Variable Scoping**
The current implementation evaluates expressions in a stateless manner. Proper variable binding would require:
- Extending the IR to include Let/Var terms with body scoping
- Thread a variable binding context through the evaluator
- Handle shadowing and nested scopes

**Solution Path**: Implement an environment stack in the evaluator to track variable bindings.

### 2. **State Management**
The goals emphasize inline state that persists across function invocations. The current implementation:
- Recognizes `state` keyword during parsing
- Does not store state persistently
- Would need per-execution-context state storage

**Solution Path**: Add a state dictionary to Stack, with state_key generation based on lexical position.

### 3. **Control Flow & Loops**
For-loops and while-loops require:
- Iterator protocol or range expansion
- Loop variable binding per iteration
- Break/continue support

**Solution Path**: Implement loop expansion - convert for-loops to range iteration with state tracking.

### 4. **User-Defined Functions**
Function definitions are parsed but not evaluated. Would need:
- Function parameter binding
- Closure capture (if supporting closures)
- Recursion handling

**Solution Path**: Store function objects in the environment and evaluate calls by entering new stack frames with parameter bindings.

### 5. **Dataflow Provenance Tracking**
The IR supports dataflow edges, but provenance tracking requires:
- Recording which inputs influenced each computed value
- Maintaining execution traces
- Efficient forward/backward slicing queries

**Solution Path**: Extend Stack to track execution history, add methods for querying provenance.

## Feedback on Documentation

### What Was Helpful
- **Petal_Goals.md** - Excellent conceptual overview of the 4 pillars
- **Outline.md** - Clear architectural direction for implementation
- **Data structure docs** - Term, Value, Program definitions were precise

### What Needs Improvement

1. **Syntax Definition** - The docs describe semantics and goals but don't define concrete syntax. I had to invent a syntax inspired by Rust/TypeScript. A BNF or syntax guide would be invaluable.

2. **Live Editing Specifics** - The live editing discussion is conceptual but doesn't explain:
   - How state keys are assigned during parsing
   - How state is migrated when control flow changes
   - What constitutes "structural similarity" for state preservation

3. **Differentiation Details** - Back-propagation is discussed philosophically but needs:
   - Which operations support differentiation
   - How non-differentiable operations are handled
   - Example gradient computations through programs

4. **Projection/Slicing Algorithm** - The conceptual description is clear, but implementation needs:
   - Dependency analysis algorithm
   - Forward vs. backward slice algorithms
   - Concrete data structures for projections

5. **Function Semantics** - Unclear how functions interact with state:
   - Can state be defined inside functions?
   - What is function referential transparency with inline state?
   - How do closures work with state?

## Next Steps for Full Implementation

### Phase 1: Core Language (Current State)
- [x] Basic expressions and operators
- [x] Control flow (if-else)
- [x] Lists and collections
- [x] Type system and conversions
- [x] REPL and script execution

### Phase 2: State & Binding
- [ ] Variable binding with proper scoping
- [ ] State declarations with persistence
- [ ] State reconciliation during live edits
- [ ] Loop variables and for-loops

### Phase 3: Functions
- [ ] User-defined functions
- [ ] Function calls with argument binding
- [ ] Recursion support
- [ ] Closures (optional)

### Phase 4: Provenance & Tracing
- [ ] Execution trace recording
- [ ] Provenance queries (what influenced this value?)
- [ ] Dynamic slicing based on execution

### Phase 5: Differentiation
- [ ] Automatic differentiation (forward or reverse mode)
- [ ] Gradient computation and back-propagation
- [ ] Non-differentiable operation handling

### Phase 6: Projections
- [ ] Static program slicing
- [ ] Scenario-based projection (for specific execution)
- [ ] Bidirectional projection editing

## File Structure

```
petal_impl_5/
├── Cargo.toml                 # Project manifest
├── src/
│   ├── lib.rs                 # Core types & Env
│   ├── main.rs                # CLI & REPL
│   ├── parse.rs               # Lexer & Parser
│   ├── eval.rs                # Evaluator
│   └── types.rs               # Type system
├── samples/                   # Example .ptl scripts
├── test_samples.sh            # Test runner
└── README.md                  # This file
```

## Performance Notes

- Recursive evaluation means deep nesting can overflow the stack
- No optimization passes (constant folding, dead code elimination)
- No JIT or bytecode compilation
- String and list operations clone values (could use COW for efficiency)

## License

This is a reference implementation for the Petal language specification.
