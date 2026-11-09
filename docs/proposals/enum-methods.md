# Proposal: methods on enums

Status: proposed, 2026-11-09

## Summary

Let `fn Kind.describe(k: Kind, …)` declare a method on an enum, called as
`k.describe(…)` on any variant. It works exactly like a method on a class:

- The same declaration syntax.
- The same receiver-first rule.
- The same pinning, when the receiver's enum is known statically.
- The same runtime dispatch on the receiver's label otherwise. For a variant,
  the label is the enum name in its tag (`"Kind.park"` → `Kind`).

**Recommendation: adopt the small version.** Instance methods only. A method
name may not equal a variant name, a payload field, a data key, `name`, `all`
or `from`. Enum-level functions (`Kind.parse(s)`) are deferred.

What decided it:

- **The machinery is already there.** Dispatch already looks methods up by
  label name (`vm/calls.rs:345-363`), and a variant already carries its enum's
  name in its tag (`enum_table.rs:4-8`). The change mostly removes a refusal
  and teaches two lookups about variants.
- **Behavior that depends on the payload has no good home today.** Constant
  data is fixed per variant, and a function held as data never receives the
  variant it belongs to. The only form is a free function, and method syntax
  cannot reach a user's free function (`Ty.float.text()` fails with "No method
  'text' on enum Ty").

**Strongest argument against:** it is sugar. Every site in the corpus works
today as a free function, and it adds a third way to attach behavior to
variants (beside a `match` in a free function and functions held as data).

## Motivation

The refusal, `core/src/compiler/mod.rs:3287-3292`:

```text
cannot declare a method on `Ty`: `Ty` is an enum, and methods are declared on
classes only (write a function that takes the variant, `fn name(v: Ty, …)`)
```

**Corpus count.** I scanned every `.ptl` file in `examples/`, `garden/`,
`core-libs/`, `core-runtime/`, `test/`, `~/cheesecake/games`,
`~/petal-creative` and `~/worlds-fair/ui/ptl`. The scanning script is in the
session scratchpad. 19 files declare 28 enums. 17 free functions take an enum
as their first parameter, either annotated with it or matching on it:

| Site | Function | Shape |
|---|---|---|
| `core-runtime/glsl/src/glsl.ptl:54-73` | `ty_text`, `is_value_type`, `is_vec`, `is_num` | 4 functions, 45 call sites (`ty_text(` 27, the predicates 18) |
| `examples/games/tetris/tetris.ptl:64`, `:112` | `piece_cells(kind, rot)`, `kick_offsets(kind, from, to)` | computed from data plus arguments |
| `examples/console/enums.ptl:21` | `area(shape)` | match on payload |
| `examples/console/pattern_matching.ptl:48`, `test/match-enum-eval/main.ptl:7` | `eval(expr)` | recursive match |
| `examples/console/enums_and_match.ptl:77`, `:98`, `:166` | `upkeep`, `apply`, `describe` | `upkeep` is per-variant constant |
| `examples/productivity/paint/app.ptl:178`, `:182`, `:338` | `mk` (×2), `tool_icon` | `mk` is not about the tool; `tool_icon` draws one |
| `examples/console/reactive_ui.ptl:53` | `todo_app(event)` | a reducer, not a method |
| `test/benchmarks/match_dispatch.ptl:156` | `by_variant` | benchmark |

Seven of the 17 (`ty_text`, `is_vec`, `is_num`, `is_value_type`,
`piece_cells`, `kick_offsets`, `area`) are what a method is for: a question
asked of the variant. Five (`mk` ×2, `todo_app`, `by_variant`, `apply`) are
not. `upkeep` is better as constant data than as either. This is a modest
pattern: most enums in the corpus are plain tags read through data
(`Tool.brush.label`) or matched where they are used.

## Design

```petal
enum Ty
  float(dim: 1), vec2(dim: 2), vec3(dim: 3),
  named(id, dim: 0),
  unknown(dim: 0),
end

fn Ty.text(t: Ty) -> string
  match t
    when Ty.named(id) -> id
    when Ty.unknown -> "?"
    when _ -> t.name
  end
end

fn Ty.is_vec(t: Ty) -> bool
  t.dim > 1
end

print(Ty.vec3.text(), Ty.named("Fx").text(), Ty.vec2.is_vec())   // vec3 Fx true
```

