# glsl

**Experimental.** Translates a Petal function to GLSL, written entirely in
Petal. A host that draws with shaders can take a shader hook as a Petal lambda
and hand the engine the GLSL this library writes for it.

```petal
import glsl

fn weight(i: int, n: int) -> float
  1.0 - abs(i) / (n + 1.0)
end

print(glsl.to_glsl(weight))
// float weight(int i, int n) {
//     return 1.0 - float(abs(i)) / (float(n) + 1.0);
// }
```

The full description is [docs/petal-to-glsl.md](../../docs/petal-to-glsl.md):
the subset that translates, the environment record a host passes, how
captures become uniforms, and what is still open.

| | |
|---|---|
| `to_glsl(f, env?, opts?)` | the GLSL of `f` as one string; a refusal is a `#error` line with the message and position |
| `glsl_function(f, env?, opts?)` | the same as parts: `{error, kind, code, common, params, uniforms, taps, code_hash, ...}` |
| `glsl_error_text(err)` | `"message [line L, column C]"` |

It needs no host layer and no natives beyond core Petal: it reads the function
with the `fn_info` and `fn_ast` builtins
([docs/function-introspection.md](../../docs/function-introspection.md)) and
never runs it.

## Caching

The result is kept in keyed `state` under the function's code hash, so calling
`to_glsl` every frame translates once, and again only after an edit that
changes the function, a function it calls, or a captured int. Captured floats
and vectors are uniforms and never cause a translation.

## Tests

`tests/effects.ptl` holds eleven post effects as Petal lambdas and
`tests/expected/` the GLSL the original Python stand-in wrote for them;
`tests/unsupported.ptl` holds lambdas that must be refused. These files call
shader functions (`src`, `fx_luma`, ...) that only exist in a shader, so they
are translated, never run. `core/tests/fn_introspection.rs` checks all of it:

```bash
cd core && cargo test --test fn_introspection
```
