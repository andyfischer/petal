# Declaration order

Which top-level declarations can be used above the line that writes them, and
what happens when one cannot. A `.ptl` file runs top to bottom with no `main`,
so this is the one place where the order of a file matters for code that
otherwise reads the same. The walkthrough is in the
[Language Guide](../language-guide.md#declaration-order-top-level-fns-are-hoisted).

| Declaration | Usable above its line? |
|---|---|
| top-level `fn` (functions and methods) | yes, unless its body reads a value the file computes at run time ([below](#when-a-fn-is-not-hoisted)) |
| `class` | yes: the constructor and the type name |
| `enum` | yes: the name and its variants |
| `let` / `const` built only from literals | its *value* is in place before the first statement, so a hoisted `fn` may read it; its *name* is not in scope above the line |
| any other `let`, and every `var` and `state` | no |
| a `fn` inside a function body | no: it is a local binding, in scope from its line down |

Hoisting is about where a name is usable. How often a value is built is a
different question, answered by [static values](../features/static-values.md):
a top-level value that depends on no dynamic data is built once by a host that
runs the script every frame, whatever its position in the file.

## Functions, classes and enums

A top-level `fn` can be called above where it is written, so mutual recursion
needs no forward declaration:

```petal
fn is_even(n)
  if n == 0 then true else is_odd(n - 1) end
end

fn is_odd(n)
  if n == 0 then false else is_even(n - 1) end
end

print(is_even(10))  // true
```

A method (`fn Rect.area(r: Rect)`) is a top-level `fn` and is hoisted the
same way. `class` and `enum` are top-level declarations and are always
hoisted: code above the declaration can build an instance or name a variant.

```petal
print(Point(1, 2).x, Kind.a)    // 1 Kind.a

class Point
  x: int,
  y: int,
end

enum Kind
  a,
  b,
end
```

Both are errors anywhere but the top level of a file, so the question does not
arise for a nested one.

## Constant `let`s

A top-level `let` or `const` built only from literals, operators, lists and
records of them, enum variants and other such bindings is in place before the
file's first statement. A function that reads one is still hoisted:

```petal
print(width_of(3))   // 560

let NODE_W = 140
let GAP = NODE_W / 2

fn width_of(n)
  n * NODE_W + (n - 1) * GAP
end
```

The binding itself is not hoisted. Top-level code above `let NODE_W` that
names `NODE_W` directly fails `petal check` with ``undefined variable
`NODE_W` ``; only a hoisted function body may read it early.

## When a `fn` is not hoisted

The hoist never changes what a function captures, so a top-level `fn` stays
exactly where it is written when

- its body reads something the file computes at run time: a top-level `var`
  or `state`, or a `let` that is reassigned, or whose initializer calls a
  function or holds one (`let W = screen_width() / 4`); or
- a statement above it *reads* a name the `fn` shadows, so
  `let old_max = max` above `fn max` still reads the builtin `max`.

Until its line runs, such a function is `nil`. A top-level call to it from
above fails with `Cannot call nil`, and `petal check` says why first:

```petal ignore
print(fit(2))
let W = abs(-7)        // calls a function: computed at run time
fn fit(n)
  n * W
end
```

```
warning: call to `fit` before its declaration, which is further down this file
and cannot be hoisted: its body reads a value the file computes at run time,
so at this point `fit` is still nil. Move this call below the declaration of
`fit`.
```

It is a warning, so `petal check` exits 0 and `petal check --strict` exits 1.
The fix is to keep a helper that reads a computed value below that value and
above its first top-level call.

The second rule needs a read. A name being in scope is not enough: a plain
`import ui` binds hundreds of bare names in a module, and a library's own
`fn spinner` still wins for the calls in that module although `spinner` is
also a `ui` export. Only a read that captures the older meaning holds a
declaration back.

```petal
let old_max = max
fn max(a, b)
  "mine"
end
print(old_max(1, 2), max(1, 2))   // 2 mine
```

A call from *inside* another function body is always fine, hoisted or not:
that body runs after the whole file has.

## Local functions

A `fn` written inside a function body is not hoisted. It is a binding in that
body, in scope from its line down, and a call above it is
``unknown function `inner` ``. Write it above its first use, or move it to the
top level.

## See also

- [Language Guide: declaration order](../language-guide.md#declaration-order-top-level-fns-are-hoisted)
- [Classes](../language-guide.md#classes--methods) and
  [enums](../language-guide.md#enums), which are hoisted as declarations
- [`var`](var.md) and [inline state](../features/inline-state.md), which are
  never hoisted
- [`petal check`](../CLI.md#check--compile-without-running), where the
  warning comes from
