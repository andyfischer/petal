# Commas

Petal requires a comma between adjacent elements of every delimited,
comma-separated construct. Whitespace is not a separator, and neither is a
newline: two elements with nothing but blank space between them is a **parse
error**.

```petal
let piece_o = [
    [0, 0, 1, 0, 0, 1, 1, 1],
    [0, 0, 1, 0, 0, 1, 1, 1],
    [0, 0, 1, 0, 0, 1, 1, 1],
    [0, 0, 1, 0, 0, 1, 1, 1],
]

let c = color(0, 1, 2)
```

## The rule

1. **A comma is required between two elements.** Anything else — a space, a
   tab, a newline — is not a separator.
2. **A trailing comma before the closing delimiter is allowed** (and optional).
3. **Newlines around a comma are insignificant.** A list may wrap across as many
   lines as it likes, as long as each element but the last is followed by a
   comma.

```petal
let a = [1, 2, 3]      // fine
let b = [1, 2, 3,]     // fine — trailing comma
let c = [              // fine — one element per line
    1,
    2,
    3
]
```

```petal ignore
let d = [1 2 3]        // ERROR: Expected ',' between list elements
let e = [              // ERROR: a newline is not a comma
    1
    2
]
```

The diagnostic names the construct and points at the element that should have
been preceded by a comma, with the same caret block a type warning and a
runtime error carry:

```text
Error: Expected ',' between list elements [line 3, column 5]
  |
3 |     2
  |     ^
```

## Where it applies

Uniformly, across every delimited, comma-separated construct in the language:

| Construct             | Example                | Parser fn (`rust/src/parse.rs`) |
| --------------------- | ---------------------- | ------------------------------- |
| List literals         | `[1, 2, 3]`            | `parse_list_literal`            |
| Function call args    | `color(0, 1, 2)`       | `parse_arg_list`                |
| Function parameters   | `fn f(a, b, c)`        | `parse_param_list`              |
| Record literals       | `{ x: 1, y: 2 }`       | `parse_record_literal`          |
| Record patterns       | `{ x: a, y: b }`       | `parse_record_pattern`          |
| List patterns         | `[a, b, ...rest]`      | `parse_list_pattern`            |
| Enum variant patterns | `Point(x, y)`          | `parse_pattern`                 |
| Enum declarations     | `enum E A, B, C end`   | `parse_enum_decl`               |
| Class declarations    | `class P x: int, end`  | `parse_class_decl`              |

All of them share one helper, `Parser::expect_element_separator`, so the rule
cannot drift between constructs.

The two keyword-delimited bodies are included, and they read alike — a
multi-line `enum` body and a multi-line `class` body both carry their commas:

```petal
enum Shape
    Circle(radius),
    Rect(w, h),
    Unit,
end

class Point
    x: int,
    y: int,
end
```

Because `end` closes them rather than a bracket, their diagnostic names it: a
body that runs past its `end` reports
``Expected ',' between class fields, or `end` to close class `Point` `` instead
of blaming whatever line the parser eventually choked on.

### Not comma-separated

Two constructs look adjacent but are *not* covered, because they are not
comma-separated in the first place:

- **Blocks** (`then … end`, `do … end`, function bodies) are newline-separated
  statement sequences. No commas.
- **JSX attributes** are HTML-style, separated by whitespace:
  `<div class="x" id={y}/>`.

## Why

The former rule made commas optional and let whitespace separate elements
(`[0 0 1 0]`, `color(0 1 2)`). It read well for dense numeric grids, but the
separator was implicit — an element was "whatever a full expression greedily
consumed" — which made the surface syntax quietly ambiguous:

- `[x [0]]` was `[x[0]]` (an index), not two elements;
- `[f (1)]` was `[f(1)]` (a call), not two elements;
- `[p .q]` was `[p.q]` (a field access), not two elements;
- `[a +\n b]` was one element, `a + b`.

Worst of all, `-` had to be made **spacing-aware** to let `[1 -2]` mean two
elements while `[1 - 2]` meant subtraction — so `f(a -b)` silently meant
`f(a, -b)`. That is gone: `-` is now an ordinary token whose meaning is fixed by
its position in the grammar.

## Unary minus, today

`-` is prefix negation when it appears where an expression is expected, and
subtraction when it appears between two expressions. Spacing is irrelevant:

```petal
let x = 1
let y = 2
print([1, -2])      // negation:    a comma ends the previous element
print(len([1, 2]) - 1)  // subtraction
print(x -y)         // subtraction — 1 - 2
print(x - y)        // subtraction — identical
print(x-y)          // subtraction — identical
```

Negative literals in patterns keep working (`when -1 -> …`), as does negation in
any argument slot: `f(a, -b)`.

## Tests

Regression tests live in [`rust/tests/required_commas.rs`](../../rust/tests/required_commas.rs):
juxtaposition is a hard error in each of the eight constructs, a newline is not
a substitute, comma-separated and trailing-comma forms still parse, error
positions point at the offending element, and `-` is subtraction or negation by
grammar position alone.
