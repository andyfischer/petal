# Proposal: enum tags in `fn_ast`

Status: proposed, 2026-11-09

## Summary

`fn_ast(f)` returns a function's syntax tree as records whose `tag`, `op` and
`target.kind` fields are strings (`{tag: "BinaryOp", op: "Add", ...}`). Its
only Petal consumer, the `glsl` translator, dispatches on those strings in
about 45 places, and no tool can see a misspelt one.

Two designs were evaluated:

- **A. Full payload enum.** Every node is a variant: `AstNode.BinaryOp(op, left,
  right, line, column)`. It can be destructured in `match`.
- **B. Enum tags on records.** Nodes stay records. Only the discriminators
  become variants of fieldless enums: `{tag: AstKind.BinaryOp, op: AstOp.Add,
  left, right, line, column}`.

**Recommendation: adopt B.** Put the enums in an opt-in bundled `ast` package,
not in `core`. Decide that placement by prototype first (see Open questions).
**Reject A.** Petal patterns must list every payload field, so every pattern
would carry `line, column`. Adding a field to a node kind would break every
pattern that matches it. A payload field may not be called `name`, which the
tree uses on six node kinds. And it would break the documented promise that
reading any node field never fails.

B is a small change with a small payoff. It catches tag typos at compile time
and makes dispatch read as dispatch. The format is experimental and has one
user, so the payoff is mostly for future tools that read the tree. Deferring
is also a defensible outcome. If B is adopted, adopt it before a second
consumer appears.

## Motivation

The tree's encoding is in
[function-introspection.md](../features/function-introspection.md#nodes). It
documents 16 statement tags, 19 expression tags, 15 binary and 2 unary ops,
and 3 assignment-target kinds, all as strings. The strings come from Rust:
`op` is literally `format!("{op:?}")` of the Rust `BinOp`
(`core/src/fn_introspect.rs:569`). Renaming a Rust variant would silently
change what Petal programs see.

**Who reads it.** One Petal program reads `fn_ast` today:
`core-runtime/glsl/src/glsl.ptl`. Cheesecake's `tests/script_host_tests.cpp:4498`
only counts how many times the native is called. There are no other `.ptl`
users in this repo, `garden/`, `~/cheesecake/games`, `~/petal-creative` or
`~/worlds-fair/ui/ptl`.

**How glsl.ptl reads it** (as of f27a7bea):

| Pattern | Sites |
|---|---|
| `match node.tag` with string arms | 1 match, 11 arms (`glsl.ptl:459-472`) |
| `.tag == "..."` / `.tag != "..."` | 13 |
| `tag == "Let"`-style `if`/`elsif` chain in `stmt` | 9 |
| `op == "..."` / `node.op == "..."`, and `when "Add"` arms | 4 + 3 + 3 |
| `BINOPS` table keyed by op strings | 1 table, 12 keys |
| `.type == "float"` etc. on literals | 5 |
| `target.kind != "Name"` | 1 |

That is about 45 comparisons against strings no checker knows. A typo such as
`when "Litteral"` or `tag == "FieldAcess"` compiles and silently never
matches. The translator then refuses valid code with a misleading message
("Literal has no GLSL form"), or, in the `stmt` chain, falls through to the
`else` branch.

## Design (B)

A bundled package `ast` declares:

```petal
pub enum AstKind
  # statements
  Let, Assign, Set, Expr, For, While, Return, Break, Continue, State, Memo,
  FnDecl, ClassDecl, EnumDecl, NativeFn, Import,
  # expressions (For is shared with the statement)
  Literal, Ident, CellGet, AtVar, BinaryOp, UnaryOp, Call, If, Match, List,
  Record, FieldAccess, IndexAccess, OptionalAccess, Block, Lambda,
  StringInterp, Element,
end

pub enum AstOp
  Add(symbol: "+"), Sub(symbol: "-"), Mul(symbol: "*"), Div(symbol: "/"),
  Mod(symbol: "%"), Eq(symbol: "=="), Ne(symbol: "!="), Lt(symbol: "<"),
  Le(symbol: "<="), Gt(symbol: ">"), Ge(symbol: ">="), And(symbol: "&&"),
  Or(symbol: "||"), Coalesce(symbol: "??"), Concat(symbol: "++"),
  Neg(symbol: "-"), Not(symbol: "!"),
end

pub enum AstTarget
  Name, Field, Index,
end
```

`fn_ast` stays a `core` native and returns the same records as today, with
three changes:

- `tag` is an `AstKind` variant;
- `op` (on `BinaryOp` and `UnaryOp`) is an `AstOp` variant;
- `target.kind` is an `AstTarget` variant.

The root's `tag` is `AstKind.Lambda` or `AstKind.FnDecl`.

Unchanged:

- Every field is still present and `nil` when it does not apply.
- A literal's `type` stays a string: it uses the same vocabulary as `type(v)`,
  and `type(v) == "int"` is the idiom programs already use.
- `name` fields stay.
- `Match` arm patterns stay in the `show-ast --json` encoding, a separate
  problem.
- `code_hash` stays the same. It is computed from the Rust-side JSON before
  conversion (`fn_introspect.rs:206-208`), so no hash moves.

`tag.name` is the old string (`AstKind.Literal.name == "Literal"`), so code
that prints or concatenates a tag has a one-token migration.

Errors are the existing enum errors:

- `AstKind.Litteral` gives "enum `AstKind` has no variant `Litteral` — did you
  mean AstKind.Literal?"
- `node.tag == "Literal"` compiles, and is always false after the change.
  The checker should warn when a string is compared with an enum-typed field.
  That needs `fn_ast`'s return to be typed beyond `record` (see Open
  questions).

