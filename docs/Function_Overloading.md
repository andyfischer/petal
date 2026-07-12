# Function Overloading (Multi-Arity)

Petal supports defining multiple functions with the same name but different numbers of
parameters. The correct variant is selected at runtime based on the argument count.

## Syntax

Define overloads by declaring the same function name multiple times with different parameter lists:

```petal
fn greet() print("hi") end
fn greet(name) print("hi", name) end
fn greet(a, b) print("hi", a, b) end

greet()           // hi
greet("world")    // hi world
greet("a", "b")   // hi a b
```

All variants must be declared at the same scope level.

## Recursion Across Overloads

Overloaded variants can call each other. A common pattern is a "convenience" variant that
delegates to a more general one with default arguments:

```petal
fn count(n) count(n, 0) end
fn count(n, acc)
    if n <= 0 then acc
    else count(n - 1, acc + 1) end
end

print(count(5))      // 5
print(count(3, 10))  // 13
```

## Closures Over Outer Variables

Overloaded variants capture variables from their enclosing scope, just like normal closures:

```petal
let prefix = "Dr."
fn title(name) title(prefix, name) end
fn title(pre, name) print(pre, name) end

title("Smith")        // Dr. Smith
title("Mr.", "Jones") // Mr. Jones
```

## Error on Wrong Arity

Calling an overloaded function with an argument count that doesn't match any variant
produces a clear error listing the available arities:

```petal
fn add(a, b) a + b end
fn add(a, b, c) a + b + c end

add(1)  // Error: add() expects 2 or 3 arguments, got 1
```

## Compilation

During compilation, the compiler prescans declarations to detect names with multiple
arities. Each variant is compiled as an independent closure with an internal name
`"name#arity"` (e.g. `greet#0`, `greet#1`, `greet#2`). Once all variants for a name are
compiled, a `MakeOverloadSet` term is emitted that bundles them together.

At runtime, the evaluator resolves an `OverloadSet` value by matching the call's argument
count against the stored `OverloadEntry` arities. The matching closure is then called
normally.

### IR Representation

- Each variant produces a `MakeClosure` term (with the internal `name#arity` name)
- A single `MakeOverloadSet` term takes all variant `MakeClosure` terms as inputs
- The `MakeOverloadSet` term is bound to the original function name in scope

### Key Data Structures

| Structure | Location | Purpose |
|-----------|----------|---------|
| `overloaded_fns: HashMap<String, usize>` | compiler | Maps overloaded names to variant count |
| `overload_variants: HashMap<String, Vec<TermId>>` | compiler | Collects closure term IDs per name |
| `OverloadEntry { arity, closure_id }` | program | Runtime mapping of arity to closure |
| `Value::OverloadSet(OverloadSetId)` | value | Runtime value representing the set |

## Limitations

- Dispatch is by **arity only** (argument count), not by type.
- Variants must differ in parameter count; two variants with the same arity are not supported.
- Overloading is only supported for named `fn` declarations, not lambdas.
