# Rebind Operator (`@`)

`@` is shorthand for rebinding a variable to the result of a call.

This:

```petal
let nums = [1]
append(@nums, 2)
```

does the same as this:

```petal
let nums = [1]
nums = append(nums, 2)
```

## Why

Petal values are immutable, so code often updates a name to a new result:
`name = something(name)`. `@` cuts that noise and lets code read a little like
in-place mutation.

## Details

### Nearest enclosing call

When calls are nested, `@var` belongs to the call it is a direct argument of.
That call's result is what gets written back:

```petal
let b = 3
let r = inc(double(@b))
// desugars to:
//   b = double(b)
//   let r = inc(b)
print(b)   // 6 — only double ran back into b
print(r)   // 7 — inc applied to the updated b
```

### Where the assignment goes

The assignment is inserted just before the statement that contains the `@`,
and the call site becomes a plain reference to the variable:

```petal
let a = 3
if ready() then
    normalize(@a)   // a = normalize(a), inside this branch
end
```

### Limitations

- **One `@` per call.** `f(@a, @b)` would have to assign one result to two
  variables, so it is rejected.
- **Must be a call argument in a statement.** A bare `@a` with no enclosing
  call (such as `let b = @a + 1`) has nothing to rebind and is rejected.
- **Not inside deferred or conditional positions.** `@` inside a lambda body,
  a `match` arm, or a `while` condition is an error, so it can never quietly
  change evaluation order.
- **`let` bindings only.** `@` desugars to `x = f(x)`, which is not allowed on
  a [`var`](overview.md#var-set-and-get). Write `set x = f(x)` instead.

Each case gives an error pointing at the `@`:

```text
Error: `@a` can only be used as an argument to a call at statement level
```

`petal lint` never rewrites `x = f(x)` into `f(@x)`. The operator is something
you reach for deliberately.