**Declaration.** `fn E.m(recv, …)`, where `E` is an enum declared in the same
file or package (see Open questions for imported enums). It follows every rule a class method follows:

- top-level only and hoisted;
- the receiver is the first parameter, and a method with none is refused;
- overloading by arity;
- annotations are read at the call site.

**Resolution of `v.m(args)`**, the class order (language-guide.md,
"Resolution order") with "class" read as "enum":

1. A callable payload field or data value named `m` (already step 1 today,
   `vm/calls.rs:315-340`).
2. A user-declared method of the receiver's enum. The enum is the part of the
   tag before the last `.` (`program::split_enum_tag`).
3. (Enums have no built-in methods.)
4. The declaring slot's enum, when the receiver's label names nothing here
   (the stale-label path, `vm/calls.rs:368-395`).
5. A builtin with the receiver first (`k.str()`), as today.

**Pinning.** A receiver whose type is `Type::EnumOf(E)` is pinned to `E`'s
method and compiled as a direct `Call`, just as a class-typed receiver is
(`typecheck/mod.rs:2245-2287`, `compiler/expr.rs:150-205`). This covers a
constructor (`Kind.park`), an annotated parameter, and a `let` bound to
either.

**Names a method may not take**, each an error naming both declarations:

- **A variant name of its enum** (`fn Op.fight` beside the variant
  `Op.fight`). Otherwise `Op.fight(…)` builds a variant while `op.fight()`
  calls a method.
- **A payload field or data key of any variant of its enum.** Otherwise step 1
  would answer first, and a non-callable field would leave the method
  unreachable from that variant only.
- **`name`, `all`, `from`.** These are the names the enum already reserves
  (`compiler/enums.rs:142-154`, `:668-697`).

Enums can be stricter than classes here. A class method named like a field is
accepted today and is dead: `fn P.x` beside the field `x` compiles, `p.x`
reads the field, and `petal lint` is silent (checked in the scratchpad). That
should become a warning for classes too; see Open questions.

**Error when no method answers:** unchanged, "No method 'describe' on enum
Kind".

**Host-built variants.** A variant whose tag names no enum of the program (an
output-buffer tag like `"rect"`, `ffi.md:500-505`) has no enum to dispatch to.
Steps 2 and 4 find nothing, and the call falls to step 5 or the error, as it
does today.

## Before and after

The "before" snippets are quoted from the files named; each runs today.
`examples/console/enums.ptl` and `enums_and_match.ptl` run clean and are in
`test/example-golden/`; `tetris.ptl` passes `petal lint`; glsl.ptl passes
`core/tests/fn_introspection.rs`. The "after" snippets are **proposed**. No
prototype was built.

### 1. The common case: a question asked of the variant

Before (`examples/console/enums.ptl:21-27`, and its call `area(s)`):

```petal
fn area(shape)
  match shape
    when Shape.Circle(r) -> 3.14159 * r * r
    when Shape.Rectangle(w, h) -> w * h
    when Shape.Triangle(b, h) -> 0.5 * b * h
  end
end
```

After:

```petal
fn Shape.area(shape: Shape) -> float
  match shape
    when Shape.Circle(r) -> 3.14159 * r * r
    when Shape.Rectangle(w, h) -> w * h
    when Shape.Triangle(b, h) -> 0.5 * b * h
  end
end
// s.area()
```

The body is the same. The name now lives with the enum, so
`area` no longer takes up a top-level name that a `Rect` helper might also
want. Chaining reads left to right: `pick().area()`. Callers that
spell `area(s)` must change; there is no automatic forwarding.

### 2. Behavior that depends on the payload (glsl)

Before (`core-runtime/glsl/src/glsl.ptl:54-73`, call sites like
`fail(c, "expected " ++ ty_text(ty) ++ ", found " ++ ty_text(v.ty), node)`):

