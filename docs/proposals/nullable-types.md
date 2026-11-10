# Proposal: nullable types (`T?`)

Status: **phase 1 accepted and shipped, 2026-11-09** (annotation only, the
recommended version). Phase 2 (strict nil checking) stays deferred and
undecided. See [Implementation status](#implementation-status) for what
shipped and the open questions it settled.

## Summary

Add `T?` to the type-annotation vocabulary, meaning "a `T` or `nil`". It is
allowed wherever an annotation is: parameters, return types, `let`, `var`,
`state` and class fields.

**Recommendation: adopt a smaller version (phase 1), and defer phase 2.**

**Phase 1: annotation only.**
- `nil` and `T` are both assignable to `T?`.
- A `T?` value is assignable to `T` with no warning: the checker does not yet
  track where nil can flow.
- `petal suggest` proposes `T?` where its evidence is a `T` and a `nil`,
  instead of dropping the slot.
- No runtime, IR or VM change. No lexer or parser change: `int?` is already one
  token, because identifiers may end in `?` (`is_valid?`).

**Phase 2: strict nil checking, deferred.**
- Infer `T?` from `if … else nil`, `?.` and `??`.
- Narrow after `if x != nil` and `if x == nil then return`.
- Warn when a `T?` reaches a `T` slot.

This needs a prototype measured against the corpus before it is decided. It is
the part that would produce new warnings in existing code.

## Motivation

Petal has no way to annotate a slot that may hold `nil`, so authors leave such
slots unannotated. Measured over the corpus (898 `.ptl` files in this repo,
`~/cheesecake/games`, `~/petal-creative` and `~/worlds-fair/ui/ptl`, not
counting `target/` or `node_modules/`):

| Pattern | Count |
|---|---|
| Parameters with a `= nil` default | 73 |
| …of those that carry an annotation | **0** |
| Annotated parameters overall | 5,049 |
| `return nil` lines | 601 |
| Top-level functions mixing `return nil` with a value (heuristic, a lower bound) | 97 of 7,959 |
| `state` / `var` initialized to `nil` | 124 |
| `if x == nil then return …` guards | 154 |
| `!= nil` comparisons | 1,722 |
| `??` uses | 1,859 |

The zero is the finding. Authors annotate thousands of parameters, but never
one that defaults to `nil`, because the annotation would warn:

```text
fn g(x: string = nil)   // warning: default value for parameter `x`: expected `string`, found `nil`
```

The same holds for class fields, where nil is the natural "absent" value.
glsl.ptl (f27a7bea) leaves six fields unannotated for this reason
(`Val.lit`, `Val.cv`, `FnState.loop_factor`, `FnState.ret_ty`, `Ctx.hook`,
`TranslationState.err`), and three `node` parameters (`fail`, `param`,
`helper`). `class V lit: int end; V(nil)` warns "field `lit` expects `int`,
found `nil`".

**What the checker does with nil today** (from `core/src/typecheck/mod.rs` and
`core/src/types.rs`):
- `Type::Nil` exists and is the type of the literal `nil` (`mod.rs:1610`).
  It is assignable only to `nil` and `any` (`types.rs:190-199`).
- A branching expression joins to `any` unless both arms agree (`join`,
  `mod.rs:507`). So `if c then 1 else nil end` is `any`, and nil almost never
  reaches a checked slot statically. In practice, a warning appears only for a
  literal `nil`, a `let k = nil` binding, or a `nil` default.
- `??` types as `any` (`mod.rs:2908`). `?.` only suppresses the
  missing-field warning (`mod.rs:1916`). There is no flow narrowing.
- The corpus is nearly warning-free. Linting 724 files outside `test/` gives 17
  warnings, none about nil. Phase 1 therefore cannot remove existing warnings.
  It removes the reason nil-able slots are left unannotated.

**`petal suggest` gives up on the same slots.** `widen(Nil, T)` is `None`
(`core/src/typecheck/infer.rs`, `widen`). A return type with evidence
`record` and `nil` produces no suggestion. Example: `fuzzy_from` in
`examples/productivity/command-palette/app.ptl:276` has all three parameters
annotated and no return type.

**Native declarations already lie about nil.** `fn_info` and `fn_ast` are
declared `-> record` (`core/packages/core/src/introspect.ptl:8-9`), but both
return `nil` for a builtin. There is no honest annotation for them.

## Design

### Syntax

`T?` for any type name `T`, written where a type name goes today:
`x: int?`, `-> record?`, `state var hud: Hud? = nil`,
`class V lit: int?, end`. Nothing else changes in the grammar.

**The lexer already accepts it.** An identifier may end in a single `?`, but
stops before `??` and `?.` (`core/src/lexer.rs:1355-1368`). `expect_type_name`
therefore receives `int?` as one token. Today it resolves to an unknown type
name: "warning: unknown type name `int?`", treated as `any` (checked).

**One conflict to close.** A class or enum may be named `Foo?` today: `class
Foo? a: int end; Foo?(1)` runs. Under this proposal, `Foo?` in type position
means "nullable `Foo`". Declaring a class or enum whose name ends in `?` becomes
an error. The corpus has none.

**Normalized and refused forms:**
- `nil?` is a warning ("`nil?` is just `nil`"), and is **not** `-> nil`.
- `any?` is a warning ("`any` already includes `nil`").
- `int??` lexes as `int` followed by `??`, which is a parse error in type
  position, with a targeted message.

### Phase 1 rules

| From → to | Today | Phase 1 |
|---|---|---|
| `nil` → `T?` | unknown type (any) | ok |
| `T` → `T?` | unknown type (any) | ok |
| `U` → `T?`, with `U` not assignable to `T` | unknown type (any) | **warning**, as for `U` → `T` |
| `T?` → `T` | n/a | ok (no nil tracking yet) |
| `T?` → `any` / `any` → `T?` | n/a | ok |
| `int` → `float?`, `int` → `num?`, `Class` → `record?` | n/a | ok, by the existing rules |

- A field read on a `T?` value is typed as on `T`. Phase 1 adds no "may be
  nil" warning.
- **`-> nil` stays the only return type that turns off the implicit return.**
  `declares_nil` checks `resolved == Some(Type::Nil)` (`core/src/ast.rs:452`),
  which a nullable type never is. `-> int?` keeps the tail as the value.
- A `T?` return holds every `return` and the tail to `T`-or-`nil`. Today a
  `return nil` in a `-> T` function warns. Under `-> T?` it doesn't.

### `petal suggest`

`widen` gains one rule: `widen(T, Nil) = widen(Nil, T) = T?` for any `T` that
is not `nil` or `any`. Effects:
- **Return slot:** a tail of `record` with one `return nil` suggests
  `-> record?`.
- **Parameter slot:** call sites passing `string` and `nil` suggest `string?`.
  A `= nil` default counts as `nil` evidence.
- The veto from d6fd8291 is unchanged: an untypeable (`any`) tail or return
  still gives no suggestion. `?` is about `nil`, not about `any`.

### Phase 2 (deferred, sketched for the decision)

- `join(T, Nil) = T?`, `join(T, T?) = T?`.
- `a?.b` is `typeof(b)?`. `a ?? b` with `a: T?` is `T` when `b` is a `T`.
- Narrowing:
  - in the then-branch of `if x != nil`;
  - after `if x == nil then return … end`;
  - in the right operand of `x != nil && …`.
- **Warning:** "`x` may be `nil` here (declared `T?`); test it or use `??`"
  when a `T?` reaches a `T` slot or has a field read.

## Before and after

All "before" snippets are quoted from the corpus. Each was run, or its file
linted, with the release binary, and the glsl ones pass
`core/tests/all/fn_introspection.rs`. All "after" snippets are **proposed**: no
prototype was built.

### 1. A parameter that may be nil

Before (`core-runtime/glsl/src/glsl.ptl`, f27a7bea):

```petal ignore
// `node` is the construct refused, or nil for the function itself.
fn fail(c: Ctx, message: string, node) -> Val
  let ts = c.tr.read()
  if ts.err == nil then
    let line = if node != nil then node.line else c.line end
```

After:

```petal ignore
fn fail(c: Ctx, message: string, node: record?) -> Val
  let ts = c.tr.read()
  if ts.err == nil then
    let line = if node != nil then node.line else c.line end
```

The comment's half-sentence becomes the annotation. The checker now holds
callers to "a record or nil": passing a `string` warns, where today nothing is
checked at all. Phase 2 would additionally warn on `node.line` without the
`!= nil` test.

### 2. A return value that may be nil: the common case

Before (`examples/productivity/command-palette/app.ptl:276-300`, trimmed):

```petal ignore
fn fuzzy_from(q: list, tc: list, start: int)
  ...
    if found < 0 then return nil end
  ...
  {score: score - n / 12 + (if pos[0] == 0 then 4 else 0 end), pos: pos}
end
```

After:

```petal ignore
fn fuzzy_from(q: list, tc: list, start: int) -> record?
```

Every parameter was already typed; only the return type was missing. This is
the shape `petal suggest` would now propose. Its caller,
`let b = if ws > 0 then fuzzy_from(…) else nil end`, gains nothing in phase 1
(the join is still `any`). In phase 2 it would be `record?`, and the existing
`if a == nil then b elsif b == nil then a` would narrow correctly.

### 3. A `nil` default

Before (`core/prelude/std.ptl:158`):

```petal ignore
pub fn plural(n, one, many = nil)
  if n == 1 then
    "1 {one}"
  else
    "{commas(n)} {many ?? one ++ "s"}"
  end
end
```

After:

```petal ignore
pub fn plural(n: int, one: string, many: string? = nil) -> string
```

`plural(3, "mouse", 2)` is now a warning. Today the 73 `= nil` parameters
check nothing. Annotating them also documents the API in the LSP hover.

### 4. A class field

Before (glsl.ptl, f27a7bea):

```petal ignore
class Val
  code: string,
  ty: Ty,
  prec: int,
  // The value of an int literal (it can be re-printed as a float), or nil.
  lit,
```

After:

```petal ignore
  lit: int?,
```

Writing `Val("x", Ty.int, 8, "3", nil)` now warns. A misplaced positional
argument is the realistic bug here.

### 5. Interaction with `state` and with `-> nil`

Before (`examples/custom-apps/petal-fantasy-nes/carts/petal_quest/game.ptl:151-152`):

```petal ignore
// `nil` is the "not built yet" sentinel — the reader function for each cell is
// what fills it in.
state var _pq_game = nil
state var _pq_level = nil
```

After:

```petal ignore
state var _pq_game: record? = nil
state var _pq_level: record? = nil
```

An un-annotated `state` reads as `any` (`language-guide.md`, Type
Annotations). An annotation types every read and constrains every write, and
that is the only way to get checking on these 124 nil-initialized cells.
Annotations have no runtime effect, so hot reload and stored state are
unaffected.

`-> nil` and `-> T?` must not be confused. `fn toggle_flag(r: int, c: int) ->
nil` (`examples/games/minesweeper/minesweeper.ptl:107`) compiles its tail as a
statement. `-> int?` would not. The `nil?` warning above exists so that
nobody writes `-> nil?` expecting either behavior.

### 6. Where the old form is still the better choice

```petal ignore
let name = ast.name ?? (opts?.name ?? "f")         // glsl.ptl, translate()
let held = live.values?.[name]                     // glsl.ptl, glsl_function()
```

Locals produced by `??` and `?.` need no annotation, and should get none. In
phase 1 an annotation on such a local adds nothing, because nothing infers
`T?` yet. Lambdas cannot declare a return type at all, so `T?` does not reach
them. The guide should keep recommending "annotate the boundary (parameters,
returns, fields, state), not the locals".

