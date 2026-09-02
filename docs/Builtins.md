# Builtins Reference

The built-in functions available in every Petal program. For language syntax
(variables, control flow, functions, state, pattern matching), see the
[Language Guide](language-guide.md). The drawing and input builtins used by
`petal-ui` apps (`draw_rect`, `mouse_x`, ...) are covered in the
[petal-ui README](../petal-ui/README.md).

Builtins take **positional arguments only**. The parameter names in this
reference are for reading, not for calling: a
[named argument](language-guide.md#named-arguments) such as `append(xs, x: 1)`
is an error (`builtin 'append' does not accept named arguments`). Named
arguments work on `fn` declarations, methods and lambdas.

## I/O

### `print(args...)`

Prints arguments to stdout, separated by spaces, followed by a newline.

```petal
print("hello")           // hello
print(1, "and", 2)       // 1 and 2
print([1, 2], {a: 3})    // [1, 2] { a: 3 }
```

## Math

### `abs(x)`

Returns the absolute value.

```petal
abs(-5)    // 5
abs(3.2)   // 3.2
```

### `sqrt(x)`

Returns the square root.

```petal
sqrt(9.0)   // 3.0
sqrt(2.0)   // 1.4142135623730951
```

### `floor(x)`

Rounds down to the nearest integer (returns float).

```petal
floor(3.7)   // 3.0
floor(-1.2)  // -2.0
```

### `ceil(x)`

Rounds up to the nearest integer (returns float).

```petal
ceil(3.2)   // 4.0
ceil(-1.7)  // -1.0
```

### `round(x)` / `round(x, places)`

Rounds to the nearest integer, or — with `places` — to that many decimal
digits. Int-preserving: an `int` argument rounds to an `int`. A negative
`places` rounds to the left of the decimal point.

```petal
round(3.4)            // 3.0
round(3.6)            // 4.0
round(3.14159, 2)     // 3.14
round(1234.0, -2)     // 1200.0
round(7, 3)           // 7
```

### `min(a, b)`

Returns the smaller of two values. Works with numbers and strings.

```petal
min(3, 5)       // 3
min("a", "b")   // "a"
```

### `max(a, b)`

Returns the larger of two values. Works with numbers and strings.

```petal
max(3, 5)       // 5
max("a", "b")   // "b"
```

### `random(min, max)`

Returns a pseudo-random float in the range [min, max).

```petal
random(0.0, 1.0)    // 0.7342... (varies)
random(1.0, 10.0)   // 4.218...  (varies)
```

### `range(start, end)`

Returns a list of integers from `start` (inclusive) to `end` (exclusive).

```petal
range(0, 5)    // [0, 1, 2, 3, 4]
range(3, 7)    // [3, 4, 5, 6]
```

### `pi()`

Returns the mathematical constant π.

```petal
pi()    // 3.141592653589793
```

### `sin(x)` / `cos(x)` / `tan(x)`

Standard trigonometric functions. Input is in radians.

```petal
sin(0.0)         // 0.0
cos(pi())        // -1.0
```

### `atan2(y, x)`

Two-argument arctangent. Returns the angle in radians between the positive x-axis
and the point `(x, y)`.

```petal
atan2(1.0, 1.0)   // 0.7853981633974483 (π/4)
```

### `exp(x)` / `log(x)`

Natural exponential and natural logarithm.

```petal
exp(1.0)   // 2.718281828459045
log(exp(2.0))   // 2.0
```

### `pow(base, exp)`

Exponentiation.

```petal
pow(2.0, 10.0)   // 1024.0
pow(9.0, 0.5)    // 3.0
```

### `sign(x)`

Returns `-1`, `0`, or `1` depending on the sign of the argument.

```petal
sign(-5)     // -1
sign(0)      //  0
sign(3.2)    //  1.0
```

### `fract(x)`

Fractional part of a float (`x - floor(x)`).

```petal
fract(3.5)     // 0.5
fract(-1.25)   // 0.75
```

### `radians(degrees)` / `degrees(radians)`

Convert between degrees and radians.

```petal
radians(180.0)     // 3.141592653589793
degrees(pi())      // 180.0
```

## Creative Coding Math

Processing-style helpers for animation, layout, and generative code.

### `clamp(value, lo, hi)`

Constrain a value to the range `[lo, hi]`. Three `int` arguments give an
`int` (so the result can still be a list index); one `float` argument makes
the result a `float`.

```petal
clamp(15.0, 0.0, 10.0)   // 10.0
clamp(-3.0, 0.0, 10.0)   //  0.0
clamp(5.0, 0.0, 10.0)    //  5.0
clamp(9, 0, 5)           // 5     (int in, int out)
clamp(3, 0.0, 5)         // 3.0   (one float makes it float)
```

```petal ignore
xs[clamp(i, 0, len(xs) - 1)]   // a clamped index is still an index
```

### `lerp(a, b, t)`

Linear interpolation. `t=0` returns `a`, `t=1` returns `b`.

```petal
lerp(0.0, 100.0, 0.3)   // 30.0
lerp(10.0, 20.0, 0.5)   // 15.0
```

### `map_range(value, in_lo, in_hi, out_lo, out_hi)`

Remap a value from one range to another — for example a pixel coordinate in
`[0, width]` to an angle in `[0, 2π]`.

```petal
map_range(5.0, 0.0, 10.0, 100.0, 200.0)   // 150.0
map_range(0.5, 0.0, 1.0, -1.0, 1.0)       //   0.0
```

### `distance(x1, y1, x2, y2)` / `distance(v1, v2)`

Euclidean distance. Accepts either four scalars or two `vec2` values.

```petal
distance(0.0, 0.0, 3.0, 4.0)                // 5.0
distance(vec2(0.0, 0.0), vec2(3.0, 4.0))    // 5.0
```

### `mag(x, y)` / `mag(x, y, z)` / `mag(v)`

Vector magnitude. Accepts 2D or 3D scalars, or a single `vec2`.

```petal
mag(3.0, 4.0)          // 5.0
mag(vec2(3.0, 4.0))    // 5.0
```

### `smoothstep(edge0, edge1, x)`

Hermite interpolation between two edges — produces a smooth S-curve from 0 to 1.
Equivalent to GLSL's `smoothstep`.

```petal
smoothstep(0.0, 1.0, 0.5)   // 0.5
smoothstep(0.0, 1.0, 0.25)  // 0.15625
```

## Noise

### `noise(x)` / `noise(x, y)` / `noise(x, y, z)`

Perlin noise in 1D, 2D, or 3D. Returns a smooth value centered around 0. Ideal
for organic motion, terrain, clouds, and flow fields.

```petal
noise(0.5)                  // smooth 1D value
noise(0.3, 0.7)             // smooth 2D value
noise(0.1, 0.2, 0.3)        // smooth 3D value
```

### `noise_seed(seed)`

Sets the global noise seed for reproducibility. Takes an integer.

```petal
noise_seed(42)
```

## Extended Randomness

### `random_int(lo, hi)`

Random integer in the half-open range `[lo, hi)`.

```petal
random_int(0, 10)    // 0..9
```

### `choose(list)`

Returns a random element from a list, or `nil` for an empty list.

```petal
choose([1, 2, 3])             // one of 1, 2, or 3
choose(["red", "green"])      // "red" or "green"
```

## Color

All color builtins return an RGB record `{r: int, g: int, b: int}` with channels
in 0..255 — the same shape produced by the `#rrggbb` color literal.

### `hsv(h, s, v)`

Create an RGB color from Hue-Saturation-Value. All three arguments are in
`[0, 1]`; hue wraps, so `1.0` is the same as `0.0`. For hue in degrees, use
`hsv_deg`.

```petal
hsv(0.0, 1.0, 1.0)          // { r: 255, g: 0, b: 0 }   (red)
hsv(1.0 / 3.0, 1.0, 1.0)    // { r: 0, g: 255, b: 0 }   (green)
```

### `hsl(h, s, l)`

Create an RGB color from Hue-Saturation-Lightness. Same argument ranges as
`hsv`. For hue in degrees, use `hsl_deg`.

```petal
hsl(0.0, 1.0, 0.5)      // { r: 255, g: 0, b: 0 }
```

### `hsv_deg(h, s, v)` / `hsl_deg(h, s, l)`

Like `hsv` / `hsl` but with hue in **degrees** `[0, 360)`.

```petal
hsv_deg(120.0, 1.0, 1.0)    // { r: 0, g: 255, b: 0 }
hsl_deg(120.0, 1.0, 0.5)    // { r: 0, g: 255, b: 0 }
```

### `color_lerp(c1, c2, t)`

Interpolate between two RGB color records. `t=0` returns `c1`, `t=1` returns `c2`.

```petal
let red = hsv_deg(0.0, 1.0, 1.0)
let blue = hsv_deg(240.0, 1.0, 1.0)
color_lerp(red, blue, 0.5)   // { r: 128, g: 0, b: 128 }
```

## Vectors (2D)

Petal has a built-in `vec2` type backed by two f64s. It works with the usual
arithmetic operators (`+`, `-`, `*`, `/`) as well as the helpers below.

### `vec2(x, y)`

Construct a 2D vector.

```petal
let v = vec2(3.0, 4.0)
print(mag(v))    // 5.0
```

### `normalize(v)`

Return a vector pointing in the same direction as `v` with magnitude 1. The
zero vector normalizes to `vec2(0, 0)`.

```petal
normalize(vec2(3.0, 4.0))    // vec2(0.6, 0.8)
```

### `dot(a, b)`

Dot product of two `vec2` values.

```petal
dot(vec2(1.0, 0.0), vec2(0.0, 1.0))   // 0.0
dot(vec2(2.0, 3.0), vec2(4.0, 5.0))   // 23.0
```

### `limit(v, max_mag)`

Return `v` if its magnitude is at most `max_mag`, otherwise a vector in the same
direction scaled to that magnitude. Useful for capping velocities.

```petal
limit(vec2(6.0, 8.0), 5.0)    // vec2(3.0, 4.0)
limit(vec2(1.0, 0.0), 5.0)    // vec2(1.0, 0.0)
```

## Type Conversion

Petal does no implicit casting. When a value has the wrong type for a slot
(a `float` where an `int` is expected, say), convert it explicitly with one of
these.

### `str(value)`

Converts any value to its string representation.

```petal
str(42)        // "42"
str(true)      // "true"
str([1, 2])    // "[1, 2]"
```

### `int(value)`

Converts to an integer. Accepts numbers and numeric strings.

```petal
int(3.7)     // 3
int("42")    // 42
```

### `float(value)`

Converts to a float. Accepts numbers and numeric strings (surrounding
whitespace is ignored). A string that isn't a number is an error — use
[`parse_float`](#parse_floats--parse_ints) when the input might be bad.

```petal
float(42)      // 42.0
float("3.5")   // 3.5
float("42")    // 42.0
```

### `parse_float(s)` / `parse_int(s)`

Like `float` / `int`, but return `nil` instead of erroring when the text isn't
a number. `parse_int` accepts only whole numbers: `"3.5"` is `nil`, not `3`.
Write `int(parse_float(s))` when truncating is what you want.

```petal
parse_float("3.5")     // 3.5
parse_float("abc")     // nil
parse_float("")        // nil
parse_int("42")        // 42
parse_int("3.5")       // nil
```

```petal ignore
let n = parse_float(text_input())
if n == nil then
  show_error("Enter a number")
else
  total = total + n
end
```

### `type(value)`

Returns the type name as a string.

```petal
type(42)          // "int"
type(3.14)        // "float"
type("hello")     // "string"
type([1, 2])      // "list"
type({a: 1})      // "record"
type(true)        // "bool"
type(nil)         // "nil"
```

## Collections

### `len(collection)`

Returns the length of a list, string, or `f64_array`.

```petal
len([1, 2, 3])      // 3
len("hello")        // 5
len([])             // 0
len(f64_array(4))   // 4
```

### `append(list, value)`

Returns a **new** list with `value` added to the end. Lists are immutable
values, so `append` never changes its input — keep the result:

```petal
let items = [1, 2]
let more = append(items, 3)   // more is [1, 2, 3]; items is still [1, 2]
items = append(items, 3)      // grow an accumulator by rebinding
```

### `push(list, value)`

Deprecated alias for [`append`](#appendlist-value). Like `append` it returns a
new list; `push(items, 3)` on its own does nothing.

### `pop(list)`

Deprecated alias for [`drop_last`](#drop_lastlist). Returns a new list without
the last element — it does **not** return the removed element (use `last` for
that).

```petal
pop([1, 2, 3])   // [1, 2]
```

### `last(list)`

Returns the last element of a list, or `nil` if the list is empty.

```petal
last([1, 2, 3])   // 3
last([])          // nil
```

### `drop_last(list)`

Returns a **new** list without the last element.

```petal
drop_last([1, 2, 3])   // [1, 2]
drop_last([])          // []
```

### `remove(record, key)`

Returns a **new** record without `key`. Removing an absent key is not an
error. Only works on records, not lists.

```petal
remove({a: 1, b: 2}, "a")   // { b: 2 }
remove({a: 1}, "missing")   // { a: 1 }
```

### `keys(record)`

Returns a list of all keys from a record.

```petal
keys({name: "Alice", age: 30})   // ["name", "age"]
```

### `values(record)`

Returns a list of all values from a record.

```petal
values({a: 1, b: 2})   // [1, 2]
```

### `field(record, key, fallback)`

Reads `record[key]`, or `fallback` when the key is absent or nil. Same as
`record[key] ?? fallback`. A bare `record.missing` is still an error — see
[Ragged records](language-guide.md#ragged-records--reading-a-field-that-may-not-be-there).

```petal
field({a: 1}, "a", 7)    // 1
field({a: 1}, "zz", 7)   // 7
```

### `has_field(record, key)`

Whether the record has `key` at all, even when its value is nil (which `??`
cannot tell apart from an absent key).

```petal
has_field({a: nil}, "a")   // true
has_field({a: 1}, "zz")    // false
```

### `contains(collection, needle)`

Checks if a list contains a value or a string contains a substring.

```petal
contains([1, 2, 3], 2)       // true
contains("hello", "ell")     // true
contains([1, 2, 3], 5)       // false
```

### `includes(collection, needle)`

JavaScript-style alias for `contains`. Same behavior.

```petal
[1, 2, 3].includes(2)        // true
"hello".includes("ell")      // true
```

### `sort(list)`

Returns a new sorted list. Numbers sort before strings.

```petal
sort([3, 1, 2])           // [1, 2, 3]
sort(["c", "a", "b"])     // ["a", "b", "c"]
```

### `reverse(collection)`

Returns a new reversed list or string.

```petal
reverse([1, 2, 3])    // [3, 2, 1]
reverse("hello")      // "olleh"
```

### `join(list, separator)`

Joins list elements into a string with the given separator.

```petal
join(["a", "b", "c"], ", ")   // "a, b, c"
join([1, 2, 3], "-")          // "1-2-3"
```

### `split(string, separator)`

Splits a string into a list by the given separator. To split into characters,
use [`chars`](#charss).

```petal
split("a,b,c", ",")     // ["a", "b", "c"]
split("a  b", " ")      // ["a", "", "b"]
```

### `upper(string)` / `lower(string)`

Case conversion, Unicode-aware.

```petal
upper("aeronaut belt")    // "AERONAUT BELT"
lower("ÉCLAIR")           // "éclair"
lower(a) == lower(b)      // case-insensitive compare
```

### `enumerate(list)`

Returns a list of `[index, value]` pairs.

```petal
enumerate(["a", "b", "c"])   // [[0, "a"], [1, "b"], [2, "c"]]
```

### `zip(list_a, list_b)`

Pairs elements from two lists. Stops at the shorter list.

```petal
zip([1, 2], ["a", "b"])   // [[1, "a"], [2, "b"]]
```

### `slice(collection, start, end?)`

Returns a slice of a list or string. Supports negative indices. `end` defaults to the
length of the collection.

```petal
slice([1, 2, 3, 4], 1, 3)    // [2, 3]
slice([1, 2, 3, 4], -2)      // [3, 4]
slice("hello", 1, 3)         // "el"
```

### `flat(list)`

Flattens one level of nesting.

```petal
flat([[1, 2], [3, 4]])       // [1, 2, 3, 4]
flat([[1, [2]], [3]])         // [1, [2], 3]
```

### `index_of(collection, needle)`

Position of the first occurrence, or `-1` when absent. On a list this is the
element index; on a string it is a **character** index, ready to pass to
[`char_at`](#char_ats-i) or [`char_slice`](#char_slices-start-end).

```petal
index_of([10, 20, 30], 20)     // 1
index_of([10, 20], 99)         // -1
index_of("hello world", "wor") // 6
index_of("abc", "z")           // -1
```

```petal ignore
let i = index_of(line, "=")
if i >= 0 then
  let key = char_slice(line, 0, i)
  let value = char_slice(line, i + 1)
end
```

## Text (character-indexed)

`len` and `slice` count **bytes**. That is wrong for text: in `"Óscar"` the
first character is two bytes, so `slice("Óscar", 0, 1)` is `""`. The builtins
here count **characters** instead.

### `chars(s)`

The string as a list of single-character strings.

```petal
chars("Óscar")   // ["Ó", "s", "c", "a", "r"]
chars("")        // []
```

### `char_len(s)`

Number of characters, as opposed to `len`'s bytes.

```petal
char_len("Óscar")   // 5
len("Óscar")        // 6
```

### `char_at(s, i)`

The single character at character index `i`. Negative indices count from the
end. An out-of-range index gives `""` rather than an error.

```petal
char_at("Óscar", 0)    // "Ó"
char_at("Óscar", -1)   // "r"
char_at("Óscar", 99)   // ""
```

### `char_slice(s, start, end?)`

`slice` for text: the indices count characters. Negative indices count from the
end, both ends clamp, and `end` defaults to the end of the string.

```petal
char_slice("Óscar Delgado", 0, 1)   // "Ó"
char_slice("Óscar", 1)              // "scar"
char_slice("Óscar", -3, -1)         // "ca"
```

## Higher-Order Functions

### `map(list, fn)`

Applies a function to each element and returns a new list.

```petal
map([1, 2, 3], fn(x) -> x * 2)         // [2, 4, 6]
map(["a", "b"], fn(s) -> s ++ "!")     // ["a!", "b!"]
```

### `filter(list, fn)`

Returns a new list containing only elements where the function returns `true`.

```petal
filter([1, 2, 3, 4], fn(x) -> x > 2)            // [3, 4]
filter(["hi", "", "ok"], fn(s) -> len(s) > 0)   // ["hi", "ok"]
```

### `reduce(list, initial, fn)`

Folds over a list, accumulating a result.

```petal
reduce([1, 2, 3], 0, fn(acc, x) -> acc + x)   // 6
reduce([1, 2, 3], 1, fn(acc, x) -> acc * x)   // 6
```

### `forEach(list, fn)`

Runs a function once for each element and returns `nil`. Use when you
want the side effects (logging, drawing, mutations) but don't need a
new list.

```petal
forEach([1, 2, 3], fn(x) -> print(x))
```

## Assertions

Assertions stop the program with a message and source location when their
condition fails.

### `assert(condition, message?)`

Fails with `assertion failed: <message>` when `condition` is falsy.

```petal ignore
assert(x > 0, "x must be positive")
assert(len(items) == 3)
```

### `assert_eq(actual, expected)`

Fails with `assertion failed: assert_eq: left=<actual> right=<expected>` when
the two values are not equal. Prefer it over `assert(a == b)` because the
message shows both values.

```petal
assert_eq(2 + 2, 4)
assert_eq(sort([3, 1, 2]), [1, 2, 3])
```

## Automatic Differentiation

These functions support forward-mode automatic differentiation with dual numbers.

### `dual(value, derivative)`

Creates a dual number with the given primal value and derivative.

```petal
let x = dual(3.0, 1.0)   // value = 3.0, derivative = 1.0
```

### `value_of(x)`

Extracts the primal value from a dual number. Returns a float for regular numbers.

```petal
value_of(dual(3.0, 1.0))   // 3.0
value_of(42)                // 42.0
```

### `deriv_of(x)`

Extracts the derivative from a dual number. Returns `0.0` for regular numbers.

```petal
deriv_of(dual(3.0, 1.0))   // 1.0
deriv_of(42)                // 0.0
```

## Typed Numeric Arrays

An `f64_array` is a flat array of floats. It is faster than a list for numeric
inner loops (particle sims, grids, field evaluations) because its elements are
stored unboxed. Like lists, it is an immutable value: `set_at` and `swap`
return a new array, so keep the result.

Reading an element is plain indexing, and `a[i] = v` writes:

```petal
let a = f64_array(2)
a[0] = 2.5
a[0]             // 2.5
```

### `f64_array(n)`

Creates a zero-filled array of length `n`.

```petal
f64_array(3)   // [0.0, 0.0, 0.0]
f64_array(0)   // []
```

### `set_at(a, i, v)`

Returns a new array with slot `i` set to `v` (an int or float). An
out-of-bounds or negative index is an error.

```petal
let a = f64_array(3)
a = set_at(a, 1, 5.5)
a[1]             // 5.5
```

### `swap(a, i, j)`

Returns a new array with the elements at `i` and `j` exchanged. Both indices
are bounds-checked.

```petal
let a = f64_array(3)
a = set_at(a, 0, 1.0)
a = swap(a, 0, 2)    // [0.0, 0.0, 1.0]
```

## Built-in Classes

Classes built into the language: available with no declaration and no import.
See the [Language Guide](language-guide.md#classes--methods) for declaring your
own, and for how `value.method(...)` resolves.

### `Rect(x, y, w, h)`

A rectangle with fields `x`, `y`, `w`, `h`. An instance is an ordinary record
with a class tag, so anything that accepts an `{x, y, w, h}` record — including
every `petal-ui` draw call — accepts a `Rect`. (`petal-ui`'s `rect(x, y, w, h)`
is the same constructor.)

```petal
let r = Rect(10, 20, 100, 40)
r.x            // 10
type(r)        // "Rect"
keys(r)        // ["x", "y", "w", "h"]
```

Each field holds exactly the number it was given, `int` or `float`:
`Rect(10.5, 20.9, 100.4, 40.6).x` is `10.5`. A non-numeric argument is an
error naming the field.

The methods below are each exactly their equivalent expression, so an int rect
gives int answers (`/` on two ints truncates) and a float rect gives float
ones. `inset` and `offset` return a `Rect`, so calls chain.

| Method | Result | Equivalent |
|--------|--------|------------|
| `r.center_x()` | number | `r.x + r.w / 2` |
| `r.center_y()` | number | `r.y + r.h / 2` |
| `r.right()` | number | `r.x + r.w` (half-open, like the hit tests) |
| `r.bottom()` | number | `r.y + r.h` |
| `r.inset(n)` | `Rect` | pulled in by `n` on all four sides; a negative `n` grows it, and `w`/`h` clamp at 0 |
| `r.offset(dx, dy)` | `Rect` | moved by a delta, same size |

```petal
let card = Rect(0, 0, 100, 40)
card.center_x()             // 50
card.right()                // 100
card.inset(5)               // Rect(5, 5, 90, 30)
card.inset(5).center_x()    // 50
card.offset(10, 10).right() // 110

let sub = Rect(0.0, 0.0, 101.0, 40.0)
sub.center_x()              // 50.5, not 50
```

Add your own methods with `fn Rect.<name>(r: Rect, ...)`; a user declaration
wins over a built-in method of the same name.