```petal
fn ty_text(t: Ty) -> string
  match t
    when Ty.named(id) -> id
    when Ty.unknown -> "?"
    when _ -> t.name
  end
end

fn is_vec(t: Ty) -> bool
  t.dim > 1
end
```

After:

```petal
fn Ty.text(t: Ty) -> string
  ...
end

fn Ty.is_vec(t: Ty) -> bool
  t.dim > 1
end
// fail(c, "expected " ++ ty.text() ++ ", found " ++ v.ty.text(), node)
```

There are 45 call sites. `is_vec(a.ty) && is_num(b.ty)` becomes
`a.ty.is_vec() && b.ty.is_num()`. Constant data cannot express `text`,
because the answer for `named` is its payload. A data function
(`text: fn() -> …`) cannot either, because it is never given the variant.

### 3. Computed from data plus arguments (tetris)

Before (`examples/games/tetris/tetris.ptl:64-79`, called at `:267`, `:359`,
`:378`):

```petal
fn piece_cells(kind: Piece, rot: int) -> list
  let n = kind.box
  let cells = kind.cells
  if kind == Piece.O then
    cells
  else
  ...
end
// let cells = piece_cells(kind, rot)
```

After:

```petal
fn Piece.cells_at(kind: Piece, rot: int) -> list
  ...
end
// let cells = kind.cells_at(rot)
```

The method cannot be called `cells`, because `cells` is a data key and the
naming rule refuses it. This is the rule doing its job, at the cost of an
invented name.

### 4. Where the old form is better: a fixed value per variant

Before (`examples/console/enums_and_match.ptl:77-83`):

```petal
fn upkeep(kind: Kind) -> int
  match kind
    when Kind.park | Kind.plaza -> 2
    when Kind.parkway -> 5
    when Kind.bridge -> 9
  end
end
```

A method `fn Kind.upkeep` would be no better. Constant data is the right form,
and it exists today: `park(…, upkeep: 2)` and `kind.upkeep`. The example keeps
the `match` deliberately, to show exhaustiveness. The guide should say: data
for a fixed value per variant, a method for a computed one. Without that rule
of thumb, authors will write methods that are tables.

### 5. A variant in `state` across an edit

```petal
enum Mode
  idle, run(speed),
end
fn Mode.label(m: Mode) -> string
  match m
    when Mode.run(s) -> "run {s}"
    when _ -> m.name
  end
end
state m: Mode = Mode.run(3)
print(m.label())
```

Edit: rename the enum to `Gait`, and the method to `fn Gait.label`.

- **The pinned call** (`m` is annotated) binds to `Gait.label` and runs the
  new code. The value still carries the tag `Mode.run`, so inside the method
  `match` sees a variant of an enum that no longer exists. Its arms do not
  match it, and `when _` answers `m.name`.
- **An un-annotated `state m = Mode.run(3)`** dispatches on the label `Mode`.
  It finds nothing, and the stale-label step 4 uses the declaring slot's enum.