## Assessment

### Project goals

**Findings.** The README lists "Type system with optional type annotations".
Phase 1 lets the annotations cover slots they can't cover today. It does not
touch live coding, introspection or program manipulation, beyond
`petal suggest` (goal-based editing tooling) becoming able to propose more.

**Opinion.** This is generic language surface, not something Petal should be
distinctive at, but it fills a gap the corpus shows authors working around.
It fits "optional types, warnings only".

### Ease of writing

**Findings.**
- `T?` is the spelling used by TypeScript (optional parameters), Kotlin, Swift,
  C# and Dart. Python's `Optional[T]` / `T | None` and Luau's `T?` are the
  other common forms. A JavaScript, Python or Lua writer will guess it.
- One new rule: `?` means nil-able in type position and is part of a predicate
  name elsewhere (`even?`). These cannot collide once `?`-suffixed class names
  are refused.
- Errors stay warnings with the current wording, plus the two normalization
  warnings (`nil?`, `any?`).

**Opinion.** Low learning cost. The risk is phase 1's permissiveness: a reader
may believe `T?` → `T` is checked when it isn't. The guide must say so plainly
until phase 2.

### Bug-freedom

**Findings.**
- Phase 1 catches wrong non-nil values in slots that today check nothing: the
  73 `= nil` parameters, nil-able fields, and the 124 nil-initialized cells.
