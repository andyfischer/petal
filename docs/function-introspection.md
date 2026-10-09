# Function Introspection (experimental)

`fn_info(f)` and `fn_ast(f)` let a running program read a function value: its
parameters with their annotations and defaults, its declared return type, what
it captured, and its syntax tree. Both return plain records and lists.

**Experimental.** The record shapes on this page may change. The builtins were
added for [`glsl`](petal-to-glsl.md), a library that translates a Petal lambda
to GLSL; other uses (an editor listing a function's parameters, documentation
tools, other code generators) are expected, and will shape what the final form
looks like.

- [`fn_info(f)`](#fn_infof)
- [`code_hash`](#code_hash)
- [`fn_ast(f)`](#fn_astf)
- [Nodes](#nodes)
- [Limits](#limits)

## `fn_info(f)`

```petal
class Fx
  color: vec3,
end
let TAPS = 3
fn weight(i: int, n: int) -> float
  1.0 - i / (n + 1.0)
end
let streak = fn(fx: Fx, amount = 0.6, tint = #1a2a6c)
  fx.color * weight(1, TAPS) * amount
end
let info = fn_info(streak)
```

`info` is:

```petal ignore
{
  name: nil,                  // "weight" for fn_info(weight)
  file: nil, line: 8, column: 14,
  code_hash: "9f3a…",         // 16 hex digits, see below
  params: [
    {name: "fx", type: "Fx", has_default: false, default: nil, default_source: nil},
    {name: "amount", type: nil, has_default: true, default: 0.6, default_source: "0.6"},
    {name: "tint", type: nil, has_default: true, default: {r: 26, g: 42, b: 108}, default_source: "#1a2a6c"},
  ],
  returns: nil,               // "float" for fn_info(weight)
  captures: [
    {name: "weight", value: <function>},
    {name: "TAPS", value: 3},
  ],
}
```

- `type` and `returns` are the annotation's name exactly as written. The type
  checker's resolution is not included.
- `default` is the value of the default when it needs no evaluation: a
  literal, a negated number, a color literal, a list or record of those, or a
  `vec2(...)` / `vec3(...)` of numbers. Any other default (`b = a * 2`) has
  `has_default: true`, `default: nil`, and its text in `default_source`.
- `captures` lists the outer bindings the body reads, in order of first use,
  with the value the closure holds. A captured `var` is read through: the
  value is the cell's current contents. A function the body calls is a capture
  like any other, so a tool can follow it with another `fn_info`. A top-level
  constant declared *below* the function is not a capture and is not listed.
- For a builtin (`fn_info(floor)`) or an overloaded name the result is `nil`.
  A non-function argument is an error.

`fn_info` is cheap: the source-level part is worked out once per function and
kept with the program.

## `code_hash`

`code_hash` identifies the function's code. It covers the parameter list
(names, annotations, default expressions), the declared return type and the
body, plus the same for every function the closure captures, recursively.

| Edit | `code_hash` |
|---|---|
| Comments, blank lines, indentation, moving the function in the file | same |
| Another function added or removed elsewhere | same |
| A hot reload that does not touch the function | same |
| The value of a captured constant (`let TAPS = 3` to `4`) | same: a captured value is data. It is in `captures` |
| The body, a parameter, an annotation, a default | different |
| The body of a function it calls | different |

So it can key a cache that survives hot reloads:

```petal
fn describe(f)
  let info = fn_info(f)
  state(info.code_hash) text = expensive_description(fn_ast(f))
  text
end

fn expensive_description(tree)
  "{tree.tag} with {len(tree.body)} statement(s)"
end

print(describe(fn(x) -> x + 1))   // Lambda with 1 statement(s)
```

Two closures of the same `fn` expression made on different frames are
different values (`==` is false) and have the same `code_hash`.

A cache whose result also depends on captured values must add them to its
key; `glsl` does this for the captured ints it writes into the GLSL text.

## `fn_ast(f)`

```petal
let f = fn(x: float, k = 2)
  let y = x * k
  if y > 1.0 then y else 1.0 end
end
let tree = fn_ast(f)
print(tree.tag, len(tree.params), len(tree.body))   // Lambda 2 2
print(tree.body[0].tag, tree.body[0].name)          // Let y
print(tree.body[1].expr.condition.op)               // Gt
```

The root is `{tag, name, class, params, returns, body, file, line, column}`:
`tag` is `"Lambda"` or `"FnDecl"`, `params` is a list of
`{name, type, default}` where `default` is an expression node or `nil`, and
`body` is a list of statement nodes.

This is the tree the parser builds, before the compiler rewrites anything, in
a flat encoding made for walking from Petal:

- every node is a record with a `tag`, its own fields, and `line` / `column`
  (1-based, in the function's file);
- every field of a node is always present, `nil` when it does not apply, so
  reading one never fails;
- an `elsif` is the nested `if` it means: `else_body` is a list holding one
  `Expr` statement whose `expr` is the next `If`.

Two things the parser has already done show up in the tree: `set x += e`
arrives as `Set` with the value `x + e`, and a color literal `#1a2a6c` arrives
as a `Record` of three int literals `r`, `g`, `b`.

## Nodes

Statements:

| `tag` | Fields |
|---|---|
| `Let` | `name`, `type`, `value`, `is_var`, `is_config` |
| `Assign`, `Set` | `target`, `value`. `target` is `{kind, name, object, field, index}` with `kind` one of `"Name"`, `"Field"`, `"Index"` |
| `Expr` | `expr` |
| `For` | `var`, `iter`, `body` |
| `While` | `condition`, `body` |
| `Return` | `value` (`nil` for a bare `return`) |
| `Break`, `Continue` | |
| `State` | `name`, `type`, `init`, `key`, `is_var` |
| `FnDecl` | `name`, `class`, `params`, `returns`, `body` |
| `ClassDecl` | `name`, `fields` (a list of `{name, type}`) |
| `EnumDecl` | `name`, `variants` (a list of `{name, fields}`) |
| `Import` | `module`, `alias`, `names`, `star` |

Expressions:

| `tag` | Fields |
|---|---|
| `Literal` | `type` (`"nil"`, `"bool"`, `"int"`, `"float"`, `"string"`), `value` |
| `Ident`, `CellGet`, `AtVar` | `name` |
| `BinaryOp` | `op`, `left`, `right`. `op` is one of `Add Sub Mul Div Mod Eq Ne Lt Le Gt Ge And Or Coalesce Concat` |
| `UnaryOp` | `op` (`Neg`, `Not`), `operand` |
| `Call` | `function`, `args`, `arg_names` (empty when every argument is positional) |
| `If` | `condition`, `then_body`, `else_body` (a statement list or `nil`) |
| `For` | `var`, `iter`, `body` (a `for` used as a value) |
| `Match` | `subject`, `arms` (a list of `{pattern, guard, body}`; `pattern` is in the encoding of `petal show-ast --json`) |
| `List` | `items` |
| `Record` | `fields` (a list of `{name, value, spread}`) |
| `FieldAccess` | `object`, `field` |
| `IndexAccess` | `object`, `index` |
| `OptionalAccess` | `value` |
| `Block` | `body` |
| `Lambda` | `params`, `body` |
| `StringInterp` | `parts`, `exprs` |
| `Element` | `element`, `props` (a list of `{name, value}`), `children` (a list of `{text, expr}`) |

## Limits

- **No source, no tree.** `fn_ast` is `nil`, and `fn_info` has `nil` for
  `code_hash`, `line`, annotations and defaults, for a class or enum
  constructor and for a program loaded from IR (`petal run --ir`), which
  carries no source text.
- **Overloaded names are not inspectable.** A name declared with several
  arities is one value standing for several functions; both builtins return
  `nil` for it.
- **Annotations are names.** `type: "num"` is what was written, whether or
  not it resolves to a type.
- **A class's fields are not reachable from a parameter.** `fn(fx: Fx)` gives
  the name `"Fx"`; the fields of `Fx` are not in the function's tree. A tool
  that needs them takes them from its caller (`glsl` takes them in its
  environment record).
- **Positions in a cached result can go stale.** A reload that only moves
  code keeps `code_hash`, so a cache keyed on it keeps the `line` / `column`
  it stored. `fn_info` and `fn_ast` themselves always answer for the current
  text.
- **The C bridge does not expose this.** A function value still crosses the
  bridge as an opaque value.

How it works is in the module comment of `core/src/fn_introspect.rs`: the
program keeps its source and each function's span, and the function's file is
parsed on first use.