This is exactly the class behavior described in language-guide.md ("When a
label outlives its class") and hot-reload.md ("Editing an enum"). Methods add
no new reload outcome. A method edit alone is an ordinary function edit
(patched or recompiled like any `fn`).

## Assessment

### Project goals

**Findings.**
- Live coding: neutral. The reload behavior is the class behavior (pair 5).
- Introspection: a pinned method call is a `Call` naming its callee, so
  slicing sees it, the same as a class method (`compiler/expr.rs:150-170`).

**Opinion.** This is generic language surface, not something Petal is
distinctive at. It earns its place by consistency: classes have methods,
and enums, the other declared type, refuse them with a message that has to
explain why.

### Ease of writing

**Findings.**
- Nothing new to learn for anyone who knows class methods. The declaration is
  already the form people try: the refusal text exists because people write
  it.
- Rust (`impl Enum`), Swift (methods in the enum body), Kotlin (members of a
  sealed class) and Java (enum methods) all allow it. Someone coming from
  those expects it.
- The new error a writer can hit is the naming rule ("method `Piece.cells`
  has the name of a data key of `Piece`"), which is needed and explains
  itself.

**Opinion.** Two ways to attach a value to a variant (data vs method) need the
guide's rule of thumb (pair 4). With it, this reads well six months later.

### Bug-freedom

**Findings.**
- **Prevents** a method silently shadowed by a field, through the naming rule.
- **Becomes possible:** none new for annotated receivers, which are pinned.
  For an un-annotated receiver whose value is not a variant (`nil`, a record),
  `x.label()` is a runtime "No method" error, as for classes.
- The checker sees misuse when the receiver is typed: arity and arguments are
  checked against the declaration (`check_method_call`).
- Oracle: dispatch is in one place (`do_method_call`), shared by the optimized
  and `--no-opt` runs. A new example joins the golden corpus.

### Performance

**Findings.**
- **Pinned calls** (an annotated parameter, a constructor) compile to a plain
  `Call`, so they cost the same as the free function.
- **Unpinned calls** do one extra lookup before falling through:
  `split_enum_tag` on the interned tag text plus a hash lookup of the enum
  name in `stack.methods`. The enum table already maps a tag to its slot by
  integer (`enum_table.rs:16-21`), so a slot→enum-name or slot→method-table
  index would keep it integer work.
- **When unused:** zero. The only new branch is reached by a method call on
  an enum variant, which today is an error unless step 1 or 5 answers.
- **Benchmark:** `test/benchmarks/match_dispatch.ptl` covers match on
  variants. A method-call variant of it would show the unpinned cost. Not
  measured: no prototype.

### Live coding and hot reload

**Findings.** Covered by pair 5. Method declarations register through the same
`DECLARE_METHOD_BUILTIN` path (`compiler/function.rs:348-375`). The method
table is per run and keyed by name, so a reload replaces it as it does for
classes. Nothing about `state` changes: variants stay tag plus payload.

### Introspection, debugging and program manipulation

**Findings.** None. A pinned call is a `Call` with its callee as an input. An
unpinned one is a `MethodCall` whose may-edges `dispatch_targets` recovers, as
for classes. `fn_info` and `fn_ast` see a method as an `FnDecl` with `class:
"Ty"`; the field name is historical and is reused.

### Maintainability of the implementation

**Findings.** The stages touched:

- **Compiler** (`compiler/mod.rs:3270-3300`): register the method under the
  enum name instead of refusing it. Add the naming checks next to
  `check_enum_variants` (`compiler/enums.rs:663`).
- **Class table** (`classes.rs`): enums need a methods list like `ClassDef`'s,
  or methods move to a table keyed by type name that both share. The second is
  cleaner and is most of the work.
- **Checker** (`typecheck/mod.rs:2245`): accept `Type::EnumOf` in
  `check_method_call`.
- **VM** (`vm/calls.rs:345`): a `Value::EnumVariant` arm beside the
  `Value::Map` arm in steps 2 and 3.5.
- **LSP and docs:** completion of `k.` should list methods. Docs change in the
  language-guide Enums section (replace the refusal mention), the
  method-resolution section ("class or enum"), and the CHANGELOG.

No new invariant. One special case is removed: enums stop being the
declared type that methods refuse. The C bridge and `ffi.md` are unaffected.

### Domains

- **Graphics** (glsl): helps, 45 call sites read better.
- **Games** (tetris pieces; `Phase` in snake and flappy is matched in place):
  helps a little.
- **Productivity and UI** (paint's `tool_icon` could be `tool.draw_icon(r,
  c)`): helps a little.
- **Dashboards, web, editor scripting, embedding hosts:** neutral. No enum
  free functions in garden or cheesecake's city_broker.

### Compatibility and migration

**Findings.**
- Purely additive: nothing that compiles today changes meaning. The only
  programs that change behavior are ones that failed to compile with the
  refusal.
- Migrating a free function to a method is optional and manual. It could be
  a `petal suggest` kind ("this function's first parameter is an enum it
  matches on"), but that is not needed for adoption.
- No dependency on unfinished work. It does interact with the name-collision
  work: a method is reached through its enum, so it adds no top-level name.

### Prior art

- **Rust** `impl Kind { fn area(&self) }`: methods on any enum, with `match
  self` inside. This is the design proposed here.
- **Swift**: methods and computed properties in the enum body. Computed
  properties are the analogue of constant data plus methods together.
- **Kotlin** sealed classes: each subclass can override. Per-variant method
  bodies are the "data holds a function" form, and Kotlin found both forms
  useful.
- **Java** enums: constant-specific bodies, rarely used and widely considered
  awkward. That supports not adding per-variant method bodies.
- **OCaml**: no methods on variants. Functions in a module beside the type
  (`Shape.area s`) serve. That is the "enum-level function" form deferred
  here, and it is close to the do-nothing option.

## Alternatives considered

- **Do nothing.** Free functions work, and the refusal message says how to
  write them. The cost is top-level names (`area`, `describe`, `eval` are
  generic and collide) and no method chaining. Reasonable if the language
  surface is being held still.
- **Functions held as data** (`Scale.double.apply(4)`, language-guide.md
  "Constant data"). This exists, but the function does not receive the
  variant, so it cannot read the payload. It suits per-variant strategies,
  not questions about the value.
- **Let method syntax reach user free functions** (step 5 for any function in
  scope, so `t.ty_text()` calls `ty_text(t)`). This is broader: it changes
  resolution for every type, makes any free function look like a method of
  everything, and makes `x.name()` depend on what is in scope. Rejected as a
  larger and riskier change for the same benefit.
- **Enum-level functions** (`fn Kind.parse(s: string) -> Kind`, called as
  `Kind.parse(s)`). Useful (OCaml's module functions, Rust's associated
  functions), but `Kind.x(…)` already means "construct variant `x`", and
  `Kind.from` is a built-in of that form. Deferred, so it can be designed
  with the module-namespacing work.
- **Smallest version.** What is proposed is already the smallest: instance
  methods only, no built-in methods, no per-variant overrides.

## Migration and compatibility

Additive; no existing program changes. Optional rewrites:

- glsl.ptl: 4 functions, 45 call sites.
- tetris: 2 functions, 4 call sites.
- enums.ptl: `area`.

The examples should show one method next to a `match`, with the data-vs-method
rule of thumb.

## Implementation sketch

1. Make the class table's methods keyed by declared-type name (class or enum),
   so the method list, arity checks and `declared_methods` are shared.
2. In the compiler, drop the enum refusal and register enum methods. Add the
   naming checks (variant, payload field, data key, `name`/`all`/`from`) with
   errors that name both sites.
3. In the checker, pin `Type::EnumOf(E)` receivers in `check_method_call`.
4. In the VM, add the `EnumVariant` arms in `do_method_call` steps 2 and 3.5,
   resolving the enum from the tag. Optionally key it by enum slot through
   `EnumTable` to avoid the string split.
5. Tests:
   - compiler unit tests for each naming error;
   - a `backend/bytecode/tests.rs` case run optimized and `--no-opt`;
   - a hot-reload test for pair 5 (enum renamed with `state` holding a
     variant, pinned and unpinned);
   - an example under `examples/console/` added to the golden corpus;
   - a method-call case in `match_dispatch.ptl`.
6. Docs: language guide (Enums, Resolution order), CHANGELOG.

Roughly a day, most of it step 1.

## Open questions

- **Class methods that a field shadows.** Should they warn, as this proposal
  makes them an error for enums? Today `fn P.x` beside a field `x` is
  silently dead (scratchpad check). The language owner decides whether
  classes get the error, a lint warning, or stay as they are.
- **Enum-level functions** (`Kind.parse(s)`): design with the module
  namespacing proposal, since both are about what `Name.member` means.
- **Methods on enums from another module.** Should a file be allowed to
  declare `fn ui.Align.flip(…)` on an imported enum? Classes allow methods on
  built-in classes. For enums, allowing it lets two importers declare the same
  method. The proposal allows only enums declared in the same file or
  package, until the collision rules are settled.
- **Per-variant method bodies** (Java's constant-specific methods): out of
  scope. A `match` in the method covers it.
