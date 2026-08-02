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

## Methods Overload Too

A [method](language-guide.md#methods) — `fn <Class>.<name>(receiver, …)` — is a
named `fn` declaration, so it overloads by exactly the same arity rule. The
receiver is an ordinary first parameter and **counts toward the arity**:

```petal
class Point
  x: int,
  y: int,
end

fn Point.shifted(p: Point, d: int)          // arity 2
  Point(p.x + d, p.y + d)
end
fn Point.shifted(p: Point, dx: int, dy: int) // arity 3
  Point(p.x + dx, p.y + dy)
end

print(Point(1, 2).shifted(5))     // { x: 6, y: 7 }
print(Point(1, 2).shifted(5, 6))  // { x: 6, y: 8 }
```

Each overload set is keyed by the *qualified* name, so the sets are independent
in both directions: `fn Other.shifted(…)` is a different set from
`fn Point.shifted(…)`, and a plain `fn shifted(…)` global is different from
both. The arity error names the method the same way:

```petal ignore
Point(1, 2).shifted(1, 2, 3)
// Error: Point.shifted() expects 2 or 3 arguments, got 4
```

## Error on Wrong Arity

Calling an overloaded function with an argument count that doesn't match any variant
produces a clear error listing the available arities:

```petal
fn add(a, b) a + b end
fn add(a, b, c) a + b + c end

add(1)  // Error: add() expects 2 or 3 arguments, got 1
```

The arity of a call is statically known, so `petal check` reports it before the
program runs — `` warning: `add` expects 2 or 3 arguments, got 1 `` — and
`petal check --strict` fails on it. Running is still what turns it into an
error; the check is a pre-flight, and it covers constructors and methods on the
same rule (a method's count excludes the receiver the call site does not write).

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
