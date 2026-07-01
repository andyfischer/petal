# Rebind Operator (`@`)

Petal values are immutable, so functions that "change" a value actually return
a new one. That produces a lot of `x = f(x)` — you call a function on a variable
and assign the result straight back to it:

```petal
let nums = [1, 2, 3]
nums = append(nums, 4)   // append returns a new list — rebind to keep it
```

The **rebind operator** `@` is shorthand for exactly this pattern. Prefixing an
argument with `@` means "assign the call's result back to this variable":

```petal
let nums = [1, 2, 3]
append(@nums, 4)   // same as: nums = append(nums, 4)
print(nums)        // [1, 2, 3, 4]
```

The general rule:

```petal
something(@var)    //  ≡   var = something(var)
```

## Nearest enclosing call

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

## Statement-level rewrite

The rebind is lifted to the **nearest statement**: the update is inserted as an
assignment immediately before the statement containing the `@`, and the call
site is replaced by a reference to the variable. This works in any position that
is evaluated once, unconditionally, at statement level — expression statements,
`let` / assignment values, `return`, `state` initializers, `for` iterables, and
the bodies of `if` / `while` (each recursed as its own statement scope):

```petal
let a = 3
if ready() then
    normalize(@a)   //  a = normalize(a)  — hoisted within this branch
end
```

Because the rewrite is purely a source-level transformation, `@` adds no runtime
machinery: `append(@nums, 4)` compiles to exactly what `nums = append(nums, 4)`
compiles to.

## Limits

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

- [Language Guide → Rebind Operator](Language_Guide.md#rebind-operator) — the
  short reference entry.
- Implementation: `rust/src/desugar.rs` (the rewrite) and the
  `test/at-arg-update/` script regression case.