- Phase 1 catches no nil bugs. "Field read on nil" is a runtime error today
  and stays one. Phase 2 is what targets it.
- Phase 1 creates no false positives, since it only widens what is accepted.
- Phase 2 would warn on existing code. Its volume is unknown without a
  prototype. The baseline is 17 warnings across 724 files, so any phase 2
  warnings will be prominent.
- No effect on the differential oracle, golden corpus or proofs: annotations
  do not reach the IR, except `-> nil`, which is unchanged.

### Performance

**Findings.** Checker-only. No change to runtime, IR or bytecode. The checker
cost is one more `Type` variant in `is_assignable_to` and `join`.
Unmeasurable against the compile time of a file. No benchmark is needed.

### Live coding and hot reload

**Findings.** No effect. Annotations are advisory and are not part of
`code_hash`'s runtime meaning. They are part of the AST, so editing one is an
ordinary source edit and recompiles the function, as editing any annotation
does today.

### Introspection, debugging and program manipulation

**Findings.**
- `fn_info` reports `type: "int?"`, the annotation as written, as it does for
  every annotation. Consumers that compare `p.type == "float"` (glsl's
  `ty_named`) see `"float?"` and fall back. glsl would refuse it with "has no
  GLSL form", which is correct, since GLSL has no nil.
