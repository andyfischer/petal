# Indentation and blocks

Indentation delimits blocks. A block starts after its opener (`then`, `else`,
`do`, the `->` of a `match` arm, or the header of a `fn`, `class`, `enum` or
`match`) and ends where the indentation ends. `end` still closes a block, and
is now optional.

```petal
fn clamp_all(xs, lo, hi)
  for x in xs do
    if x < lo then lo
    else if x > hi then hi
    else x

print(clamp_all([1, 5, 9], 2, 8)) # [2, 5, 8]
```

The same program with its `end`s is still valid, and the two can be mixed block
by block:

```petal
fn clamp_all(xs, lo, hi)
  for x in xs do
    if x < lo then lo
    else if x > hi then hi
    else x
    end
  end
end

print(clamp_all([1, 5, 9], 2, 8)) # [2, 5, 8]
```

## The two forms of a body

What follows an opener decides the form.

**A line break opens an indented block.** The next line of code must be
indented further than the line the construct starts on. Its column is the
block's column: every statement of the block starts at that column, and the
block ends at the first line that starts left of it.

```petal
let n = 3
if n > 2 then
  print("big") # big
  print("really") # really
print("done") # done
```

**Code on the opener's line is a one-line body.** It runs to the end of the
line.

```petal
fn double(x) x * 2
let label = if double(2) > 3 then "big" else "small"
for i in range(0, 2) do print(i)
print(label) # big
```

A one-line body extends as far as the expression reaches, so
`if c then 1 else 2 + 3` is `if c then 1 else (2 + 3)`. Write the `end` to stop
it sooner: `if c then 1 else 2 end + 3`.

Comments and blank lines play no part in any of this: only lines of code are
measured. Indent with spaces; a tab in the indentation of a line that is
measured is an error.

## `end`

`end` closes the innermost open block.

- On a line of its own it must sit at the column of the line that opened the
  block. Anything else is an error, so indentation and `end`s cannot disagree.
- On the same line as the block's last statement it closes it there, which is
  how a one-liner is written with an `end`: `if c then a else b end`.

`end` is still the way to do three things that indentation alone cannot:

```petal
let xs = [3, 1, 2]

# Two statements in a one-line body.
if len(xs) > 2 then print("a"); print("b") end # a b

# Go on with the expression after a block.
let s = for x in xs do
  str(x)
end |> join("-")
print(s) # 3-1-2

# Two block lambdas in one call.
fn twice(f, g)
  f()
  g()
twice(fn()
  print("first") # first
end, fn()
  print("second") # second
end)
```

Without the `end`, a `;` after a one-line body is an error, because it could
end the body or continue it: `if c then a; b`.

## `else`, `else if` and `when`

A line that starts with `else` or `else if` continues the nearest open `if`
that starts at or left of its column. Both of these layouts work:

```petal
let t = "vec2"
let size = if t == "float" then 1
    else if t == "vec2" then 2
    else 0
let bucket = if size > 1 then
  "wide"
else
  "narrow"
print(size, bucket) # 2 wide
```

On one line, an `else` binds to the nearest `if`:
`if a then if b then 1 else 2` is `if a then (if b then 1 else 2)`.

The arms of a `match` sit at one column, under the `match` or indented from
it. An arm's body is one line, or an indented block under the arm:

```petal
fn describe(n)
  match n
    when 0 -> "zero"
    when 1 ->
      let word = "one"
      word ++ "!"
    when _ -> "many"

print(describe(1)) # one!
```

## Inside brackets

A block inside `( )`, `[ ]` or `{ }` follows the same rule, measured from the
line that opens it. The closing bracket ends it:

```petal
fn each(xs, f)
  for x in xs do f(x)

each([1, 2], fn(x)
  let y = x * 10
  print(y) # 10 20
)
```

Everything else inside brackets is free-form, as before: line breaks between
elements and arguments mean nothing. A `{…}` hole among an element's children
follows the rule too, measured from the first token of its line.

## A snippet indented as a whole

The first statement sets the left margin, so source held in a string of a host
language, or pasted with its indentation, parses as it would at column 1:

```petal ignore
    let total = 0
    for x in xs do
      total += x
    print(total)
```

## Strings

Strings are opaque to layout. A string may span lines, and a line that begins
inside one is not measured; what follows the string on its last line belongs
to the statement the string is in.

## Errors

| Message | Cause |
|---|---|
| expected an indented block after `then` | The opener is followed by a line break, and the next line is not indented further. |
| unexpected indent | A line starts right of the block it is in. |
| this line is indented less than its block, but more than the enclosing one | A line starts between two block columns. |
| `end` at column 4 does not line up with the `for` block opened at line 14 (column 3) | An `end` on its own line is not at the column of the line that opened its block. |
| `;` after a one-line `then` is ambiguous | A one-line body with no `end` holds a `;`. |
| a tab in the indentation of this line | Indent with spaces. |

## Code written before this change

Code whose indentation agrees with its `end`s parses exactly as it did. Code
whose indentation disagrees with them (a body that is not indented, a line
dedented inside a block) is now an error, never a different program.
`petal fmt` reads such a file by its `end`s and indents it by them.
