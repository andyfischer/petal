# Petal Parser Implementation Milestones

This directory contains a progressive series of code samples designed to guide the implementation of the Petal language parser. Each file represents a milestone that builds upon the previous ones, starting with the most basic syntax and gradually introducing more complex language features.

## Implementation Strategy

The files are designed to be implemented **in order**, with each milestone adding new parser capabilities while ensuring all previous milestones continue to work. This approach allows for:

- **Incremental development** - Build the parser step by step
- **Continuous testing** - Verify each milestone works before moving to the next
- **Clear progress tracking** - Know exactly which features are implemented
- **Regression prevention** - Ensure new features don't break existing ones

## Milestone Progression

### 📝 **01_literals.ca** - Basic Literal Values
**Parser Requirements:**
- Numeric literals (integers, floats, scientific notation, hex, binary)
- String literals (basic strings, empty strings)
- Boolean literals (`true`, `false`)
- Null literal (`null`)
- Color literals (`:FF0000` syntax)
- Negative numbers

**What to implement:**
- Lexer for all literal token types
- Basic expression parsing for literals
- Value representation for each literal type

### 🔤 **02_variables.ca** - Variables and Assignment
**Parser Requirements:**
- Variable declarations (`let x = value`)
- Variable references (identifiers)
- Variable assignment (`x = new_value`)

**What to implement:**
- `let` keyword parsing
- Identifier parsing and validation
- Assignment operator (`=`)
- Symbol table or variable storage
- Scope management (basic)

### ➕ **03_expressions.ca** - Operators and Expressions
**Parser Requirements:**
- Arithmetic operators (`+`, `-`, `*`, `/`, `%`, `**`)
- Comparison operators (`>`, `<`, `>=`, `<=`, `==`, `!=`)
- Logical operators (`&&`, `||`, `!`)
- Operator precedence and associativity
- Parentheses for grouping

**What to implement:**
- Operator precedence table
- Expression parsing with precedence climbing or shunting yard
- Unary and binary operator handling
- Expression evaluation

### 📦 **04_collections.ca** - Arrays and Objects
**Parser Requirements:**
- Array literals (`[]`, `[1, 2, 3]`, `[1 2 3]`)
- Object literals (`{}`, `{key: value}`)
- Nested collections
- Property access (`obj.field`, `obj["key"]`)
- Array indexing (`arr[0]`, `arr[x][y]`)

**What to implement:**
- Collection literal parsing
- Optional comma handling
- Property access operators (`.` and `[]`)
- Nested collection support

### 🔧 **05_functions.ca** - Function Calls and Declarations
**Parser Requirements:**
- Function calls (`func(args)`)
- Optional commas in function calls
- Named parameters (`func(name: value)`)
- Method calls (`obj.method()`)
- Function declarations (`func name() {}`)
- Function parameters and return types
- Body expressions vs. block bodies

**What to implement:**
- `func` keyword parsing
- Parameter list parsing (with optional types)
- Function call argument parsing
- Named argument syntax
- Return type annotations (`->`)
- Function body parsing (block vs. expression)

### 🔀 **06_control_flow.ca** - Control Flow Structures
**Parser Requirements:**
- If statements (`if`, `else if`, `else`)
- If expressions (returning values)
- For loops (`for x in iterable`)
- While loops (`while condition`)
- Loop expressions (returning collections)
- Nested control structures

**What to implement:**
- Control flow keywords (`if`, `else`, `for`, `while`, `in`)
- Condition parsing
- Block vs. expression bodies for control flow
- Loop iteration syntax
- Control flow expression evaluation

### 🎯 **07_pattern_matching.ca** - Pattern Matching
**Parser Requirements:**
- Match expressions (`match value { ... }`)
- Pattern syntax (`->` instead of `=>`)
- Literal patterns, variable patterns, wildcard (`_`)
- Guard clauses (`if` conditions in patterns)
- Destructuring patterns (objects, arrays)
- Complex pattern matching

**What to implement:**
- `match` keyword and block parsing
- Pattern syntax parsing
- Arrow operator (`->`) for patterns
- Guard clause parsing (`if` in patterns)
- Destructuring pattern syntax
- Pattern matching evaluation logic