- `petal suggest` gains the `T?` rule.
- `propose-edit`, provenance and direct manipulation do not read annotations.

### Maintainability of the implementation

**Findings.**
- `Type` is `Copy` and mostly fieldless. Class and enum types carry an id into
  the compilation's `ClassTable` (`types.rs:52-63`). The least invasive
  encoding is a new `Type::OrNil(NullableId)`, interned in the same table, or a
  `Copy` base enum. Making it a flag on every type would touch the roughly 450
  `Type::` matches in `typecheck/` and `types.rs`.
- Stages touched:
  - type name resolution (`Type::from_name` accepting a trailing `?`);
  - class and enum declaration (refuse `?` names);
  - `is_assignable_to`;
  - `display` / `name`;
  - `infer.rs` `widen`;
  - LSP hover, which renders the type;
  - the guide.
- No lexer, parser, compiler, IR, VM, formatter or C bridge changes.
- New invariant: `declares_nil` must keep matching exactly `Type::Nil`. A test
  should pin that `-> nil?` and `-> int?` keep the implicit return.
- Native declarations should be corrected to `T?` where they can return nil
  (`fn_info`, `fn_ast`, and `docs/Builtins.md` has 14 "nil when/if" mentions to
  audit).

### Domains

- **Graphics** (`to_glsl`): small help. Annotations in glsl.ptl.
- **Games**: helps. Entity lookups and "not built yet" `state` sentinels
  (petal_quest).