## Before and after

All "before" snippets are quoted from `core-runtime/glsl/src/glsl.ptl` at
f27a7bea, which passes `core/tests/all/fn_introspection.rs`. All "after" snippets
are **proposed**. No prototype was built.

### 1. Expression dispatch: the common case

Before (`glsl.ptl:457-473`):

```petal ignore
fn expr(c: Ctx, node: record) -> Val
  if failed(c) then return bad() end
  match node.tag
    when "Literal" -> e_literal(c, node)
    when "Ident" -> e_ident(c, node)
    when "CellGet" -> e_ident(c, node)
    ...
    when "For" -> fail(c, "a `for` used as a value builds a list, which has no GLSL form", node)
    when other -> fail(c, other ++ " has no GLSL form", node)
  end
end
```

After:

```petal ignore
fn expr(c: Ctx, node: record) -> Val
  if failed(c) then return bad() end
  match node.tag
    when AstKind.Literal -> e_literal(c, node)
    when AstKind.Ident | AstKind.CellGet -> e_ident(c, node)
    ...
    when AstKind.For -> fail(c, "a `for` used as a value builds a list, which has no GLSL form", node)
    when other -> fail(c, other.name ++ " has no GLSL form", node)
  end
end
```

The same number of lines. Each arm is eight characters longer. A misspelt arm
is now a compile error. An or-pattern merges the two `e_ident` arms, which is
also possible today with strings. `other.name` replaces `other`.

### 2. Operators: an enum with data replaces part of a table

Before (`glsl.ptl:128-135`, `561-600`):

```petal ignore
let BINOPS = {
  Add: {sym: "+", prec: 5}, Sub: {sym: "-", prec: 5},
  ...
}
...
  if op == "And" || op == "Or" then
  ...
  if op == "Mod" then
  ...
  let o = BINOPS?.[op]
  ...
    cv = match op
      when "Add" -> a.cv + b.cv
      when "Sub" -> a.cv - b.cv
      when "Mul" -> a.cv * b.cv
      when _ -> nil
    end
```

After:

```petal ignore
let BINOPS = {
  Add: {sym: "+", prec: 5}, Sub: {sym: "-", prec: 5},
  ...
}
...
  if op == AstOp.And || op == AstOp.Or then
  ...
  if op == AstOp.Mod then
  ...
  let o = BINOPS?.[op.name]
  ...
    cv = match op
      when AstOp.Add -> a.cv + b.cv
      when AstOp.Sub -> a.cv - b.cv
      when AstOp.Mul -> a.cv * b.cv
      when _ -> nil
    end
```

The table stays. The precedence and symbol are GLSL's, and they belong to the
translator, not to Petal's AST (`AstOp.symbol` is Petal's source spelling,
which here happens to match). The gain is only typo safety on the
comparisons. A reader may expect `AstOp` to carry a precedence. It doesn't,
because Petal's precedence is the parser's business, and putting it in the
enum would make it a second source of truth.

### 3. Statement chain: where enums help most

Before (`glsl.ptl:883-950`, trimmed):

```petal ignore
  let tag = node.tag
  if tag == "Let" then
    ...
  else if tag == "Set" || tag == "Assign" then
    if node.target.kind != "Name" then
  ...
  else if tag == "Break" then
    emit(c, "break;")
  else
    fail(c, tag ++ " has no GLSL form", node)
  end
```

After:

