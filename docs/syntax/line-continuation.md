# Line continuation

A statement normally ends at the end of the line. Two things override that, so
a long expression can be broken across as many lines as it needs.

## An operator at the end of a line

Everything after a binary operator may move to the next line. The operator is
obviously unfinished, so the line break is ignored:

```petal
let total = 1 +
  2
print(total) // 3
```

## An operator at the start of a line

A line that *begins* with a binary operator continues the expression above it:

```petal
let total = 1
  + 2
  * 3
print(total) // 7
```

Precedence is unaffected by the layout — the example above is `1 + (2 * 3)`,
exactly as if it were written on one line.

This is what lets a long condition wrap where it reads best:

```petal
let ready = true
let paused = false
if ready
   && !paused
   && true then
  print("go") // go
end
```

Pipelines wrap the same way:

```petal
let n = [1, 2, 3]
  |> len()
print(n) // 3
```

### Which operators start a continuation

`+`, `*`, `/`, `%`, `++`, `|>`, `&&`, `||`, `??`, `==`, `!=`, `>`, `>=`, `<=`.

Two operators are deliberately excluded, because a line starting with either
one is already meaningful on its own:

- `-` — it would be read as negation. A line starting with `-` stays a fresh
  expression. Break after the `-` (or after the operator before it) instead.
- `<` — it opens a JSX element.

Blank lines and comment lines between the two halves are fine; the continuation
rule looks past them.