- **Productivity and UI**: helps. ui prelude functions take `id = nil` and
  `wants = nil` (`core-libs/petal-ui/prelude/_frame.ptl:312-331`), and `find`
  and selection results are nil-able.
- **Dashboards and data**: helps a little. Optional query results.
- **Web, editor scripting, config**: neutral.
- **Embedding hosts**: neutral. `native fn` declarations can say `-> record?`
  honestly.

### Compatibility and migration

**Findings.**
- Nothing breaks. `T?` is currently an "unknown type name" warning, and the
  corpus contains no `?`-suffixed type annotations and no `?`-suffixed class or
  enum names (both counted: 0).
- Migration is opt-in. `petal suggest --apply` can add `T?` where its evidence
  supports it, proven by its existing "compiles with no new warning" check.
- Phase 2, if adopted, is the compatibility event. It should ship behind a flag
  first (`petal lint --strict-nil`, or the `// @strict` pragma that
  `docs/dev/type-declarations-plan.md` §4 already anticipates).
- It closes off no future direction. The type plan's non-goals exclude
  parameterized types, unions and aliases. `T?` is none of those, and it
  doesn't need them. Union types (`T | U`) would subsume it, and `T?` would
  then be sugar for `T | nil`, as in Luau.

### Prior art

- **TypeScript.** `strictNullChecks` (2.0, 2016) shipped opt-in, and is still
  off unless `strict` is set, because turning it on in an existing codebase
  produces many errors at once. That is the model for
  phase 1 now, phase 2 behind a flag.
- **Kotlin, Swift.** Nullability in the type, with flow narrowing (smart
  casts, `if let`). Both are compiler errors from day one, which fits a
  language that started with them.
- **Python.** `Optional[T]` / `T | None` is checked only by external tools
  (mypy, pyright), which narrow after `is not None`. This is close to Petal's
  "advisory annotations", and mypy's `--strict-optional` default-flip
  (0.600, 2018) is the TypeScript story again.
- **Luau.** `T?` as sugar for `T | nil` in a gradually typed Lua. It is the
  closest analogue, with the same `nil`-heavy idioms (`x = x or default`).
  Luau narrows on `if x then`, which Petal cannot do as simply, because Petal's
  truthiness makes `0` and `""` falsy.

### Alternatives considered

- **Do nothing.** Slots stay unannotated. This is honest, but loses checking
  on the slots that are otherwise well-typed (`fuzzy_from`).
- **Make `nil` assignable to every type** (Java/Go-style, or TypeScript without
  `strictNullChecks`). The smallest change: two warnings disappear and nothing
  is added to the language. It costs the information. A reader can no longer
  tell `node: record` from "record or nil", `suggest` cannot express the
  difference, and phase 2 becomes impossible without a breaking re-tightening.
  The current warnings (`literal nil to T`) catch nothing in today's corpus
  (0 occurrences), so the protection lost is small. The documentation lost is
  not.
- **Union types `T | nil`.** More general, but a stated non-goal of the type
  plan. Not needed for the measured problem.
- **`?T` prefix** (Zig). It would need lexer work (`?` alone is a lex error
  today) and matches fewer languages.
- **A comment convention or `any`.** What glsl.ptl does now.

## Migration and compatibility

None required. Optional adoption:
1. `petal suggest --only types --apply` across the corpus to add `-> T?`
   where proven.
2. Hand-annotate `= nil` parameters, nil-able class fields and
   nil-initialized `state`.
3. Correct native declarations that can return nil.

## Implementation sketch

1. `Type::OrNil(id)` (or a `Copy` base enum); `Type::from_name` strips one
   trailing `?`; `nil?` and `any?` warnings; `display` appends `?`.
2. Refuse class or enum names ending in `?`.
3. `is_assignable_to`: `Nil → T?`, `T → T?`, `T? → T`, with the base type
   checked by the existing rules.
4. `infer.rs` `widen`: `(T, Nil) → T?`. Update `suggest` tests, including the
   d6fd8291 veto test.
