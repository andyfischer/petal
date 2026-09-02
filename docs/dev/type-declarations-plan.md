# Optional Static Type Declarations

Status: **shipped.** Annotations parse, a warning-only checker runs on every
compile, and the warnings surface through the CLI, the LSP and the MCP tools.
This document records the design decisions and what was built. The
user-facing description is the "Type Annotations" section of
[language-guide.md](../language-guide.md).

Petal stays dynamically typed at runtime. Annotations add a compile-time
*check* on top; they never change how a program executes.

---

## 1. Goals and decisions

### Goals

- Optional annotations on `let`/`var`/`state` bindings, function parameters,
  and function return types.
- A shallow, local checker that catches obvious mismatches.
- Annotated and un-annotated code coexist freely in one file.
- An un-annotated binding gets an inferred type where one is obvious,
  otherwise `any`.

### Decisions

| Question | Decision |
|---|---|
| What happens on a mismatch | A **warning**. The program still compiles and runs. This matches the "forgiving types" line in [goals.md](goals.md). |
| Inference depth | Shallow and local: literals, called function signatures, and a table of builtin result types. Anything unclear is `any`. |
| Runtime checks | None. A dynamic value flowing into a typed slot is trusted. |
| Implicit casts | None. The checker never inserts a conversion; the programmer writes `int()`, `float()` or `str()`, which the checker treats as the sanctioned way to satisfy a type. |
| Syntax | Lowercase names, `:` on bindings and parameters, `->` on a named function's return type. Type names mirror what `type()` prints at runtime. |

### Non-goals

- No parameterized types (`list<int>`), arrow types, structural records, or
  user-defined aliases. `list`, `record` and `function` are opaque. Writing
  `list<int>` gets one targeted parse error rather than a confusing
  downstream one.
- No whole-program inference.
- No enum variant field annotations (`Circle(radius: float)`). The shared
  parameter parser already accepts them, but `EnumVariant.fields` is still a
  list of names, so the types are dropped. Cheap to add if wanted.

---

## 2. Syntax

```petal
let count: int = 0
let name: string = "Petal"

fn area(r: float) -> float
  3.14159 * r * r
end

fn greet(name: string)          // return type optional
  print("Hello,", name)
end

// annotated and un-annotated params mix freely
fn scale(v, factor: float) -> float
  v * factor
end

// lambda parameters can be annotated
let double = fn(n: int) -> n * 2
```

**Lambdas have no return annotation.** A lambda's `->` introduces its body,
so `fn(n: int) -> int -> n * 2` would need two arrows. Named `fn`
declarations get both.

**Type names are contextual, not reserved.** `int`, `float` and `str` are
also the cast builtins. The lexer keeps emitting identifiers; the parser
treats one as a type only after `:` or `->`. So `int(x)` keeps working and no
common words are reserved.

### Vocabulary

```
any  num  nil  bool  int  float  string (alias: str)
list  record  function  enum  vec2  f64_array  element  symbol  dual
handle  pending
<any class name>
```

Every name except `any`, `num` and class names is exactly what
`Value::type_name()` returns, so the name you see at runtime is the name you
write. `num` means "`int` or `float`", the contract most arithmetic actually
has. A class name (`Rect`, or a user class) is a type too; it resolves against
the compilation's class table.

### Assignability

- `any` is compatible with everything, in both directions. That is what lets
  dynamic and static code interoperate.
- `int` is assignable to `float` (mirrors runtime promotion). `float` to
  `int` is not; write `int()`.
- `int`, `float` and `dual` are assignable to `num`. `num` narrows to
  nothing without a cast. (`dual` is included because rejecting it would warn
  on working autodiff code.)
- A class instance is assignable to `record`, not the reverse.
- Otherwise the types must be equal.

---

## 3. What was built

### Type representation — `rust/src/types.rs`

`Type` is an enum with one variant per vocabulary entry plus
`Class(ClassId)`. `Type::from_name` parses the fixed vocabulary;
`Type::resolve(name, classes)` also handles class names. `is_assignable_to`
implements the rules above. `FnSignature { params, ret }` is a function's
declared signature. None of this is in the serialized IR: types are
compile-time side tables only, and codegen drops annotations to plain names.

### AST and parser

- `TypeAnn { name, resolved: Option<Type>, span }` is a written annotation.
  An unknown name is kept (not dropped) so the checker can warn on it, with a
  caret under just the type name.
- `Param { name, ty }`; `Let`/`State` carry `ty`; `FnDecl` carries `ret`.
- The parser builds both the direct AST and the CST event stream, and the CST
  projection is authoritative, so annotation syntax lives in four places:
  `parse.rs`, `cst/mod.rs` (`SyntaxKind::TypeAnnotation` / `ReturnType`),
  `cst_project.rs`, and `ast.rs`. A `debug_assert_eq!` in `cst/driver.rs`
  compares the two on every parse in debug builds.
- The tree-sitter grammar (`editor-support/tree-sitter-petal`) and the vim
  syntax file model the same syntax.

### The checker — `rust/src/typecheck/`

