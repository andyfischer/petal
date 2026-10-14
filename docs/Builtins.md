# Builtins Reference

The built-in functions of the Petal runtime: the ones in scope in every
program, and the domain sets a file imports ([which is which](#which-builtins-need-an-import)). For language syntax
(variables, control flow, functions, state, pattern matching), see the
[Language Guide](language-guide.md). The drawing and input builtins used by
`petal-ui` apps (`draw_rect`, `mouse_x`, ...) are covered in the
[petal-ui README](../core-libs/petal-ui/README.md).

**The parameter names in this reference are real.** Every builtin with a
fixed parameter list takes [named arguments](syntax/function-overloading.md#named-arguments)
under the names its heading shows, in any order after the positional ones:

```petal
import creative
print(clamp(15, lo: 0, hi: 10))                        // 10
print(map_range(value: 5, in_lo: 0, in_hi: 10, out_lo: 0, out_hi: 100))  // 50.0
print([1, 2, 3, 4].slice(start: 1, end: 3))            // [2, 3]
```

- A parameter marked `?` (`slice(collection, start, end?)`) is optional and
  may be left off, named or not. Optional parameters are always last, and one
  cannot be skipped to reach a later one: `noise(x: 1, z: 2)` is
  `noise() is missing a value for parameter 'y'`.
- A builtin listed in several forms (`random()` / `random(max)` /
  `random(min, max)`, `distance(x1, y1, x2, y2)` / `distance(v1, v2)`) takes
  the names of whichever form the call fits.
- With method syntax the receiver is the first parameter, so
  `xs.slice(start: 1)` names the ones after it.
- A name may be a keyword: `end:` is an ordinary argument label
  (`range(start: 0, end: 5)`).
- The variadic builtins — `print(args...)` and `format(template, args...)` —
  have no parameter names to give, and refuse a named argument
  (`builtin 'print' does not accept named arguments`).

A wrong name is an error when the line runs (`clamp() has no parameter named
'low'`), and from `petal check` before that.

Each builtin's signature is written once, as a `native fn` declaration in a
package compiled into the binary (`core/packages/`): parameter names, which
are optional, the argument types the checker holds a call to, and the result
type. See [usage/ffi.md](usage/ffi.md#native-interfaces-native-fn) for the declaration
syntax.

### Which builtins need an import

What reads as language is **global**: it is declared by the `core` package and
in scope in every file with no import — `print`, `len`, `str`, `type`,
`range`, the casts, assertions, the collection, string and record functions,
the higher-order functions, formatting, and the plain math of the
[Math](#math) section (plus `clamp` and `random_int`).

The **domain builtins** live in bundled packages and are in scope only in a
file that imports the package:

| Import | Names |
|---|---|
| `import creative` | `lerp`, `map_range`, `distance`, `mag`, `smoothstep`, `choose` |
| `import noise` | `noise`, `noise_seed` |
| `import color` | `hsv`, `hsl`, `hsv_deg`, `hsl_deg`, `color_lerp` |
| `import vec2` | `vec2`, `vec3`, `normalize`, `dot`, `cross`, `limit`, `rotate` |
| `import autodiff` | `dual`, `value_of`, `deriv_of` |
| `import json` | `json_stringify`, `json_parse` |
| `import io` | `symbol`, `push_output`, `binding` |

After `import noise` the names work bare (`noise(x)`), qualified
(`noise.noise_seed(1)`) and as methods (`x.noise()`); `import noise: noise_seed`
takes just the names listed. Without the import, a call is an error that says
which import to add (`petal check` reports it, and `petal fix-imports` adds
it). Method syntax follows the same rule: `v.normalize()` reaches the `vec2`
builtin only in a file that imports `vec2` — the import of some other file of
the program does not count, and `petal check` reports the call when the
receiver is plainly a number, string, list or vector. The values themselves need no
import: a `vec2` a function was handed still has `.x`, arithmetic and `==`.

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

### `random()` / `random(max)` / `random(min, max)`

Returns a pseudo-random float in the range [min, max). `min` defaults to 0.0
and `max` to 1.0.

```petal
random()            // 0.7342... (varies), in [0, 1)
random(16.0)        // 9.310...  (varies), in [0, 16)
random(1.0, 10.0)   // 4.218...  (varies)
```

### `range(end)` / `range(start, end)` / `range(start, end, step)`

Returns a list of integers from `start` (inclusive) to `end` (exclusive).
With a `step`, counts by it; a negative step counts down, stopping before
`end`. A step of 0 is an error. (`range(n)` is `range(0, n)`.)

```petal
range(0, 5)        // [0, 1, 2, 3, 4]
range(3, 7)        // [3, 4, 5, 6]
range(0, 10, 3)    // [0, 3, 6, 9]
range(5, 0, -1)    // [5, 4, 3, 2, 1]
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

### `asin(x)` / `acos(x)` / `atan(x)`

Inverse trigonometric functions, returning radians. `asin` and `acos` take
`x` in [-1, 1] (outside it they return NaN); `atan` takes any number. For the
angle of a point, use `atan2(y, x)`, which knows the quadrant.

```petal
asin(1.0)            // 1.5707963267948966 (π/2)
degrees(acos(0.5))   // 59.99999999999999
atan(1.0)            // 0.7853981633974483 (π/4)
```

### `hypot(x, y)`

The length of the vector `(x, y)`, `sqrt(x*x + y*y)`, without overflowing on
large inputs.

```petal
hypot(3.0, 4.0)   // 5.0
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

### `safe_div(a, b)`

Division that returns `nil` instead of aborting when the divisor is zero.
Otherwise it is exactly `/`: int / int truncates, floats divide as floats.
Bare `/` aborts on a zero divisor by design; use `safe_div` where the divisor
comes from input or can legitimately be zero, and default with `??`.

```petal
safe_div(7, 2)          // 3
safe_div(7.0, 2)        // 3.5
safe_div(1, 0)          // nil
safe_div(1.0, 0.0)      // nil
safe_div(3, 0) ?? 0     // 0
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

`clamp` is global; the rest of this section needs `import creative`.

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

Linear interpolation. `t=0` returns `a`, `t=1` returns `b`. `a` and `b` may
also be two `vec2`s or two `vec3`s, two colors, or two equal-length lists of
numbers, interpolated component-wise. Colors blend like `color_lerp`: per
channel, alpha included, rounded to int channels.

```petal
import creative
import vec2
lerp(0.0, 100.0, 0.3)   // 30.0
lerp(10.0, 20.0, 0.5)   // 15.0
lerp(vec3(0.0, 0.0, 0.0), vec3(10.0, 20.0, 30.0), 0.5)   // vec3(5.0, 10.0, 15.0)
lerp(#6a5a48, #ffd08a, 0.5)            // { r: 181, g: 149, b: 105 }
lerp([0.0, 10.0], [1.0, 20.0], 0.5)    // [0.5, 15.0]
```

### `map_range(value, in_lo, in_hi, out_lo, out_hi)`

Remap a value from one range to another — for example a pixel coordinate in
`[0, width]` to an angle in `[0, 2π]`.

```petal
import creative
map_range(5.0, 0.0, 10.0, 100.0, 200.0)   // 150.0
map_range(0.5, 0.0, 1.0, -1.0, 1.0)       //   0.0
```

### `distance(x1, y1, x2, y2)` / `distance(v1, v2)`

Euclidean distance. Accepts either four scalars, two `vec2` values, or two
`vec3` values.

```petal
import creative
import vec2
distance(0.0, 0.0, 3.0, 4.0)                // 5.0
distance(vec2(0.0, 0.0), vec2(3.0, 4.0))    // 5.0
distance(vec3(1.0, 1.0, 1.0), vec3(3.0, 4.0, 7.0))    // 7.0
```

### `mag(x, y)` / `mag(x, y, z)` / `mag(v)`

Vector magnitude (length). Accepts 2D or 3D scalars, or a single `vec2` or
`vec3`.

```petal
import creative
import vec2
mag(3.0, 4.0)          // 5.0
mag(vec2(3.0, 4.0))    // 5.0
mag(vec3(2.0, 3.0, 6.0))    // 7.0
```

### `smoothstep(edge0, edge1, x)`

Hermite interpolation between two edges — produces a smooth S-curve from 0 to 1.
Equivalent to GLSL's `smoothstep`.

```petal
import creative
smoothstep(0.0, 1.0, 0.5)   // 0.5
smoothstep(0.0, 1.0, 0.25)  // 0.15625
```

## Noise

Needs `import noise`.

### `noise(x)` / `noise(x, y)` / `noise(x, y, z)`

Perlin noise in 1D, 2D, or 3D. Returns a smooth value centered around 0. Ideal
for organic motion, terrain, clouds, and flow fields.

```petal
import noise
noise(0.5)                  // smooth 1D value
noise(0.3, 0.7)             // smooth 2D value
noise(0.1, 0.2, 0.3)        // smooth 3D value
```

### `noise_seed(seed)`

Sets the global noise seed for reproducibility. Takes an integer.

```petal
import noise
noise_seed(42)
```

## Extended Randomness

`random_int` is global; `choose` needs `import creative`.

### `random_int(lo, hi)`

Random integer in the half-open range `[lo, hi)`.

```petal
random_int(0, 10)    // 0..9
```

### `choose(list)`

Returns a random element from a list, or `nil` for an empty list.

```petal
import creative
choose([1, 2, 3])             // one of 1, 2, or 3
choose(["red", "green"])      // "red" or "green"
```

## Color

Needs `import color`.

All color builtins return an RGB record `{r: int, g: int, b: int}` with channels
in 0..255 — the same shape produced by the `#rrggbb` color literal.

### `hsv(h, s, v)`

Create an RGB color from Hue-Saturation-Value. All three arguments are in
`[0, 1]`; hue wraps, so `1.0` is the same as `0.0`. For hue in degrees, use
`hsv_deg`.

```petal
import color
hsv(0.0, 1.0, 1.0)          // { r: 255, g: 0, b: 0 }   (red)
hsv(1.0 / 3.0, 1.0, 1.0)    // { r: 0, g: 255, b: 0 }   (green)
```

### `hsl(h, s, l)`

Create an RGB color from Hue-Saturation-Lightness. Same argument ranges as
`hsv`. For hue in degrees, use `hsl_deg`.

```petal
import color
hsl(0.0, 1.0, 0.5)      // { r: 255, g: 0, b: 0 }
```

### `hsv_deg(h, s, v)` / `hsl_deg(h, s, l)`

Like `hsv` / `hsl` but with hue in **degrees** `[0, 360)`.

```petal
import color
hsv_deg(120.0, 1.0, 1.0)    // { r: 0, g: 255, b: 0 }
hsl_deg(120.0, 1.0, 0.5)    // { r: 0, g: 255, b: 0 }
```

### `color_lerp(c1, c2, t)`

Interpolate between two RGB color records. `t=0` returns `c1`, `t=1` returns
`c2`. If either color has an alpha `a`, the result does too (a color without
one counts as opaque, 255). `lerp` does the same on two colors.

```petal
import color
let red = hsv_deg(0.0, 1.0, 1.0)
let blue = hsv_deg(240.0, 1.0, 1.0)
color_lerp(red, blue, 0.5)   // { r: 128, g: 0, b: 128 }
```

## Vectors (2D and 3D)

Needs `import vec2` (which declares `vec3` and the shared helpers too).

Petal has built-in `vec2` and `vec3` types, backed by two and three f64s. Both
work with the usual arithmetic operators and share the helpers below
(`normalize`, `dot`, `limit`, plus `mag`, `distance` and `lerp` above), which
answer in the kind of vector they were given. `rotate` is 2D-only and `cross`
3D-only.

Operators, for either kind:

- `a + b`, `a - b`, `a * b`, `a / b` between two vectors of the same kind work
  component-wise. Mixing a `vec2` with a `vec3` is an error.
- `v + s`, `v - s`, `v * s`, `v / s` with a number `s` apply `s` to every
  component; `s + v`, `s - v`, `s * v` work too.
- `-v` negates every component.
- `==` / `!=` compare components.
- `.x`, `.y` (and `.z` on a `vec3`) read a component as a float.

A `vec2` is stored inline in a value; a `vec3` is a small immutable heap object
(three f64s would not fit), which makes it slightly costlier to create but
otherwise indistinguishable. One visible difference: a zero `vec2` is falsy in
an `if`, while every `vec3` is truthy, like a record — compare against
`vec3(0, 0, 0)` explicitly. Both survive `json_stringify` / `json_parse` (as
`{"type": "vec3", "x": ..., "y": ..., "z": ...}`) and a state save/restore.

### `vec2(x, y)`

Construct a 2D vector.

```petal
import creative
import vec2
let v = vec2(3.0, 4.0)
print(mag(v))    // 5.0
```

### `vec3(x, y, z)`

Construct a 3D vector.

```petal
import creative
import vec2
let p = vec3(1.0, 2.0, 2.0)
print(mag(p))    // 3.0
print(p * 2)     // vec3(2.0, 4.0, 4.0)
print(p.z)       // 2.0
```

### `normalize(v)`

Return a vector pointing in the same direction as `v` with magnitude 1. The
zero vector normalizes to the zero vector (`vec2(0, 0)` / `vec3(0, 0, 0)`).

```petal
import vec2
normalize(vec2(3.0, 4.0))    // vec2(0.6, 0.8)
normalize(vec3(0.0, 0.0, 5.0))    // vec3(0.0, 0.0, 1.0)
```

### `dot(a, b)`

Dot product of two `vec2` or two `vec3` values.

```petal
import vec2
dot(vec2(1.0, 0.0), vec2(0.0, 1.0))   // 0.0
dot(vec2(2.0, 3.0), vec2(4.0, 5.0))   // 23.0
dot(vec3(1.0, 2.0, 3.0), vec3(4.0, 5.0, 6.0))   // 32.0
```

### `cross(a, b)`

Cross product of two `vec3` values: a vector perpendicular to both, following
the right-hand rule.

```petal
import vec2
cross(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0))   // vec3(0.0, 0.0, 1.0)
```

### `limit(v, max_mag)`

Return `v` if its magnitude is at most `max_mag`, otherwise a vector in the same
direction scaled to that magnitude. Useful for capping velocities.

```petal
import vec2
limit(vec2(6.0, 8.0), 5.0)    // vec2(3.0, 4.0)
limit(vec2(1.0, 0.0), 5.0)    // vec2(1.0, 0.0)
limit(vec3(0.0, 0.0, 10.0), 2.0)    // vec3(0.0, 0.0, 2.0)
```

### `rotate(v, angle)`

Return `v` turned by `angle` radians, in the same sense as
`vec2(cos(angle), sin(angle))` — so on a y-down screen a positive angle turns
clockwise. Useful for ship headings and polygon outlines.

```petal
import vec2
rotate(vec2(1.0, 0.0), pi() / 2)   // vec2(~0.0, 1.0)
rotate(vec2(2.0, 1.0), pi())       // vec2(-2.0, ~-1.0)
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

An enum variant converts to the way it is written in source:
`str(Color.red)` is `"Color.red"` and `str(Op.fight("bridge", true))` is
`"Op.fight(\"bridge\", true)"`. For the variant's bare name use `.name`
(`Color.red.name` is `"red"`).

### `int(value)`

Converts to an integer. Accepts numbers and numeric strings.

```petal
int(3.7)     // 3
int("42")    // 42
```

### `float(value)`

Converts to a float. Accepts numbers and numeric strings (surrounding
whitespace is ignored). A string that isn't a number is an error — use
[`parse_float`](#parse_floatstring--parse_intstring) when the input might be bad.

```petal
float(42)      // 42.0
float("3.5")   // 3.5
float("42")    // 42.0
```

### `parse_float(string)` / `parse_int(string)`

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
import ui

let n = parse_float(text_input())
if n == nil then
  show_error("Enter a number")
else
  total = total + n
end
```

### `json_stringify(value)` / `json_stringify(value, indent)`

Encodes a value as JSON text: `nil` → `null`, bools, numbers, strings, lists
→ arrays, records → objects (field order kept). The one-argument form is
compact; `indent` pretty-prints with that many spaces per level. A class
instance encodes as its fields; a value with no JSON form (a function)
encodes as its display string. This is the codec for `panel_store_set` and
any other string-only channel.

```petal
import json
json_stringify({a: 1, b: [true, nil]})   // {"a":1,"b":[true,null]}
json_stringify("q\"uote")                // "q\"uote"
```

### `json_parse(text)`

Decodes JSON text into the matching value — objects become plain records —
or returns `nil` when the text is not JSON, so a corrupt store entry is a
`?? default` rather than an aborted frame. (The text `null` also parses to
`nil`.)

```petal
import json
json_parse("[1, 2.5, \"x\"]")        // [1, 2.5, "x"]
json_parse("\{\"a\": \{\"b\": 2}}").a.b   // 2  (`\{` — a bare `{` interpolates)
json_parse("not json")             // nil
```

```petal ignore
import json
import garden
state todos = json_parse(panel_store_get("todos") ?? "[]") ?? []
panel_store_set("todos", json_stringify(todos))
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

For a class instance it is the class's name, and for an enum variant the
enum's name:

```petal
enum Color
    red,
    custom(r, g, b),
end

print(type(Color.red))                // Color
print(type(Color.custom(1, 2, 3)))    // Color
print(type(Rect(0, 0, 1, 1)))         // Rect
```

## Formatting

`str(x)` prints a float in full (`str(12.3 * 3)` is `"36.900000000000006"`).
These produce text for a table, a price or a label.

### `fixed(value, places?)`

The number as a string with exactly `places` digits after the decimal point
(2 when omitted; clamped to 0..17). Rounds half away from zero, as
[`round`](#roundx--roundx-places) does.

```petal
fixed(12.3456, 2)   // "12.35"
fixed(5)            // "5.00"
```

### `commas(value, places?)`

The number with a `,` every three digits of its integer part. With `places`
it is rounded to that many decimals first; without, an int prints as an int
and a float as `str` prints it.

```petal
commas(1234567)        // "1,234,567"
commas(1234.5678, 2)   // "1,234.57"
```

### `pad_start(value, width, fill?)` / `pad_end(value, width, fill?)`

Pads to `width` characters with `fill` (a space when omitted), on the left
or on the right. A value that is not a string is rendered as `str` does
first. A value already `width` long or longer is returned as it is.

```petal
pad_start(7, 3, "0")   // "007"
pad_end("ab", 5)       // "ab   "
```

### `format(template, args...)`

printf-style templating. Each `%` placeholder takes the next argument:

| Placeholder | Meaning |
|---|---|
| `%s` | the value, as `str` renders it |
| `%d` | the value as a whole number (truncated) |
| `%f` | the value as a float; `%.2f` fixes two decimals |
| `%x` | the value in lowercase hex |
| `%%` | a literal `%` |

A `,` straight after the `%` groups thousands (`%,d`, `%,.2f`). A number
sets a minimum field width, padded on the left, or on the right when it is
written negative (`%8.2f`, `%-10s`). The placeholders are `%` and not `{}`
because `{` starts an interpolation inside a string literal. `format` is
variadic and takes no named arguments.

```petal
print(format("%s has %,d points (%.1f%%)", "Ana", 12345, 99.5))   // Ana has 12,345 points (99.5%)
print(format("%-6s|%6.2f|%x", "ab", 3.14159, 255))                // ab    |  3.14|ff
```

## Collections

### `len(collection)`

Returns the length of a list, string, or `f64_array`. A string's length is its
number of **characters** (Unicode code points), not bytes; see
[String offsets](#string-offsets).

```petal
len([1, 2, 3])      // 3
len("hello")        // 5
len("Óscar")        // 5
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

### `prepend(list, value)`

Returns a **new** list with `value` in front. The mirror of `append`.

```petal
prepend([2, 3], 1)   // [1, 2, 3]
```

### `concat(a, b)`

Joins two lists, or two strings, into a new one. `append(xs, ys)` adds `ys`
as a single element; `concat` splices its elements in.

```petal
concat([1, 2], [3, 4])   // [1, 2, 3, 4]
append([1, 2], [3, 4])   // [1, 2, [3, 4]]
concat("ab", "cd")       // "abcd"
```

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

### `field(rec, key, fallback)`

Reads `rec[key]`, or `fallback` when the key is absent or nil. Same as
`rec[key] ?? fallback`. A bare `record.missing` is still an error — see
[Ragged records](language-guide.md#ragged-records--reading-a-field-that-may-not-be-there).

```petal
field({a: 1}, "a", 7)    // 1
field({a: 1}, "zz", 7)   // 7
```

### `has_field(rec, key)`

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

Returns a new sorted list. Numbers sort before strings; NaNs sort after every
other number.

```petal
sort([3, 1, 2])           // [1, 2, 3]
sort(["c", "a", "b"])     // ["a", "b", "c"]
```

### `sort(list, compare)`

Sorts with a comparator of two elements. `compare(a, b)` returns a number
(negative: `a` first; positive: `b` first; zero: a tie) or a bool (`true`: `a`
first). The sort is stable.

```petal
sort([3, 1, 2], fn(a, b) -> b - a)            // [3, 2, 1]
sort(rows, fn(a, b) -> a.score < b.score)     // lowest score first
```

### `sort_by(list, key, descending?)`

Sorts by a key read off each element; `key(element)` is called once per
element and must return a number or a string (any other key is an error, and
`petal check` reports it ahead of time when it can see what the key function
returns).
`descending` is `true` or `"desc"` for descending order, and `false`,
`"asc"`, or omitted for ascending. The sort is stable in both directions, so
chained calls sort by several keys — least significant first.

```petal
sort_by(rows, fn(r) -> r.score)                  // lowest score first
sort_by(rows, fn(r) -> r.score, "desc")          // highest score first
sort_by(sort_by(rows, fn(r) -> r.name), fn(r) -> r.score, "desc")   // ties by name
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
use [`chars`](#charsstring).

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

### `trim(string)`

The string without its leading and trailing whitespace — spaces, tabs, line
breaks, and the other Unicode whitespace characters. Whitespace inside the
text stays.

```petal
trim("  hello world \n")   // "hello world"
trim("   ")                // ""
```

### `starts_with(string, prefix)` / `ends_with(string, suffix)`

Whether the string begins (or ends) with the given text. Every string starts
and ends with `""`.

```petal
starts_with("hello.ptl", "hello")   // true
ends_with("hello.ptl", ".ptl")      // true
starts_with("hello", "Hello")       // false (case matters; compare `lower` of both)
```

### `repeat(string, count)`

The string written `count` times in a row. A `count` of zero or less gives
`""`; `count` must be an int.

```petal
repeat("-", 20)      // "--------------------"
repeat("ab", 3)      // "ababab"
repeat("x", 0)       // ""
```

Use it instead of appending in a loop. `out = out ++ s` copies everything
written so far on every pass, so a loop of `n` appends does work proportional
to `n²`; `repeat` allocates the result once. When the pieces differ, collect
them in a list and [`join`](#joinlist-separator) it at the end.

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
length of the collection. On a string the indices count **characters**, so a
slice never cuts one in half.

```petal
slice([1, 2, 3, 4], 1, 3)    // [2, 3]
slice([1, 2, 3, 4], -2)      // [3, 4]
slice("hello", 1, 3)         // "el"
slice("Óscar Delgado", 0, 1) // "Ó"
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
`slice` or to index with `s[i]`.

```petal
index_of([10, 20, 30], 20)     // 1
index_of([10, 20], 99)         // -1
index_of("hello world", "wor") // 6
index_of("abc", "z")           // -1
```

```petal ignore
let i = index_of(line, "=")
if i >= 0 then
  let key = slice(line, 0, i)
  let value = slice(line, i + 1)
end
```

## String offsets

Every string position counts **characters** (Unicode code points): `len(s)`,
`s.length`, `slice(s, a, b)`, `s[i]` and `index_of(s, needle)` all use the same
unit, so an offset one of them returns can be handed to another. In `"Óscar"`
the first character takes two bytes of storage, and `len("Óscar")` is still 5.

```petal
let s = "Óscar"
print(len(s))            // 5
print(s[0])              // Ó
print(s[-1])             // r
print(slice(s, 0, 2))    // Ós
print(index_of(s, "c"))  // 2
```

`s[i]` is the one-character string at index `i`. Like a list index it counts
from the end when negative and is an error when out of range; use
[`char_at`](#char_atstring-index) where an out-of-range read should give `""`.

These are constant-time on a string that is all ASCII, however long. A string
containing a non-ASCII character is walked to find the position; reading it in
order (`s[i]`, then `s[i + 1]`) resumes from the last position, so a pass over
the whole string is still linear. To visit every character, `for c in chars(s)`
is the simplest form.

A code point is not always what a reader sees as one character: `"é"` written
as `e` plus a combining accent is two, and so is a flag emoji.

`chars`, `char_len`, `char_at` and `char_slice` date from when `len` and `slice`
counted bytes. `char_len` and `char_slice` now do exactly what `len` and `slice`
do on a string, and remain for existing scripts.

### `byte_len(string)`

Size of the string's UTF-8 encoding in bytes. For size limits and wire formats;
use `len` for anything about the text.

```petal
byte_len("Óscar")   // 6
byte_len("hello")   // 5
```

### `byte_slice(string, start, end?)`

`slice` with **byte** offsets into the UTF-8 encoding. Negative indices count
from the end and both ends clamp. A bound that falls inside a character moves
inward to the next character boundary, so the result is always whole characters
and never longer than the range asked for.

```petal
byte_slice("Óscar", 0, 2)   // "Ó"
byte_slice("Óscar", 0, 1)   // ""  (half of "Ó" is dropped)
byte_slice("hello", 1, 3)   // "el"
```

### `chars(string)`

The string as a list of single-character strings.

```petal
chars("Óscar")   // ["Ó", "s", "c", "a", "r"]
chars("")        // []
```

### `char_len(string)`

Number of characters. The same as `len` on a string.

```petal
char_len("Óscar")   // 5
len("Óscar")        // 5
```

### `char_at(string, index)`

The single character at character index `index`. Negative indices count from the
end. An out-of-range index gives `""` rather than an error.

```petal
char_at("Óscar", 0)    // "Ó"
char_at("Óscar", -1)   // "r"
char_at("Óscar", 99)   // ""
```

### `char_slice(string, start, end?)`

The same as `slice` on a string: the indices count characters. Negative indices
count from the end, both ends clamp, and `end` defaults to the end of the string.

```petal
char_slice("Óscar Delgado", 0, 1)   // "Ó"
char_slice("Óscar", 1)              // "scar"
char_slice("Óscar", -3, -1)         // "ca"
```

## Higher-Order Functions

### `map(list, f)`

Applies a function to each element and returns a new list.

```petal
map([1, 2, 3], fn(x) -> x * 2)         // [2, 4, 6]
map(["a", "b"], fn(s) -> s ++ "!")     // ["a!", "b!"]
```

### `filter(list, pred)`

Returns a new list containing only elements where the function returns `true`.

```petal
filter([1, 2, 3, 4], fn(x) -> x > 2)            // [3, 4]
filter(["hi", "", "ok"], fn(s) -> len(s) > 0)   // ["hi", "ok"]
```

### `reduce(list, initial, f)`

Folds over a list, accumulating a result.

```petal
reduce([1, 2, 3], 0, fn(acc, x) -> acc + x)   // 6
reduce([1, 2, 3], 1, fn(acc, x) -> acc * x)   // 6
```

### `forEach(list, f)`

Runs a function once for each element and returns `nil`. Use when you
want the side effects (logging, drawing, mutations) but don't need a
new list.

```petal
forEach([1, 2, 3], fn(x) -> print(x))
```

## List helpers

Written in Petal in the `std` prelude (`core/prelude/std.ptl`), which every
program imports implicitly, so they are called bare with no import. A
program's own declaration of one of these names shadows it silently. They
are ordinary functions, not natives: they take no method syntax
(`xs.sum()` is an error; write `sum(xs)` or `xs |> sum()`).
[`field`](#fieldrec-key-fallback) and
[`has_field`](#has_fieldrec-key) are prelude functions too.

### `first(xs)`

The first element, or `nil` when the list is empty. The complement of
[`last`](#lastlist).

```petal
first([3, 4])   // 3
first([])       // nil
```

### `is_empty(xs)`

Whether a list or string has no elements: `len(xs) == 0`.

### `sum(xs)` / `product(xs)`

The sum (0 for an empty list) or the product (1 for an empty list) of a list
of numbers.

```petal
sum([1, 2, 3])       // 6
product([2, 3, 4])   // 24
```

### `mean(xs)`

The arithmetic mean as a float, or `nil` for an empty list.

```petal
mean([1, 2, 3, 6])   // 3.0
```

### `minimum(xs)` / `maximum(xs)`

The smallest or largest element, or `nil` for an empty list. Works on any
elements `<` compares. ([`min`](#mina-b) and [`max`](#maxa-b) take two
numbers.)

```petal
minimum([3, 1, 2])   // 1
maximum([3, 1, 2])   // 3
```

### `count(xs, pred)` / `any(xs, pred)` / `all(xs, pred)`

How many elements satisfy `pred`, whether at least one does (`false` for an
empty list), and whether every one does (`true` for an empty list).

```petal
count([1, 2, 3, 4], fn(x) x > 2 end)   // 2
any([1, 2, 3], fn(x) x > 2 end)        // true
all([1, 2, 3], fn(x) x > 2 end)        // false
```

### `find(xs, pred)`

The first element that satisfies `pred`, or `nil` when none does.

```petal
find([1, 2, 3], fn(x) x > 1 end)   // 2
```

### `take(xs, n)` / `drop(xs, n)`

The first `n` elements, and everything after them. `n` is clamped to the
list, so neither is ever an error.

```petal
take([1, 2, 3], 2)   // [1, 2]
drop([1, 2, 3], 2)   // [3]
take([1, 2, 3], 9)   // [1, 2, 3]
```

### `clamp01(x)`

`clamp(x, 0, 1)`.

## Function Introspection (experimental)

Two builtins let a program read a function value as plain records and lists.
They are global, and their result shapes may still change.
[Function Introspection](features/function-introspection.md) is the full
description: every field, `code_hash`, the nodes of the tree, and the limits.

### `fn_info(f)`

A record describing the function `f` (a lambda or a named `fn`): `name`,
`file`, `line`, `column`, `params`, `returns`, `captures` and `code_hash`.
`nil` for a builtin and for an overloaded name. See
[`fn_info`](features/function-introspection.md#fn_infof).

### `fn_ast(f)`

The function's syntax tree as nested records, each with a `tag`. `nil` when
there is no source to read (a builtin, an overloaded name, a class
constructor, a program loaded from IR). See
[`fn_ast`](features/function-introspection.md#fn_astf).

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

Needs `import autodiff`.

The constructor and accessors for dual numbers (forward-mode automatic
differentiation). How a derivative moves through arithmetic and through the
other builtins, with worked examples, is in
[features/differentiation.md](features/differentiation.md).

### `dual(value, derivative)`

Creates a dual number from two plain numbers.

```petal
import autodiff
let x = dual(3.0, 1.0)   // value = 3.0, derivative = 1.0
```

### `value_of(x)`

The value part of a dual number, as a float. A plain number comes back as a float.

```petal
import autodiff
value_of(dual(3.0, 1.0))   // 3.0
value_of(42)                // 42.0
```

### `deriv_of(x)`

The derivative part of a dual number, as a float. `0.0` for a plain number.

```petal
import autodiff
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

### `f64_array(length)`

Creates a zero-filled array of `length` elements.

```petal
f64_array(3)   // [0.0, 0.0, 0.0]
f64_array(0)   // []
```

### `set_at(array, index, value)`

Returns a new array with slot `index` set to `value` (an int or float). An
out-of-bounds or negative index is an error.

```petal
let a = f64_array(3)
a = set_at(a, 1, 5.5)
a[1]             // 5.5
```

### `swap(array, i, j)`

Returns a new array with the elements at `i` and `j` exchanged. Both indices
are bounds-checked.

```petal
let a = f64_array(3)
a = set_at(a, 0, 1.0)
a = swap(a, 0, 2)    // [0.0, 0.0, 1.0]
```

## Frozen Data

The garbage collector visits every value the program still holds each time it
runs. For most programs that is cheap. For a program that keeps a very large
table for its whole life (a generated world, a rule table, a level loaded from
disk) it is a pause on every collection that grows with the table: about 2.5 ms
for 100,000 records, before any garbage is freed.

`freeze` takes such a value out of the collector's work. A frozen value is
still an ordinary value: it reads, compares, prints and passes around exactly
as before, and an update of it yields a new, unfrozen value as an update of
any value does.

### `freeze(value)`

Makes `value` and everything it holds frozen, and returns `value`. The
collector no longer looks inside a frozen value and never frees one. Lists,
records, strings, `f64_array`s, vectors and enum variants freeze, to any depth.
Freezing a number, a bool or `nil`, or a value that is already frozen, does
nothing.

```petal
fn build_table(n)
  for i in range(0, n) do
    { id: i, name: "row " ++ str(i), tags: [i, i + 1] }
  end
end

state table = freeze(build_table(1000))
print(frozen(table), len(table))      // true 1000

let longer = append(table, { id: 1000, name: "extra", tags: [] })
print(frozen(longer), len(table))     // false 1000
```

The limits are part of the contract:

- **Freezing is one-way.** There is no unfreeze, and a frozen value is never
  freed, even after the program drops its last reference to it. The memory
  comes back when the program's heap goes away (the program is restarted or
  its context is dropped).
- **Freeze data that lives as long as the program**, once: in a `state`
  initializer, as above, or behind a check that it has not been built yet.
  `freeze` on a value rebuilt every frame leaks every one of them, so a
  top-level `let table = freeze(build())` in a script that runs once per
  frame is a leak. `state` initializers run once.
- **A top-level table that depends on no dynamic data is the exception to
  that**, where the host keeps static values between runs (it does by
  default; see [performance.md](dev/performance.md#static-values)): `let
  table = [...]` followed by `freeze(table)` freezes the one table the stack
  keeps, once, and `frozen(table)` is then true from the top of every later
  run. `let table = freeze(build())` is still rebuilt and frozen on every
  run, because a call of `freeze` is never static. With static values off
  both forms leak a table per run.
- **A value holding a function or a `var` cell cannot be frozen.** The call
  is an error (`freeze: the value holds a function, which cannot be frozen`)
  and nothing is frozen.
- **A hot reload keeps frozen values**, like the rest of the heap. A table
  that is rebuilt after a reload and frozen again leaves the old one
  allocated.
- **Freezing removes the value from a collection's work, not the rest of the
  heap.** A collection afterwards costs what the unfrozen data and the garbage
  cost. Freeze a table as soon as it is built: until then it is ordinary data
  and every collection during the build visits it.

A forked execution (and a snapshot restored from one) carries the frozen
values it was taken with.

### `frozen(value)`

Whether `value` is frozen. `false` for numbers, bools, `nil` and every value
`freeze` has not reached.

```petal
let xs = [1, 2, 3]
print(frozen(xs))     // false
freeze(xs)
print(frozen(xs))     // true
print(frozen(42))     // false
```

## Host I/O

Needs `import io`. These are the script's side of the channels a host reads
and writes; [usage/embedding-guide.md](usage/embedding-guide.md) describes
the host's side.

### `symbol(name)`

The symbol called `name`: an interned name that identifies an output buffer
or a host binding. Two calls with the same name give the same symbol, and
`type` of one is `"symbol"`.

### `push_output(buffer, value)`

Appends `value` to the output buffer the symbol `buffer` names, for the host
to drain after the run. Returns `nil`.

```petal
import io
push_output(symbol("shapes"), {kind: "dot", x: 10, y: 20})
```

### `binding(symbol)`

The value the host has bound to the symbol (a uniform the host sets before a
run), or `nil` when nothing is bound.

## Pending Values

A host can hand a script a value that has not arrived yet: an image still
loading, a query in flight. Such a value is *pending*. `??` falls through a
pending value as it does through `nil`. These inspect one; all are global.

### `is_pending(value)` / `is_ready(value)`

`is_pending` is true when the value is pending at all, loading or errored.
`is_ready` is its negation: true for any usable value.

```petal
is_pending(1)   // false
is_ready(1)     // true
```

### `is_loading(value)` / `is_error(value)`

True when the value is pending and, respectively, still loading or failed.
Both are false for a value that is not pending.

### `error_of(value)`

The error a failed pending value holds, or `nil` for anything else.

### `or_else(value, fallback)`

`fallback` when `value` is pending (loading or errored), otherwise `value`.
Both arguments are evaluated before the call; `value ?? fallback` evaluates
its right side only when needed.

### `resource_key(value)`

The cache key of a pending value's resource as an int, or `nil` for a value
that is not pending. Two pending values for the same resource have the same
key.

## Host Handles

### `is_valid(handle)`

Whether the value is a [host handle](game-engine-features/handles.md) whose
object the host still considers alive. Never an error: `nil`, a value that is not a handle
and a stale handle are all `false`.

```petal
is_valid(nil)   // false
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
