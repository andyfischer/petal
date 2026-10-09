# Petal to GLSL (experimental)

`glsl` is a library, written in Petal, that turns a Petal function into GLSL
text. A host that draws with shaders can then take a shader hook as a Petal
lambda instead of GLSL inside a string:

```petal ignore
import glsl

let duotone = fn(fx: Fx, dark = #1b1040, light = #ffd9a0, amount = 0.6)
  let two = lerp(linear_to_srgb(dark), linear_to_srgb(light), fx_luma(fx.color))
  lerp(fx.color, two, amount)
end

let g = glsl.glsl_function(duotone, ENV)
// g.code:
//   vec3 two = mix(linear_to_srgb(params.dark), linear_to_srgb(params.light), fx_luma(fx.color));
//   fx.color = mix(fx.color, two, params.amount);
// g.params:  {dark: #1b1040, light: #ffd9a0, amount: 0.6}
```

**Experimental.** The library, its result record and the environment record
may change. It lives in [`core-runtime/glsl`](../core-runtime/glsl/) and is
built on the [function introspection](function-introspection.md) builtins
`fn_info` and `fn_ast`, which are experimental too.

It reads the function; it never runs it. The function only has to be in the
subset below, and code outside it is refused with the line and column of the
construct.

- [Using it](#using-it)
- [The environment](#the-environment)
- [Hooks and plain functions](#hooks-and-plain-functions)
- [The subset](#the-subset)
- [Captures](#captures)
- [Caching](#caching)
- [Errors](#errors)
- [How it compares with the stand-in](#how-it-compares-with-the-stand-in)
- [Still open](#still-open)

## Using it

Make the package importable (`petal run -I core-runtime app.ptl`,
`env.add_package("core-runtime/glsl")`, or `register_package` for a host
without a filesystem; see [core-runtime](../core-runtime/README.md)), then:

| | |
|---|---|
| `glsl.to_glsl(f, env?, opts?)` | the GLSL of `f` as one string: the functions it calls, then `f` itself. A refusal comes back as `#error to_glsl: <message> [line L, column C]`, which fails where the shader is compiled |
| `glsl.glsl_function(f, env?, opts?)` | the same as parts, for a host: `{error, kind, code, common, name, signature, params, param_types, uniforms, uniform_types, taps, taps_in_loop, code_hash}` |
| `glsl.glsl_error_text(err)` | `"message [line L, column C]"` for the `error` of a result |

The result of `glsl_function`:

| Field | |
|---|---|
| `error` | `nil`, or `{message, line, column}` |
| `kind` | `"hook"` or `"function"` ([below](#hooks-and-plain-functions)) |
| `code` | a hook: the statements of its body, one per line. A function: its whole definition |
| `common` | the GLSL functions `code` calls, `""` when there are none |
| `name` | a function: its GLSL name. A hook: `env.entry` |
| `signature` | a hook: `void <entry>(inout <Struct> <self>)` |
| `params`, `param_types` | a hook: `{name: default}` and `{name: glsl type}` for each parameter after the first |
| `uniforms`, `uniform_types` | `{name: current value}` and `{name: glsl type}` for each captured float, vector or color the code reads |
| `taps`, `taps_in_loop` | how many times the functions named in `env.count` are called, with constant-bound loops multiplied out (`nil` when a loop's bounds are not constant), and whether any call sits in a loop |
| `code_hash` | the function's [`code_hash`](function-introspection.md#code_hash) |

`opts`:

| | |
|---|---|
| `name` | a prefix for the GLSL functions emitted (`{name: "streak"}` names a helper `streak_weight`), and the name of a translated lambda |
| `inline_captures` | `true` writes captured floats, vectors and colors into the text as constants instead of making them uniforms ([Captures](#captures)) |

## The environment

The library knows Petal and GLSL and nothing about any engine. What the
shader side provides is the `env` record:

```petal ignore
let ENV = {
  // The structs a function may take as its first parameter, with field types.
  structs: {Fx: {color: "vec3", uv: "vec2", time: "float", resolution: "vec2"}},
  // Functions the shader has that Petal does not: argument and return types.
  functions: {
    src: {args: ["vec2"], returns: "vec3"},
    fx_luma: {args: ["vec3"], returns: "float"},
  },
  self: "fx",           // the struct's name in the shader (default: the parameter's name)
  result: "color",      // the field the body's value is assigned to
  uniforms: "params",   // uniforms are read as params.<name> ("" for bare names)
  count: "src",         // calls to count into `taps` (a name or a list)
  entry: "effect",      // the hook's function name in `signature` and `to_glsl`
}
```

A call of a name in `functions` is emitted as written, with its arguments
checked against the signature. Petal's checker does not know these names, so
the host should register them (or the script is checked with
`petal check --native src ...`).

## Hooks and plain functions

**A hook** is a function whose first parameter is annotated with a struct of
the environment (`fx: Fx`). That parameter is the shader's `inout` struct.
Every other parameter is a uniform and needs a default, which is its default
value; its type is its annotation, or what the default implies (a number is
`float`, a color is `vec3`, `vec2(...)` / `vec3(...)` are themselves). The
value of the body is assigned to `<self>.<result>`, and `return e` becomes
that assignment followed by `return;`.

**A plain function** is anything else. Every parameter needs an annotation
(`float`, `int`, `bool`, `vec2`, `vec3`, or a struct), the return type is the
declared one or what the body implies, and the result is one GLSL function:

```petal ignore
fn weight(i: int, n: int) -> float
  1.0 - abs(i) / (n + 1.0)
end
print(glsl.to_glsl(weight))
// float weight(int i, int n) {
//     return 1.0 - float(abs(i)) / (float(n) + 1.0);
// }
```

A function called from translated code is translated the same way, once, and
lands in `common`. It has to be a named `fn` or a lambda the code captures.

## The subset

| Petal | GLSL |
|---|---|
| `float`, `int`, `bool`, `vec2`, `vec3` | the same |
| a color literal `#1a2a6c` in a body | a linear `vec3` constant |
| `+ - * /` | the same. An int meeting a float or a vector is converted: a literal is re-printed (`1` becomes `1.0`), anything else is wrapped in `float(...)`. GLSL ES has no implicit conversion |
| `%` | `%` on ints, `mod(a, b)` on floats |
| `< <= > >= == !=`, `&& \|\| !`, unary `-` | the same; conditions must be real bools |
| `v.x`, `v.xy`, `fx.color` | the same |
| `floor ceil fract round sin cos tan asin acos atan exp log sqrt radians degrees normalize abs sign min max clamp smoothstep pow dot cross distance` | the same name |
| `lerp`, `mag(v)`, `mag(x, y)`, `atan2`, `hypot`, `pi()` | `mix`, `length(v)`, `length(vec2(x, y))`, `atan(y, x)`, `length(vec2(x, y))`, the constant |
| `vec2(...)`, `vec3(...)`, `float(x)`, `int(x)` | the same, arguments converted to float |
| `step`, `mod` | passed through (GLSL has them; Petal does not, so such a function cannot also run on the CPU) |
| `let`, `var`, `set`, `set x += e` | a typed declaration, an assignment. A second `let x` gets a new name (`x_1`); a name that is a GLSL keyword or function gets a trailing `_` |
| `if` as a statement, `elsif`, `else` | `if (...) { ... } else { ... }` |
| `if` as a value | `c ? a : b`, or a temporary assigned in both arms when an arm has statements |
| `for i in range(a, b)` (and a literal step) | `for (int i = a; i < b; i++)` |
| `while`, `break`, `continue`, `return` | the same |

Refused: strings, lists, records (other than a color literal), `match`,
`state`, `for` over a list or used as a value, lambdas as values, named
arguments, method calls, recursion, assigning to a parameter, and any
function that is neither in the table, in the environment, nor a translatable
Petal function.

Where the two languages differ at the edges, the translation follows GLSL:
floats are 32-bit, ints wrap instead of failing, `mod` floors where Petal's
`%` truncates (they differ for negative operands), and `round` at `.5` is left
to the GLSL implementation where Petal rounds away from zero.

## Captures

What a function reads from outside itself is a capture, and `fn_info` gives
its current value. By its type:

| Captured value | In the shader |
|---|---|
| an int, a bool | written into the text. Ints are structural: loop bounds, tap counts |
| a float, a `vec2`, a `vec3`, a color | a uniform, read as `params.<name>`; its current value is in `uniforms` on every call |
| a function | translated, as a helper in `common`. A helper's own captured floats are uniforms named `<helper>_<name>` |
| anything else | refused where it is read |

Making floats uniforms means the text depends on the code alone. A value that
changes every frame, or a constant being tuned live, changes a uniform and
compiles nothing. `{inline_captures: true}` writes them into the text
instead, for a host that has no place for extra uniforms; the text then
changes whenever such a value does.

## Caching

A script runs once per frame, so `to_glsl(fn ... end)` is called every frame
with a new closure. The translation is kept in keyed `state`, under the
function's [`code_hash`](function-introspection.md#code_hash), the captured
values that are written into the text, the environment and the options. A
frame with a cache hit costs one `fn_info` per function involved and a state
read; the syntax tree is not fetched.

Because the key is the code and not the closure, the cache survives hot
reloads. Measured by `core/tests/fn_introspection.rs`
(`a_script_that_translates_every_frame_translates_once_per_change`), for a
lambda with one helper, a captured int and a captured float that changes
every frame, with the call memo on and with it off:

| | Translations |
|---|---|
| the first frame | 1 |
| the next 11 frames (the captured float moves every frame) | 0 |
| a reload that adds a comment | 0 |
| a reload that adds an unrelated function and binding (a full recompile) | 0 |
| a reload that changes a captured float | 0 (the uniform's value changes) |
| a reload that edits the lambda's body | 1 |
| a reload that edits the helper | 1 |
| a reload that changes a captured int (a loop bound) | 1 |

A frame with a cache hit ran about 640 VM instructions for that whole script
(building the environment record included), against about 4,700 for the frame
that translates.

The call memo does not add to this today. It replays the capture walk inside
the library, but not the call of `glsl_function` itself: to match the call it
compares the closure's captures, which for a library function are the
library's other functions, and that comparison runs past the memo's budget.
The keyed state is what makes the call cheap.

Two limits. A reload that only moves code keeps the cached result, so the
`line` / `column` of a cached *error* can be stale until the function is
edited. And the cache is per `state`: a host that drops state on reload
translates again.

## Errors

`glsl_function` returns the first problem as `error`, with the position of
the construct in the function's own file:

```
a condition must be a bool, found float (GLSL has no truthiness) [line 24, column 45]
IndexAccess has no GLSL form [line 21, column 60]
no GLSL operation between vec3 and vec2 [line 36, column 44]
the function returns the new `fx.color` (vec3), found float [line 33, column 48]
param `amount` needs a default (it is the param's default value) [line 30, column 15]
```

It does not print and does not fail the run, so the caller decides how to show
it. `petal check` accepts all five of those programs: its inference is
shallow by design, and the translator does its own.

## How it compares with the stand-in

The mapping is a port of `petal2glsl.py`, the Python stand-in of the original
experiment (in the Cheesecake repository, `docs/petal-to-glsl-experiment/`),
which read `petal show-ast --json` offline. The stand-in's eleven effects and
its output are kept under `core-runtime/glsl/tests/`, and
`the_eleven_effects_translate_to_the_stand_ins_glsl` checks that the library
writes the same text, byte for byte, for all eleven (with
`inline_captures: true`, which is what the stand-in did).

Where the library differs:

| | Stand-in | Library |
|---|---|---|
| A captured float, vector or color | inlined if a compile-time constant, refused otherwise | a uniform by default; inlined with `inline_captures` |
| A captured `state` or other run-time value | refused (not available offline) | read from the closure, like any capture |
| Struct fields, environment functions | built in (Cheesecake's `Fx` and post-effect functions) | passed in `env` |
| Helper names | `gen_<effect>_<fn>` | `<opts.name>_<fn>` |
| Errors about a parameter or the result type | no position | the position of the parameter or of the result expression |
| A large or tiny float | Python's `repr` (`1e-07`) | Petal's `str` (`0.0000001`); the same value |
| An `if` value whose arm needs statements, tried first as `?:` | the names and tap counts used by the failed attempt are kept | the attempt is undone |
| Reserved names | GLSL keywords and environment functions | those, plus the GLSL functions the translator emits |
| Checks | none on arity of `clamp`, `lerp`, ...; none on assigning to a param | both refused |

## Still open

From the gaps listed by the investigation; none blocks translating a post
effect.

- **Other hooks.** A hook assigns one field. Surface, vertex and sky hooks
  write several fields and use `vec4` and matrices; that needs a design for "the
  lambda returns a record of the fields it changes".
- **`vec4`, matrices, integer vectors, arrays** do not exist in Petal, so
  code that needs them stays GLSL.
- **CPU-side vector math.** `floor(v)`, `1.0 / v`, `min(v, w)`, swizzles and
  `vec3(color)` fail when a translatable function is *run* in Petal, and
  `step` / `mod` are not Petal builtins. Translation is unaffected; running
  the same function on the CPU and the GPU is not possible in general yet.
- **A color is a record, not a `vec3`**, on the CPU.
- **Exact numeric forms.** `mod` and `round` are emitted directly; the forms
  that match Petal on negative operands and at `.5`
  (`a - b * trunc(a / b)`, `sign(x) * floor(abs(x) + 0.5)`) are not.
- **`map_range`, `safe_div`, `limit`, `rotate`** have no expansion.
- **The shader environment has no Petal side.** Names like `src` are known to
  the translator through `env` only; the checker needs them registered by the
  host, and Petal's `noise` is not the shader's.
- **Class fields come from `env`**, not from the class declaration.
- **Errors are returned, not raised.** A builtin that raises an error at a
  given source position would let the library fail at the lambda.
- **The C bridge** does not expose function values; a host takes the GLSL as
  a string from Petal code.
- **Speed in the VM** has only been measured roughly. Translating the eleven
  effects takes about 70,000 VM instructions in total and about half a
  millisecond each in a release build (macOS, one run of the whole file
  against the same file translating nothing). It is paid once per change.
