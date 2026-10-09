# Implicit Return Values

A Petal function does not need a `return`. This page is the whole rule: what a
function yields when it falls off its end, how that reaches into `if`, `match`
and `for`, and the one declaration — `-> nil` — that turns it off.

The short version:

- A function's value is the value of the **last statement of its body**.
- That last statement is in **value position**, and value position is passed
  inward: to the last statement of each `if` branch and `match` arm, and it is
  what makes a `for` loop **collect a list** instead of running for its side
  effects.
- A function declared **`-> nil` has no implicit return**. Its last statement
  is an ordinary statement: nothing is returned, and a trailing loop builds no
  list.

## The tail of a function

```petal
fn add(a, b)
  a + b              // the last statement: this is what add returns
end

fn describe(n)
  let kind = if n % 2 == 0 then "even" else "odd" end
  "{n} is {kind}"    // a string, returned
end

print(add(2, 3))     // 5
print(describe(7))   // 7 is odd
```

Only the *last* statement counts. Every statement before it runs and has its
value dropped:

```petal
fn noisy(n)
  n * 100            // computed, then dropped
  n + 1              // returned
end

print(noisy(1))      // 2
```

What "the value of the last statement" is, by the kind of statement:

| The body ends in | The function returns |
|---|---|
| an expression — `a + b`, `f(x)`, a literal | that expression's value |
| an `if` / `match` | the value of whichever branch or arm ran (see below) |
| a `for` loop | a **list**: the loop collects (see [Loops](#loops)) |
| a `while` loop | `nil` — `while` has no value |
| nothing (an empty body) | `nil` |
| `return expr` / `return` | `expr` / `nil` |

A body that ends in a binding (`let x = 5`, `x = 9`, `set v = 7`) yields the
value that was bound. That falls out of how a body's value is read rather than
being something to design around: end the function with the name if you mean
to return it, so the next reader does not have to know this row.

## `if` and `match` pass the position inward

An `if` is an expression, and its value is the last statement of the branch
that ran. A `match` is the same, per arm. So when an `if` or `match` is a
function's tail, each branch's tail is the function's tail:

```petal
fn sign(n)
  if n > 0 then
    "positive"
  elsif n < 0 then
    "negative"
  else
    "zero"
  end
end

fn name_of(code)
  match code
    when 0 -> "ok"
    when 1 -> "warning"
    when _ do
      print("unknown code {code}")
      "error"          // a `do … end` arm yields its last statement
    end
  end
end

print(sign(-4))        // negative
print(name_of(1))      // warning
```

An `if` with **no `else`** yields `nil` when the condition is false:

```petal
fn label(n)
  if n > 0 then "positive" end
end

print(label(1))        // positive
print(label(-1))       // nil
```

The position is only passed inward when the `if`/`match` itself is in value
position. One written mid-body is a statement, and so are its branch tails:

```petal
fn total(xs)
  var sum = 0
  if len(xs) == 0 then
    print("empty")     // a statement: this `if` is not the tail
  end
  for x in xs do set sum = get sum + x end
  get sum
end

print(total([1, 2, 3]))   // 6
```

## Loops

A `for` loop has two behaviours, and which one it gets is decided entirely by
position.

**In statement position** it is a side-effect loop. It allocates nothing and
has no value.

**In value position** it is a *mapping*: it evaluates to a list holding the
value of its body's last statement, once per iteration.

```petal
for i in range(0, 3) do print(i) end          // statement: no list

let squares = for i in range(1, 4) do i * i end
print(squares)                                 // [1, 4, 9]
```

### Where a loop is captured

A loop is in value position — and collects — when its value is used:

| Position | Example |
|---|---|
| bound to a name | `let xs = for … end`, `xs = for … end` |
| `return`ed | `return for … end` |
| an argument | `len(for … end)` |
| a list element or record field | `[for … end]`, `{rows: for … end}` |
| interpolated | `"{for … end}"` |
| **the tail of a function** (its implicit return) | `fn f(xs) for x in xs do x * 2 end end` |
| **the tail of an `if` branch or `match` arm** whose value is used | below |
| **the tail of a collecting loop's body** | nested loops give nested lists |

```petal
fn doubled(xs)
  for x in xs do x * 2 end           // the function's tail: collects
end

fn rows(n)
  if n > 0 then
    for i in range(0, n) do i end    // branch tail of a tail `if`: collects
  else
    []
  end
end

fn grid(n)
  for row in range(0, n) do
    for col in range(0, n) do row * 10 + col end   // tail of a collecting body
  end
end

print(doubled([1, 2, 3]))   // [2, 4, 6]
print(rows(3))              // [0, 1, 2]
print(grid(2))              // [[0, 1], [10, 11]]
```

### Where a loop is not captured

Everywhere else. In each of these the loop is a side-effect loop and no list
is built:

- a loop at the **top level of a file** — a script's statements are never
  values, including the last one;
- a loop that is **not the last statement** of its body;
- a loop that is the tail of an `if` or `match` which is itself a statement;
- a loop in the body of a **side-effect loop**, or of a `while`;
- a loop that is the tail of a function declared **`-> nil`**.

```petal
fn report(xs)
  for x in xs do print(x) end        // not the tail: no list
  if len(xs) > 2 then
    for x in xs do print(x * 2) end  // tail of a statement `if`: no list
  end
  len(xs)
end

print(report([1, 2]))                // prints 1, 2, then 2
```

### What a collecting loop gathers

Each iteration contributes the value of the body's last statement, by the same
rules as a function's tail. So an `if` with no `else` contributes `nil` on the
iterations it skips — it does not filter. Use `continue` to leave an iteration
out, and `break` to stop with what has been gathered:

```petal
let padded = for i in range(0, 4) do
  if i % 2 == 1 then i end
end
print(padded)                        // [nil, 1, nil, 3]

let odds = for i in range(0, 6) do
  if i % 2 == 0 then continue end    // contributes nothing
  i
end
print(odds)                          // [1, 3, 5]

let firsts = for i in range(0, 100) do
  if i == 3 then break end           // stops; keeps what was gathered
  i
end
print(firsts)                        // [0, 1, 2]
```

`while` is statement-only. It has no collecting form and its value is `nil`
wherever it is written.

## `-> nil` turns the implicit return off

A function whose return type is declared `nil` has **no implicit return**. Its
body is compiled with the last statement in statement position, exactly like
every statement before it:

- whatever the last statement evaluates to is dropped, and the function yields
  `nil`;
- a trailing `for` is a side-effect loop — **no list is built**;
- the same goes for the tails of a trailing `if` or `match`.

```petal
fn squares(xs) -> list
  for x in xs do x * x end           // a mapping: returns [1, 4, 9]
end

fn show_all(xs) -> nil
  for x in xs do print(x) end        // a side-effect loop: returns nil
end

print(squares([1, 2, 3]))            // [1, 4, 9]
print(show_all([1, 2]))              // prints 1, 2, then nil
```

This is why it matters. Without the declaration, `show_all` would build a list
of whatever `print` returns on every call, hand it back, and have its caller
drop it. A function that ends in a loop run for its side effects should say
`-> nil`.

Only `nil` does this. The check is on the declared name and nothing else:

| Declaration | The tail is |
|---|---|
| no return type — `fn f(xs)` | the return value |
| `-> list`, `-> num`, `-> any`, a class name, … | the return value |
| `-> nil` | a statement; the function returns `nil` |

The declared type is not consulted to decide whether a loop "should" collect:
`-> num` on a function that ends in a loop still returns the list, and the
type checker reports the mismatch. Because a `-> nil` function's tail is not
its return value, the checker does not hold that tail to the declared type —
there is no "declares `nil` but returns `int`" warning for it.

`-> nil` applies to methods (`fn Rect.draw(self) -> nil`) and to each overload
variant on its own. It says nothing about functions nested inside: an inner
`fn` or lambda follows its own declaration.

A loop whose value is *used* inside a `-> nil` function still collects.
`-> nil` moves the tail out of value position; it does not change what value
position means:

```petal
fn summarize(xs) -> nil
  let doubled = for x in xs do x * 2 end    // bound to a name: a list
  print(doubled)
end

summarize([1, 2])                            // [2, 4]
```

## Explicit `return`

`return expr` leaves the function with `expr`; a bare `return` leaves with
`nil`. It exits the *function* from wherever it is written, including from
inside a loop, and a loop being `return`ed is in value position:

```petal
fn first_negative(xs)
  for x in xs do
    if x < 0 then return x end       // leaves first_negative
  end
  nil
end

print(first_negative([3, -1, -2]))   // -1
```

`return` is not affected by `-> nil`. A `return expr` in a `-> nil` function
still returns `expr`, and the type checker warns that the function declares
`nil` but returns something else. (A bare `return` is the early exit such a
function wants.)

## Lambdas

A lambda has nowhere to write a return type, so it **always** returns its
tail, by all the rules above. A loop that ends a lambda collects:

```petal
let twice = fn(x) x * 2 end
let spread = fn(n) for i in range(0, n) do i end end

print(twice(4))        // 8
print(spread(3))       // [0, 1, 2]
```

The `->` in the short lambda form introduces the body, not a return type:
`fn(a, b) -> a + b` is a lambda returning `a + b`. A `return` inside a lambda
returns from the lambda, not from the function around it.

To give a callback no result, end it in `nil`, or declare a named function
`-> nil` and pass that.

## Finding these in existing code

`petal suggest` reports every un-annotated function that ends in a loop, and
reads its call sites to say which declaration it wants:

```
app.ptl:12  fn draw_all
  suggest: -> nil
  because: ends in a `for` loop, which collects a list as its implicit
           return, but none of its 3 calls uses it — `-> nil` turns the
           implicit return off, so the loop builds no list
```

A function whose result some caller uses gets `-> list`. One that is never
called, or is `pub`, is reported with both options and left for the author.
See [`petal suggest`](CLI.md#suggest--suggest-safe-refactors-for-a-file).

## Where this lives

The compiler threads one flag — "is this statement's value used?" — through
`compile_stmts` (`core/src/compiler/stmt.rs`), `compile_if` / `compile_match`
(`compiler/expr.rs`) and `compile_loop_body` (`compiler/phi.rs`);
`compile_function` (`compiler/function.rs`) starts it at true, or at false for
a `-> nil` declaration. A `for` records the answer as the `collect` flag on its
loop term, which is what the bytecode lowers to `loop_collect`. The behaviour
is pinned by `core/tests/implicit_return.rs`.
