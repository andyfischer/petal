# Optional Commas

Petal allows commas to be **optional** in delimited lists. Elements may be
separated by commas, by newlines, or simply by whitespace (juxtaposition). This
is encouraged for dense, table-like data such as lists of literal numbers.

```petal
let piece_o = [
    [0 0  1 0  0 1  1 1]
    [0 0  1 0  0 1  1 1]
    [0 0  1 0  0 1  1 1]
    [0 0  1 0  0 1  1 1]
]

color(0 1 2)
```

## Where it applies

Comma-optional parsing is already implemented uniformly across every
delimited, comma-separated construct in the language:

| Construct                | Example                       | Parser fn (`rust/src/parse.rs`) |
| ------------------------ | ----------------------------- | ------------------------------- |
| List literals            | `[1 2 3]`                     | `parse_list_literal` (~708)     |
| Function call args        | `color(0 1 2)`                | `parse_arg_list` (~1127)        |
| Function parameters       | `fn f(a b c)`                 | `parse_param_list` (~292)       |
| Record literals          | `{ x: 1  y: 2 }`              | `parse_record_literal` (~726)   |
| Record patterns          | `{ x: a  y: b }`              | `parse_record_pattern` (~937)   |
| List patterns            | `[a b ...rest]`               | `parse_list_pattern` (~906)     |
| Enum variant patterns     | `Point(x y)`                  | `parse_pattern` (~843)          |
| Enum declarations         | `enum E A B C end`            | `parse_enum_decl` (~187)        |

All of these follow the same loop shape: parse an element, skip newlines, then
consume a comma *if one is present*. The comma is never required.

## How it works

The expression grammar is a layered precedence-climbing parser (lowest →
highest): `pipe → or → and → equality → comparison → concat → additive →
multiplicative → unary → postfix → primary`.

A list/arg loop simply calls `parse_expr` repeatedly until it hits the closing
delimiter:

```rust
// parse_list_literal, abridged
self.advance(); // consume '['
self.skip_newlines();
while !matches!(self.peek(), Token::RBracket | Token::Eof) {
    let elem = self.parse_expr()?;
    elements.push(elem);
    self.skip_newlines();
    if matches!(self.peek(), Token::Comma) {  // optional
        self.advance();
        self.skip_newlines();
    }
}
self.expect(&Token::RBracket)?;
```

Because elements are separated by "the next `parse_expr` simply starts again,"
the **separator is implicit**: whatever token `parse_expr` does not consume ends
the current element and begins the next. This is what makes whitespace-only
separation work — but it is also the source of every ambiguity below.

## Ambiguities and gotchas

The core rule to understand:

> An element is whatever a full expression greedily consumes. Whitespace is
> **not** a separator — it is invisible to the parser. Any token that can
> continue the current expression (an infix or postfix operator) will bind to
> the preceding element, even when surrounding whitespace makes it *look* like a
> new element is starting.

### 1. Minus is spacing-aware (the big one)

Negative numbers in a comma-less list are the one case that *needs* spacing to
disambiguate, so Petal makes `-` spacing-aware. A `-` that has **whitespace
before but none after** (e.g. `1 -2`) begins a new negated element; a `-` with
symmetric spacing (or none) is ordinary subtraction:

```petal
[1 -2]     // TWO elements: [1, -2]            (space before, none after)
[1 - 2]    // ONE element:  (1 - 2) == -1      (spaces both sides)
[1-2]      // ONE element:  (1 - 2) == -1      (no spaces)
[1- 2]     // ONE element:  (1 - 2) == -1      (space after only)
[10 -3 -1] // THREE elements: [10, -3, -1]
```

The rule applies **only inside juxtaposition contexts** — list literals and call
argument lists. Everywhere else, `-` is always subtraction regardless of
spacing, so ordinary code is unaffected:

```petal
let x = total -discount   // subtraction (statement context)
return a -b               // subtraction
if a -b > 0 then ...      // subtraction
```

Because the rule is scoped, nested non-list sub-expressions reset it — a grouping
paren or an index is a single expression, not a juxtaposition list:

```petal
[(1 -2)]   // ONE element: -1   (grouping → subtraction)
xs[3 -1]   // index 2           (index → subtraction)
```

This is implemented with a dedicated `Token::MinusPrefix` (emitted by the lexer
when a `-` is space-before/no-space-after) plus an `in_juxta` parser flag that is
true only while parsing list/argument elements. See `rust/src/lexer.rs` and
`parse_additive` / `parse_primary` in `rust/src/parse.rs`.

A newline also separates elements, so a `-` starting a new line is prefix
negation regardless of the spacing rule:

```petal
[1
 -2]       // TWO elements: [1, -2]
```

> **Caveat — `f(a -b)` means `f(a, -b)`.** Because call arguments are a
> juxtaposition context, asymmetric spacing splits them too. Write `f(a - b)` or
> `f(a-b)` when you mean a single subtracted argument.

`+` has no unary form and is *not* spacing-aware, so `[1 +2]` is `[1 + 2] == [3]`
(one element). Use a comma or a newline if you need `+`-led elements.

### 2. Postfix operators bind to the preceding element too

The same greediness applies to every postfix operator — indexing `[...]`,
call `(...)`, and field access `.`:

```petal
[x [0]]    // ONE element: x[0]    (index)   ❌ not [x, [0]]
[f (1)]    // ONE element: f(1)    (call)    ❌ not [f, (1)]
[p .q]     // ONE element: p.q     (field)   ❌ not [p, p.q]
```

So juxtaposition is only truly unambiguous for *atoms* (number/string/bool/ident
literals) that are not followed by an operator token. For the encouraged use
case — grids of literal numbers — this is exactly what you have, which is why
the tetris piece tables work cleanly.

### 3. Multi-line dangling operators

Because a trailing infix operator makes `parse_additive` continue across the
newline (it calls `skip_newlines` after the operator), a line that ends in an
operator silently merges with the next "element":

```petal
[a +
 b]        // ONE element: a + b
```

## Guidance

- **Recommended:** use comma-less juxtaposition for dense rows of literal
  numbers/atoms (`[0 0 1 0]`, `color(0 1 2)`, `[1 -2 -3]`). This is the intended
  sweet spot.
- For negative numbers, rely on spacing (`[1 -2]`) or be explicit with a comma
  (`[1, -2]`). Write subtraction with symmetric spacing (`a - b`).
- **Use a comma (or parentheses)** whenever an element is *followed* by `[`, `(`,
  or `.` (see §2) — those postfix operators are not spacing-aware and will bind
  to the preceding element.
- Prefer commas in hand-written, mixed-expression lists where readability
  matters more than density.

## Tests

Regression tests for this behavior live in
`rust/tests/optional_commas.rs` (lexer token classification, list/argument
juxtaposition, and the spacing-aware minus rule including its reset in grouping
and index contexts).