```petal ignore
  match node.tag
    when AstKind.Let -> ...
    when AstKind.Set | AstKind.Assign ->
      if node.target.kind != AstTarget.Name then
      ...
    when AstKind.Break -> emit(c, "break;")
    when other -> fail(c, other.name ++ " has no GLSL form", node)
  end
```

This could be a `match` on strings today. The enum does not force the
rewrite, but it makes the `if`/`elsif` chain the odd form out. A typo in the
chain is the most dangerous today, because the `else` reports the valid
construct as unsupported.

### 4. Where the old form is as good

Before (`glsl.ptl:1153-1159`):

```petal ignore
    if d.tag == "Literal" && (d.type == "float" || d.type == "int") then
      ty = Ty.float
    else if d.tag == "UnaryOp" && d.op == "Neg" && d.operand.tag == "Literal" then
```

After:

```petal ignore
    if d.tag == AstKind.Literal && (d.type == "float" || d.type == "int") then
      ty = Ty.float
    else if d.tag == AstKind.UnaryOp && d.op == AstOp.Neg && d.operand.tag == AstKind.Literal then
```

This is longer, and it now mixes two vocabularies: enum tags beside a string
`type`. A reader has to know which fields are enums. That is a real cost of B,
and the reason the type list in the docs has to say it.

### 5. Interaction with caches, printing and hot reload

Before (`docs/features/function-introspection.md:90-100`):

```petal
fn describe(f)
  memo(info.code_hash) text = expensive_description(fn_ast(f))
  text
end

fn expensive_description(tree)
  "{tree.tag} with {len(tree.body)} statement(s)"
end
# Lambda with 1 statement(s)
```

After: the same code prints `AstKind.Lambda with 1 statement(s)`. Writing
`{tree.tag.name}` restores the old text. Nothing else changes:

- The memo key is `code_hash`, which is unchanged.
- An AST value kept in `state` across a reload keeps working. A variant value
  is its tag text plus payload (`core/src/enum_table.rs:4-8`), and the `ast`
  package does not reload with the user's program.

### 6. Why A was rejected

A has to be judged against the language's own rules, not just glsl.ptl:

```petal ignore
# proposed A — pattern must list every payload field (language-guide.md, Pattern Matching)
match node
  when AstNode.BinaryOp(op, left, right, _, _) -> ...
  when AstNode.Ident(id, _, _) -> ...          # `name` is reserved, so `id`
  when AstNode.Call(function, args, arg_names, _, _) -> ...
  when _ -> ...
end
```

These were checked against today's language, using a scratch package
declaring `pub enum Node` (test files in the session scratchpad):

- `Ident(name, line, column)` is refused: "variant `Node.Ident` may not have a
  payload field named `name`: `.name` is every variant's own name".
- Patterns must list every field. Adding a field to a node kind (say a new
  flag on `Call`) would break every `Call` pattern.
- Reading a field the variant lacks is an error ("No field 'id' on enum
  variant Node.BinaryOp"), the same as a record, but the docs' promise that
  every field is present (`function-introspection.md:131-132`) cannot hold.

## Assessment

### Project goals

**Findings.** The README goals include introspection and goal-based program
manipulation. `fn_ast` is the in-language half of introspection, and
`docs/aspirational-ideas/README.md:25` lists programs-as-values as a
direction. The change makes the tree a little easier and safer to walk. It
adds no capability.

**Opinion.** It points the right way but does little on its own. Petal's
distinctive work here is the term graph, provenance and IR-as-target, not the
shape of this record format.

### Ease of writing

**Findings.**
- A newcomer has to learn that `tag`, `op` and `kind` are enums, but `type` is
  a string (pair 4). Enum syntax is already in the guide and widely used in the
  corpus (29 enum declarations across examples and tests).
- Error messages for a wrong variant are good ones, with did-you-mean.
- Comparing a tag with a string is silently false (see Design), which is worse
  than today unless the checker warns.

**Opinion.** For someone coming from Python's `ast` module (`isinstance(node,
ast.BinOp)`) or TypeScript's discriminated unions (`node.kind ===
SyntaxKind.BinaryExpression`), B reads as expected. The strings-plus-enum mix
is the main readability cost.

### Bug-freedom

**Findings.**
- Prevents misspelt tags and ops: about 45 sites in glsl.ptl.
- Prevents silent drift when a Rust `BinOp` variant is renamed, provided a Rust
  test checks the Petal declaration against the Rust enums (Implementation
  sketch).
- Introduces a new failure: string-vs-enum comparison is always false.
- Exhaustiveness is a hard error (checked: a `match` missing a variant is
  refused). A consumer that matches `AstKind` exhaustively without `when _`
  breaks whenever the language adds a construct. Rust handles this with
  `#[non_exhaustive]` on `syn`'s enums. Petal has no equivalent.