### 🌊 **08_dataflow.ca** - Dataflow Programming
**Parser Requirements:**
- Dataflow operator (`@`)
- Function chaining (`data @ func1() @ func2()`)
- Object updates (`obj @ { field: value }`)
- Multiline dataflow chains
- Mixed dataflow and object updates

**What to implement:**
- `@` operator parsing and precedence
- Dataflow chain parsing
- Object update syntax (`obj @ { updates }`)
- Method chaining vs. dataflow distinction
- Dataflow evaluation and transformation

### 🔗 **09_lambda_functions.ca** - Lambda Functions
**Parser Requirements:**
- Lambda syntax (`func(x) => expr`)
- Multi-line lambda bodies
- Lambda captures and closures
- Higher-order functions
- Immediately invoked lambdas

**What to implement:**
- Lambda expression parsing
- Arrow syntax (`=>`) for lambdas
- Lambda body parsing (expression vs. block)
- Closure capture analysis
- Lambda evaluation and scope handling

### 🏗️ **10_structs_enums.ca** - Type Definitions
**Parser Requirements:**
- Struct definitions (`struct Name { fields }`)
- Struct instantiation (`Name{field: value}`)
- Method definitions (`func Type.method()`)
- Enum definitions (`enum Name { variants }`)
- Enum with data (`Variant(data)`)
- Generic type parameters (`<T>`)

**What to implement:**
- `struct` and `enum` keywords
- Type definition parsing
- Field and variant parsing
- Generic parameter syntax (`<T>`)
- Type instantiation syntax
- Method definition syntax

### ⚡ **11_state_management.ca** - State Keyword
**Parser Requirements:**
- State declarations (`state var = value`)
- State in different scopes (functions, loops, conditionals)
- State persistence semantics
- Complex state structures

**What to implement:**
- `state` keyword parsing
- State variable declaration syntax
- State scoping rules
- State persistence mechanism
- Integration with control flow

### 🚀 **12_advanced_features.ca** - Advanced Syntax
**Parser Requirements:**
- String interpolation (`"${variable}"`)
- Multiline strings (`"""..."""`)
- Destructuring assignment (`let [x, y] = array`)
- Complex nested patterns
- Advanced dataflow combinations
- All features working together

**What to implement:**
- String interpolation parsing
- Multiline string handling
- Destructuring assignment syntax
- Complex pattern combinations
- Integration of all language features
- Error handling and recovery

## Testing Strategy

For each milestone:

1. **Parse successfully** - The parser should handle all syntax in the file
2. **Generate correct AST** - The abstract syntax tree should represent the code accurately
3. **Evaluate correctly** - If implementing evaluation, the code should execute as expected
4. **Handle errors gracefully** - Invalid variations should produce clear error messages

## Implementation Tips

### Start Simple
- Begin with a basic recursive descent parser
- Focus on getting the syntax right before optimization
- Use clear, descriptive error messages

### Build Incrementally
- Implement each milestone completely before moving on
- Run all previous milestone tests when adding new features
- Keep the codebase clean and well-documented

### Handle Edge Cases
- Test with invalid syntax variations
- Consider operator precedence carefully
- Plan for future language extensions

### Documentation
- Document the grammar as you implement it
- Keep examples of working and failing cases
- Maintain a changelog of implemented features

## Parser Architecture Suggestions

### Recommended Structure
```
src/parser/
├── lexer.cpp          # Tokenization
├── parser.cpp         # Main parsing logic
├── ast.cpp           # AST node definitions
├── precedence.cpp    # Operator precedence
└── error.cpp         # Error handling
```

### Key Components
- **Lexer**: Convert source text to tokens
- **Parser**: Build AST from tokens
- **AST**: Represent program structure
- **Evaluator**: Execute the AST (optional)
- **Error Handler**: Provide clear error messages

## Future Extensions

Once all milestones are complete, consider:
- **Performance optimization** - Faster parsing algorithms
- **Better error recovery** - Continue parsing after errors
- **IDE integration** - Language server protocol support
- **Debugging support** - Source maps and breakpoints
- **Module system** - Import/export functionality

This milestone-based approach ensures a solid foundation for the Petal language implementation while providing clear goals and measurable progress.