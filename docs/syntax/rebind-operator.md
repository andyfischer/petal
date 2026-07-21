# Rebind Operator (`@`)

The `@` operator is syntax sugar for an expression that rebinds an existing
variable name to a new expression.

This code:
```petal
let nums = [1]
append(@nums, 2)
```

Does the same thing as this:
```petal
let nums = [1]
nums = append(@nums, 2)
```

## Why does the language have the @ operator

The Petal language is dataflow-based and values are immutable. So it's extremely
common for Petal code to contain a series of expressions that update an existing
name to new results. Without the syntax sugar, many Petal programs would be filled
with lines that look like `name = something(name)`. This syntax helps cut down
the noise and support code that looks somewhat like languages that use in-place mutation.

## Details

### Nearest enclosing call

When calls are nested, `@var` binds to the **nearest enclosing call** — the call
it is a direct argument of. That call is what gets written back to `var`:

```petal
let b = 3
let r = inc(double(@b))
// desugars to:
//   b = double(b)     // @b's nearest enclosing call is double(...)
//   let r = inc(b)
print(b)   // 6  — only double ran back into b
print(r)   // 7  — inc applied to the updated b
```

So `@` rebinds the value produced by its immediate call, and any surrounding
calls see the already-updated variable.

### Statement-level rewrite

The rebind is lifted to the **nearest statement**. The update is inserted as an
assignment immediately before the statement containing the `@`, and the call
site is replaced by a reference to the variable.

```petal
let a = 3
if ready() then
    normalize(@a)   //  a = normalize(a)  — hoisted within this branch
end
```

### Limitations

Since Petal is still in a 0.x experimental state, the operator is currently
narrow in what it supports:

The operator is intentionally narrow in its first form:

- **One `@` per call.** `f(@a, @b)` would have to assign a single result to two
  variables, so it is rejected.
- **Must be a call argument at statement level.** A bare `@a` with no enclosing
  call (e.g. `let b = @a + 1`) has nothing to rebind and is rejected.
- **Deferred / conditional positions are left alone.** `@` inside a lambda body,
  a `match` arm, or a `while` *condition* is not rewritten, so it can never
  silently change evaluation order. Using it there is an error.

Each of these produces a clear error pointing at the `@`:

```text
Error: `@a` can only be used as an argument to a call at statement level
```

## See also

- Implementation: `rust/src/desugar.rs`.