- No effect on the differential oracle or golden corpus: no example calls
  `fn_ast`.

**Opinion.** A net gain, but only if the string-comparison warning comes with
it.

### Performance

**Findings.**
- `fn_ast` runs once per translated function, and `glsl` memoizes the result
  under `code_hash`. Cheesecake asserts exactly two calls for a lambda and its
  helper (`script_host_tests.cpp:4498`).
- The conversion builds the same number of values. Each tag becomes an
  interned `EnumTag` (4 bytes) instead of a heap string.
- Matching on a declared variant is an integer slot comparison
  (`enum_table.rs:16-21`) rather than a string compare chain.
- It is not on any per-frame path. Not measured: no benchmark covers
  translation, and the call count makes it irrelevant.

### Live coding and hot reload

**Findings.** `code_hash` is computed before conversion, so it is unchanged,
and so are the reload outcomes in `fn_introspection.rs` (relocated / patched).
Variant values survive reloads by tag text. If the enums live in a bundled
package, they are not user code and do not reload.

### Introspection, debugging and program manipulation

**Findings.** None: the term graph, provenance, `explain` and direct
manipulation do not read `fn_ast` output. The change is confined to the
value-conversion boundary.

### Maintainability

**Findings.**
- Rust: replace `json_to_value(&meta.ast)` (`vm/introspect.rs:177`) with a
  walker that converts the `tag`, `op` and `kind` keys into variants. About
  80-150 lines.
- Petal: about 40 lines of declarations.
- A Rust test that every `StmtKind`, `ExprKind`, `BinOp`, `UnaryOp` and
  `AssignTarget` has a variant. It adds an invariant (the Petal declaration
  must track the parser), but makes it checked instead of implicit.
- Docs: the two tables in `function-introspection.md`, its example output, and
  `petal-to-glsl.md` if it quotes tags.
- The C bridge (`Env::function_params`) reads `FnMeta.params`, not the tree,
  so it is unaffected.
- `docs/tasks/wasm-size-improvements.md:60` plans to leave `fn_introspect` out
  of wasm. The new package should be gated by the same feature.

### Domains

- **Graphics** (`to_glsl`): helps, as the motivating user.
- **Editor scripting** (Garden): neutral today. It would help a future in-editor
  outline or refactoring script written in Petal.
- **Games, UI apps, dashboards, web, embedding hosts**: neutral. None reads
  `fn_ast`.

### Compatibility and migration

**Findings.**
- Breaks glsl.ptl (about 45 sites, manual but mechanical) and the docs'
  example output. Nothing else in the corpus.
- `fn_ast` is labelled experimental (`function-introspection.md:7-11`), so a
  breaking change is allowed.
- There is no warning-first path, because the value changes type. Shipping
  glsl.ptl's update in the same commit is the migration.

