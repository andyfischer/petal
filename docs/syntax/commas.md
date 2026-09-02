# Commas

Every comma-separated construct in Petal needs a comma between adjacent
elements. Whitespace is not a separator, and neither is a newline. Two
elements with only blank space between them is a parse error.

```petal
let grid = [
    [0, 0, 1, 0],
    [0, 0, 1, 0],
]

let c = color(0, 1, 2)
```

## The rule

1. A comma is required between two elements.
2. A trailing comma before the closing delimiter is allowed.
3. Newlines around a comma do not matter. A list may wrap across as many
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

The error names the construct and points at the element that needed a comma
before it:

```text
Error: Expected ',' between list elements [line 3, column 5]
  |
3 |     2
  |     ^
```

## Where it applies

| Construct             | Example                |
| --------------------- | ---------------------- |
| List literals         | `[1, 2, 3]`            |
| Function call args    | `color(0, 1, 2)`       |
| Function parameters   | `fn f(a, b, c)`        |
| Record literals       | `{ x: 1, y: 2 }`       |
| Record patterns       | `{ x: a, y: b }`       |
| List patterns         | `[a, b, ...rest]`      |
| Enum variant patterns | `Point(x, y)`          |
| Enum declarations     | `enum E A, B, C end`   |
| Class declarations    | `class P x: int, end`  |

Multi-line `enum` and `class` bodies carry their commas too:

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

Because `end` closes these bodies rather than a bracket, a body that runs past
its `end` reports ``Expected ',' between class fields, or `end` to close class
`Point` ``.

### Not comma-separated

- Blocks (`then … end`, `do … end`, function bodies) are sequences of
  statements separated by newlines. No commas.
- JSX attributes are separated by whitespace, HTML-style:
  `<div class="x" id={y}/>`.

## Unary minus

Because a comma always ends an element, `-` never has to guess from spacing
whether it is negation or subtraction. It is negation when it appears where an
expression is expected, and subtraction when it appears between two
expressions:

```petal
let x = 1
let y = 2
print([1, -2])          // [1, -2] — negation
print(len([1, 2]) - 1)  // 1 — subtraction
print(x -y)             // -1 — subtraction
print(x - y)            // -1 — identical
print(x-y)              // -1 — identical
```

Negative literals in patterns work (`when -1 -> …`), as does negation in any
argument slot: `f(a, -b)`.
