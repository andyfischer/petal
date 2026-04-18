# Builtins Reference

All built-in functions available in Petal.

## I/O

### `print(args...)`

Prints arguments to stdout, separated by spaces, followed by a newline.

```petal
print("hello")           // hello
print(1, "and", 2)       // 1 and 2
print([1, 2], {a: 3})    // [1, 2] {a: 3}
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

### `round(x)`

Rounds to the nearest integer (returns float).

```petal
round(3.4)   // 3.0
round(3.6)   // 4.0
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
fract(3.7)    // 0.7
fract(-1.2)   // 0.8
```

### `radians(degrees)` / `degrees(radians)`

Convert between degrees and radians.

```petal
radians(180.0)     // 3.141592653589793
degrees(pi())      // 180.0
```

## Creative Coding Math

These are the "Processing-style" utility builtins — the small vocabulary that
keeps animation, layout, and generative code readable.

### `clamp(value, lo, hi)`

Constrain a value to the range `[lo, hi]`.

```petal
clamp(15.0, 0.0, 10.0)   // 10.0
clamp(-3.0, 0.0, 10.0)   //  0.0
clamp(5.0, 0.0, 10.0)    //  5.0
```

### `lerp(a, b, t)`

Linear interpolation. `t=0` returns `a`, `t=1` returns `b`.

```petal
lerp(0.0, 100.0, 0.3)   // 30.0
lerp(10.0, 20.0, 0.5)   // 15.0
```

### `map_range(value, in_lo, in_hi, out_lo, out_hi)`

Remap a value from one range to another. The creative-coding workhorse — use it
to turn "pixel coordinate in `[0, width]`" into "angle in `[0, 2π]`", and similar.

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

Create an RGB color from Hue-Saturation-Value. `h` is in degrees (0–360),
`s` and `v` in 0.0–1.0.

```petal
hsv(120.0, 1.0, 1.0)    // { r: 0, g: 255, b: 0 }
```

### `hsl(h, s, l)`

Create an RGB color from Hue-Saturation-Lightness. Same argument ranges as `hsv`.

```petal
hsl(0.0, 1.0, 0.5)      // { r: 255, g: 0, b: 0 }
```

### `color_lerp(c1, c2, t)`

Interpolate two RGB color records.

```petal
let red = hsv(0.0, 1.0, 1.0)
let blue = hsv(240.0, 1.0, 1.0)
color_lerp(red, blue, 0.5)   // a purple
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
direction scaled to that magnitude. Used constantly in physics simulations to
cap velocities and steering forces.

```petal
limit(vec2(6.0, 8.0), 5.0)    // vec2(3.0, 4.0)
limit(vec2(1.0, 0.0), 5.0)    // vec2(1.0, 0.0)
```

## Type Conversion

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

Converts to a float.

```petal
float(42)    // 42.0
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

Returns the length of a list or string.

```petal
len([1, 2, 3])   // 3
len("hello")     // 5
len([])          // 0
```

### `push(list, value)`

Appends a value to the end of a list. Mutates the list in place.

```petal
let items = [1, 2]
push(items, 3)     // items is now [1, 2, 3]
```

### `append(list, value)`

Same as `push` — appends a value to the end of a list.

### `pop(list)`

Removes and returns the last element. Returns `nil` if the list is empty.

```petal
let items = [1, 2, 3]
let last = pop(items)   // last = 3, items = [1, 2]
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

Splits a string into a list by the given separator.

```petal
split("a,b,c", ",")     // ["a", "b", "c"]
split("hello", "")       // ["h", "e", "l", "l", "o"]
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

## Higher-Order Functions

### `map(list, fn)`

Applies a function to each element and returns a new list.

```petal
map([1, 2, 3], fn(x) { x * 2 })        // [2, 4, 6]
map(["a", "b"], fn(s) { s ++ "!" })     // ["a!", "b!"]
```

### `filter(list, fn)`

Returns a new list containing only elements where the function returns `true`.

```petal
filter([1, 2, 3, 4], fn(x) { x > 2 })       // [3, 4]
filter(["hi", "", "ok"], fn(s) { len(s) > 0 })  // ["hi", "ok"]
```

### `reduce(list, initial, fn)`

Folds over a list, accumulating a result.

```petal
reduce([1, 2, 3], 0, fn(acc, x) { acc + x })   // 6
reduce([1, 2, 3], 1, fn(acc, x) { acc * x })   // 6
```

### `forEach(list, fn)`

Runs a function once for each element and returns `nil`. Use when you
want the side effects (logging, drawing, mutations) but don't need a
new list.

```petal
forEach([1, 2, 3], fn(x) { print(x) })
```

## Assertions

Runtime assertions abort the program with a message and source location
when their condition fails. Useful for defensive programming and tests.

### `assert(condition, message?)`

Aborts with `assertion failed: <message>` (or a default message) when
`condition` is falsy.

```petal
assert(x > 0, "x must be positive")
assert(len(items) == 3)
```

### `assert_eq(actual, expected)`

Aborts with `assert_eq: left=<actual> right=<expected>` when the two
values are not equal. Prefer over `assert(a == b)` because the failure
message shows both operands.

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
