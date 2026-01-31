# Petal Syntax Specification

This document defines the concrete syntax for the Petal programming language.

## Design Principles

- **Expression-oriented**: Everything is an expression that produces a value
- **Dataflow-first**: Data flow is explicit and traceable
- **Minimal syntax**: Simple, readable, no unnecessary ceremony
- **State-aware**: `state` keyword for inline state management

## Basic Syntax

### Literals

```petal
42              // Integer
3.14            // Float
true            // Boolean
false           // Boolean
nil             // Nil value
"hello"         // String
```

### Variables

Variables are immutable by default (functional style):

```petal
let x = 10
let y = x + 5
```

### Arithmetic and Operators

```petal
1 + 2           // Addition
10 - 3          // Subtraction
4 * 5           // Multiplication
20 / 4          // Division
5 % 2           // Modulo

x == y          // Equality
x != y          // Inequality
x < y           // Less than
x > y           // Greater than
x <= y          // Less than or equal
x >= y          // Greater than or equal

true && false   // Logical AND
true || false   // Logical OR
!true           // Logical NOT
```

### Functions

Functions are defined with `fn`:

```petal
fn add(a, b) {
    a + b
}

fn factorial(n) {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}
```

Last expression is the return value (no explicit `return` needed, though `return` is supported for early returns).

### Control Flow

**If expressions:**

```petal
let result = if x > 0 {
    "positive"
} else if x < 0 {
    "negative"
} else {
    "zero"
}
```

**Loops:**

```petal
// For loop with range
for i in range(0, 10) {
    print(i)
}

// For loop with list
for item in [1, 2, 3] {
    print(item)
}

// While loop
let i = 0
while i < 10 {
    print(i)
    i = i + 1
}
```

### State Management

The `state` keyword declares persistent state within a function:

```petal
fn counter() {
    state count = 0
    count = count + 1
    count
}

// First call returns 1, second returns 2, etc.
```

State persists across invocations within the same execution stack.

### Built-in Functions

```petal
print(value...)          // Print values to stdout
range(start, end)        // Generate range for iteration
len(list)                // Length of list/string
sqrt(x)                  // Square root
sin(x), cos(x)           // Trigonometric functions
floor(x), ceil(x)        // Rounding
abs(x)                   // Absolute value
random(min, max)         // Random number in range
int(x), float(x), str(x) // Type conversions
push(list, item)         // Add item to list
pop(list)                // Remove and return last item
```

### Collections

**Lists:**

```petal
let numbers = [1, 2, 3, 4, 5]
let first = numbers[0]
let length = len(numbers)
```

**Maps:**

```petal
let person = {
    name: "Alice",
    age: 30
}
let name = person.name
```

### Comments

```petal
// Single line comment

/* Multi-line
   comment */
```

## Grammar Summary

```ebnf
program     ::= statement*

statement   ::= let_stmt | fn_def | state_decl | expr_stmt

let_stmt    ::= "let" IDENT "=" expr
fn_def      ::= "fn" IDENT "(" params? ")" block
state_decl  ::= "state" IDENT "=" expr

expr_stmt   ::= expr

expr        ::= assignment
assignment  ::= IDENT "=" expr | logical_or
logical_or  ::= logical_and ( "||" logical_and )*
logical_and ::= equality ( "&&" equality )*
equality    ::= comparison ( ( "==" | "!=" ) comparison )*
comparison  ::= term ( ( "<" | ">" | "<=" | ">=" ) term )*
term        ::= factor ( ( "+" | "-" ) factor )*
factor      ::= unary ( ( "*" | "/" | "%" ) unary )*
unary       ::= ( "!" | "-" ) unary | call
call        ::= primary ( "(" args? ")" | "[" expr "]" | "." IDENT )*
primary     ::= literal | IDENT | "(" expr ")" | if_expr | for_expr | while_expr | block | list | map

if_expr     ::= "if" expr block ( "else" ( if_expr | block ) )?
for_expr    ::= "for" IDENT "in" expr block
while_expr  ::= "while" expr block
block       ::= "{" statement* expr? "}"

list        ::= "[" ( expr ( "," expr )* )? "]"
map         ::= "{" ( IDENT ":" expr ( "," IDENT ":" expr )* )? "}"

literal     ::= NUMBER | STRING | "true" | "false" | "nil"
```