**Placement.** When this was written, enum names were global across
modules: a file importing a package that declares `enum Node` could not
declare its own `enum Node` ("enum `Node` is already declared in
`astk.ptl`…", checked with a scratch package). Since 2026-11-09 an enum
belongs to the module that declares it
([module-name-collisions.md](module-name-collisions.md), part C): a file's
own `enum Node` wins the bare name over an import's, two modules' `Node`s are
different enums, and a host reads the package's variants as
`"<package>::Node.call"`. The collision below no longer breaks those four
files. Two plain imports that both export a name are still ambiguous used
bare, and a package's enums still carry the package in their tags, so the
`Ast` prefix remains the friendlier name for an enum every file can see.
`core` is in scope in every file (`core/packages/core/petal.toml`).

The corpus has no `AstKind`, `AstOp` or `AstTarget`. It does have `enum Op`
twice (`examples/console/enums_and_match.ptl:26`,
`test/benchmarks/micro/enum_reads.ptl:22`) and `enum Expr` twice
(`examples/console/pattern_matching.ptl:42`, `test/match-enum-eval/main.ptl:2`).
The unprefixed names `Op`/`Node`/`Expr` would break those four files. That is
why the names are `Ast`-prefixed and why an opt-in `ast` package is
preferred.

### Prior art

- **Python `ast`**: a class per node, with keyword class patterns
  (`case BinOp(op=Add())`). Named sub-patterns are what make the
  full-variant style pleasant there, and Petal does not have them.
- **TypeScript compiler API**: records with a `kind: SyntaxKind` numeric enum.
  This is B almost exactly, and it has scaled to hundreds of node kinds.
- **Rust `syn`**: full enums, with `#[non_exhaustive]` added because adding
  variants broke downstream matches.
- **Elixir quoted forms**: `{:+, meta, [a, b]}`, atoms as tags, with
  position in a `meta` slot so patterns can ignore it in one place.

### Alternatives considered

- **Do nothing.** glsl works and is tested. The risk is typos in about 45
  sites of one file. This is a reasonable choice while there is one consumer.
- **Constants instead of enums.** glsl.ptl could declare
  `let LITERAL = "Literal"` locally, and a misspelt constant name is a compile
  error. This costs zero language change and gives most of the typo safety for
  glsl alone. Every future consumer would redo it, and nothing ties the
  constants to the parser.
- **A lint.** Check string literals compared with `.tag` of an `fn_ast`
  result. This needs dataflow from the native's result, which is more work
  than B for less.
- **A (full payload enum).** Rejected above.
- **Smallest useful version.** Only `tag` becomes an enum, and `op`/`kind`
  stay strings. This saves about a third of the work but leaves the op
  comparisons, which are where the Rust Debug-format coupling lives.

## Migration and compatibility

- glsl.ptl: about 45 sites, done in the same commit as the change. The 306
  differential cases used to check f27a7bea can be reused to check that the
  output is unchanged.
- Docs: `function-introspection.md` (tables, example output),
  `petal-to-glsl.md` if it quotes tags, and CHANGELOG under **Breaking**.
- No host or other project changes.

## Implementation sketch

1. Prototype the placement question first (see Open questions): one enum in a
   bundled `ast` package, one native returning a variant of it, and a program
   that does and does not import `ast`.
2. Add `core/packages/ast` (or the chosen home) with the three enums, behind
   the `fn_introspect` feature.
3. In `vm/introspect.rs`, replace `json_to_value` for `fn_ast` with a walker
   that maps `tag`, `op` and `kind` to `EnumVariant`s. Keep the JSON for
   hashing.
4. Add a Rust test that every Rust AST variant has a Petal counterpart, and
   the reverse.
5. Type `fn_ast`'s result well enough for the checker to warn on
   string-vs-enum comparison of `tag`, or file that as a follow-up.
6. Update glsl.ptl, the docs and the CHANGELOG. Run `fn_introspection.rs`
   and the glsl differential.

Roughly 1-2 days.

## Open questions

- **Where the enums live.** In `core` (global, reserves three names) or in an
  opt-in `ast` package. If they live in `ast`, does a program that never
  imports it still get usable values from `fn_ast`? Fieldless variants need no
  declaration for `==` and printing, but `.name` on an undeclared tag and
  `AstOp.symbol` constant data need checking. Needs the prototype in step 1.
  The language owner decides.
- **Non-exhaustive enums.** Should Petal have a way to mark an enum as
  "more variants may come", so library enums like this one can grow without
  breaking exhaustive matches? This is a separate language question, and it
  becomes pressing once any package exports an enum.
- **The string-vs-enum warning.** Should it come with B or follow it?
  Without it, B replaces "typo never matches" with "unconverted comparison
  never matches".
- **Literal `type`.** It stays a string here. Revisit if a `Ty`-like enum ever
  becomes a core concept.

## Related papercuts found while refactoring glsl.ptl

These came up in f27a7bea and in the checks for this proposal. They are not
part of this design. Each would need its own proposal or bug.

1. **`get` cannot be a class field name.** `class Cell get: function, ... end`
   fails with "Expected a field name or `end` to close class `Cell`, got
   `get`". A record key `get:` and the call `c.get()` both work. glsl.ptl had
   to rename its context closures to `read`/`write`.
2. **Enums cannot have methods.** `fn Ty.text(t: Ty)` is refused: "methods are
   declared on classes only". glsl.ptl uses the free function `ty_text(t)`.
   Constant data covers fixed per-variant values, but not computed ones that
   depend on the payload (`Ty.named(id)` → `id`). (Since 2026-11-09 enums
   take methods: [enum-methods.md](enum-methods.md).)
3. **No nullable types.** There is no `record?` or `Ty?`. A parameter that may
   be `nil` (glsl's `node` in `fail`, `param`, `helper`) has to stay
   unannotated, because `record` warns on a literal `nil` argument. Class
   fields that may be nil (`Val.lit`, `FnState.ret_ty`, `Ctx.hook`) are
   unannotated for the same reason.
4. **A payload field may not be named `name`.** It is reserved for the
   variant's own name. This is reasonable, but it is a trap for data-shaped
   enums like an AST.
5. **Enum names are global across modules,** even for a qualified import. A
   package that exports an enum reserves its name in every importer. Class
   names do not behave this way: a user `class Rect` shadows the built-in one.