5. Tests:
   - `declares_nil` is unaffected by `-> T?` and `-> nil?`;
   - class field and `state` annotations accept `nil`;
   - `x: int? = "s"` warns.
6. Docs: language guide (Type Annotations), the type plan's §4, `petal help
   suggest`.
7. Correct `fn_info` / `fn_ast` and audit the other native declarations.

Roughly 1-2 days for phase 1. Phase 2 is a separate project: flow narrowing
is new checker infrastructure.

## Open questions

- **Phase 2 at all, and its volume.** Prototype `join(T, Nil) = T?` plus
  narrowing, lint the corpus, and count new warnings before deciding. The
  language owner decides whether strict nil checking is ever on by default.
- **Narrowing on truthiness.** Should `if x then` narrow a `record?`?
  Petal's falsy `0` and `""` make that wrong for `int?` and `string?`. It
  would be correct only for types with no falsy values.
- **`T?` → `T` in phase 1.** Silent (as proposed) or an opt-in lint? Silent
  keeps phase 1 warning-neutral, at the cost of a reader overestimating what
  is checked.
- **Encoding.** Interned `OrNil(id)` vs a restructured `Type`. This is the
  implementer's call, but it affects how cheaply union types could follow.

## Implementation status

Phase 1 shipped 2026-11-09, following the implementation sketch:

1. **Encoding: `Type::OrNil(NonNil)`** (`core/src/types.rs`). `NonNil` is a
   `Copy` mirror of `Type`'s variants minus `any`, `nil` and `T?` itself, so
   `Type` stays `Copy`, needs no table to compare or assign, and neither
   `nil?`, `any?` nor `T??` is representable. Chosen over an interned id
   (which would have threaded the `ClassTable` into `is_assignable_to`) and
   over a flag on every type (which touches every `Type::` match). A union
   type would replace `OrNil` with a list of members later; nothing outside
   `types.rs` matches on the `NonNil` payload. `Type::from_name` strips one
   trailing `?`; `ClassTable::declared_type` does the same for class and enum
   names. `nil?` and `any?` resolve to `nil` and `any` and warn.
2. Class and enum names ending in `?` are refused (`ClassTable::name_taken`).
3. `is_assignable_to`: `nil` → `T?`; `U` → `T?` iff `U` → `T`; `T?` → `U`
   iff `T` → `U`. **`T?` → `T` stays silent** (open question settled as
   proposed). A field read on a `T?`, and a `T?` passed to a builtin, is
   checked as on `T`. A *method call* on a `T?` receiver is left to runtime
   dispatch rather than pinned to the class, since a nil receiver must not
   reach `fn Class.m` directly.
4. `petal suggest`: the `widen(T, nil) = T?` rule lives in `conclude`
   (`core/src/typecheck/infer.rs`) rather than `widen`, so it also composes
   with field-read evidence (`.x` reads plus a `nil` call site give
   `record?`). A `= nil` default is a new `Evidence::NilDefault`; it makes
   the conclusion `T?` but never concludes `nil` by itself. The d6fd8291
   `any` veto is unchanged and runs first.
5. Tests: `core/tests/all/nullable_types.rs` (assignability, defaults, fields,
   `state`, `-> T?` / `-> nil?` keeping the implicit return, `declares_nil`,
   spellings, the `int??` parse error, `?`-named classes and enums),
   `core/tests/all/suggest.rs` (nullable suggestions end to end), and unit tests
   in `types.rs` and `infer.rs`.
6. Docs: language guide (Type Annotations, "Nullable types"), the type
   plan's vocabulary, assignability and §4, `petal help suggest`.
7. `fn_info` / `fn_ast` are declared `-> record?`. The other natives that can
   return nil (`last`, `first`, `find`, `argmin`/`argmax`, `json_parse`,
   `binding`) declare no return type, so they needed no change.

`declares_nil` now also requires the name not to end in `?`, so `-> nil?`
(which resolves to the type `nil`) keeps the implicit return.

Not done in phase 1, by design: annotating the corpus (glsl.ptl's nil-able
fields, the `= nil` parameters). That is the opt-in migration above.