`check_module` runs from `compile_module` after the declaration pre-scan. It
keeps a scoped environment, infers bottom-up, and warns at these sites:

- unknown type name;
- a typed `let`/`var`/`state` whose initializer conflicts;
- reassignment (`=` or `set`) of an annotated binding, including from inside
  a closure;
- call arguments against declared parameter types, and argument *count*
  against every known arity (functions, constructors, methods);
- a function body's tail expression and every explicit `return` against the
  declared return type;
- a field read that the value's class does not declare;
- a discarded result of a known-pure builtin (`push(xs, x)` as a statement;
  `typecheck/unused.rs`).

Inference is deliberately conservative: a false negative is always preferred
to a false positive. Notes that follow from that:

- A `var` or `state` without an annotation reads as `any`, because a `set`
  can retype the cell from anywhere and this pass cannot see it. An annotated
  cell types every read and checks every write.
- Match-arm variables, loop variables and lambda parameters bind as `any`.
- `typecheck/builtin_types.rs` lists builtin result types, but only
  certainties: `len` is `int`; `abs`/`min`/`clamp`/`round` preserve
  int-ness; `reverse`/`slice`/`get` are absent because their result is a
  runtime question. **Treat additions to this table as correctness
  changes**: `lint --fix` deletes identity casts based on it, so a wrong
  entry becomes a wrong source rewrite, not just a spurious warning.
- Field and index writes (`set r.a = …`) are unchecked, because `record` and
  `list` are opaque.

The whole un-annotated corpus must stay silent: `petal check` over every
`.ptl` in the repo must not gain a warning from a checker change.

### Annotations drive static dispatch

An annotation buys more than a warning. When the checker pins a method-call
receiver to exactly one class, the compiler binds that call directly to
`fn Class.method` instead of emitting a runtime `MethodCall`. This is what lets
a live edit take effect on values whose class label predates it, and it gives
a slice the exact callee.

The guards are the design: a call stays dynamically dispatched whenever the
two mechanisms could disagree — a field of the same name (data beats
declarations), a method the class does not declare (a global native could
still answer), an arity no overload accepts, or a declaration written *below*
the call (nothing in Petal hoists). Each guard has a test in
`rust/tests/static_dispatch.rs`.

For an un-annotated `state`/`var` (typed `any`, so never pinned), the call
carries its declaration's class as a *hint*, consulted only when the
receiver's label names no class that still exists in the program. This keeps
`state c = C(1)` working across a rename of `C` without inferring a type from
a mutable binding's initializer, which would break polymorphism through one
binding (`state shape = Circle(2)` then `shape = Square(3)`).

Method declarations carry a signature too, so a pinned `r.center_x()` infers
its declared return type and checks its arguments. The built-in `Rect` is
fully typed: its fields are `num`, the edge accessors return `num`, and
`inset`/`offset` return `Rect` so they chain.

### Surfacing

- `petal check` prints warnings with carets to stderr and exits 0;
  `--json` adds a `warnings[]` array; `--strict` exits 1 when any warning
  exists (for CI).
- `petal run` prints warnings to stderr before executing. Under `--json` they
  still go to stderr so stdout stays clean for JSON consumers.
- MCP `CheckSnippet` forwards `check --json`; `TestSnippet` shows them via
  `run`'s stderr.
- `show-ast --json` serializes an annotation as `{"name": "int",
  "resolved": "Int"}` (`resolved` omitted for an unknown name).

### Docs and examples

`language-guide.md` (Type Annotations), `CLI.md` (`check`), `Builtins.md`
(casts), `goals.md` ("Types as a projection"), `examples/console/typed.ptl`
with its golden.

---

## 4. Still open

- **Structured warnings in `run --json`.** A `warnings[]` field on the run
  report would let `TestSnippet` return them as data instead of stderr text.
- **Enum variant field annotations** (see non-goals; cheap).
- **Parameterized and richer types**, user aliases, deeper inference — all
  deferred by design.
- **A per-file `// @strict` pragma** to opt a file into error-level
  enforcement. The warning channel was designed so this is a small addition.
- **Compile-time unknown-method warnings** were considered and dropped:
  dispatch also reaches embedder-registered natives the checker cannot see,
  so the warning would fire on working code.

---

## 5. Verification recipe

```bash
cd rust && cargo test --lib typecheck::        # checker unit tests
cargo test --test type_annotations             # the annotation grammar
cargo test --test static_dispatch              # dispatch pinning and its guards
cd ts && npx vitest run test/type-annotations.test.ts test/type-warnings.test.ts
cd editor-support/tree-sitter-petal && npx tree-sitter generate && npx tree-sitter test

B=rust/target/debug/petal
$B run examples/console/typed.ptl                  # clean, no warnings
$B check --json -e 'let x: int = "hi"'             # {"ok":true,"warnings":[…]}
$B check --strict -e 'let x: int = "hi"'; echo $?  # exit 1
$B check -e 'let xs: list<int> = [1]'              # "parameterized types are not supported"
$B check -e 'let r = Rect("a", 1, 2, 3)'           # field `x` expects `num`, found `string`
```
