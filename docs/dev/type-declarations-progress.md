# Optional Static Type Declarations — Progress & Handoff

Living status tracker for implementing optional static type declarations.
**Design rationale lives in [`type-declarations-plan.md`](type-declarations-plan.md)** — read it first.
This doc tracks *what is done, what remains, and how to continue*.

Last updated: 2026-07-14 · Branch: `main`

---

## Locked decisions (do not re-litigate)
- **Optional** annotations; absence ⇒ inferred type or `any`.
- **Enforcement = warnings only.** The checker never blocks compilation. Needs a
  new non-fatal diagnostic channel (see Chunk E/F).
- **Inference = shallow / local** (literals + called fn signatures; else `any`).
- **Runtime checks = none** (static-only) this phase.
- **No implicit casting** — explicit `int()` / `float()` / `str()` only.
- **Syntax:** lowercase, contextual type names; `:` on bindings/params, `->` on
  named-fn return. `str` is an accepted alias for `string`.

---

## Status board

| Chunk | Status | Commit | Summary |
|-------|--------|--------|---------|
| A — `Type` core | ✅ done | `e817f17` | `rust/src/types.rs`: `Type`, `from_name`, `name`, `is_assignable_to` |
| B — let & param annotations | ✅ done | `28c7724` | `let x: int`, `fn f(a: int)`, lambda params → `Param`/`Let.ty` |
| C — fn return types | ✅ done | `d604d21` | `fn f(...) -> t` → `FnDecl.ret` |
| D — prescan signature table | ⬜ todo | — | collect declared param/return types for call-site checking |
| E — the checker | ⬜ todo | — | shallow inference + assignability + non-fatal diagnostics |
| F — surface diagnostics | ⬜ todo | — | warnings via `run`/`check` + MCP, caret rendering |
| G — docs & examples | ⬜ todo | — | Language_Guide, goals.md reconcile, `examples/typed.ptl`, README |

Legend: ✅ done · 🚧 in progress · ⬜ todo

---

## What exists after Chunk C (the parsing foundation)

The parser accepts and the AST/tooling expose annotations; **nothing checks or
uses them yet** (compiler strips them to names; runtime unaffected).

### Type representation — `rust/src/types.rs`
- `pub enum Type { Any, Nil, Bool, Int, Float, String, List, Record, Function,
  Enum, Vec2, F64Array, Element, Symbol, Dual, Handle, Pending }`
  (derives `Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize`).
- `Type::from_name(&str) -> Option<Type>` — lowercase vocab + `str` alias;
  unknown ⇒ `None`.
- `Type::name(&self) -> &'static str` — canonical spelling, == `Value::type_name`
  for concretes, `"any"` for `Any`.
- `Type::is_assignable_to(&self, &Type) -> bool` — `Any` both ways; `Int`→`Float`
  yes, `Float`→`Int` no; else equality.

### AST — `rust/src/ast.rs`
- `pub struct Param { pub name: String, pub ty: Option<Type> }`
- `StmtKind::Let { name, ty: Option<Type>, value }`
- `StmtKind::FnDecl { name, params: Vec<Param>, ret: Option<Type>, body }`
- `ExprKind::Lambda { params: Vec<Param>, body }` (no return type)
- `EnumVariant.fields` stays `Vec<String>` (field types deferred).

### Parser — `rust/src/parse.rs`
- `parse_type_annotation() -> Result<Option<Type>, String>` — optional `: type`,
  wrapped in a `TypeAnnotation` CST node.
- `parse_return_type() -> Result<Option<Type>, String>` — optional `-> type` on
  named fns, wrapped in a `ReturnType` CST node.
- `parse_param_list()` now returns `Vec<Param>` (shared by fn/lambda/enum;
  `parse_enum_decl` maps `Param`→name).

### CST — `rust/src/cst/mod.rs`, `rust/src/cst_project.rs`
- New `SyntaxKind::TypeAnnotation` and `SyntaxKind::ReturnType`.
- Projection helpers: `type_from_annotation_node(&SyntaxNode) -> Option<Type>`
  (reads the first ident, skips `:`/`->`), `projected_params(&SyntaxNode) ->
  Vec<Param>`. `param_list` returns `Vec<Param>`; `param_names` (names only)
  retained for enums.
- `LetStmt` projection excludes the `TypeAnnotation` node when finding the value;
  `FnDecl` projection reads the optional `ReturnType` child.

### Compiler — annotations dropped to names (for now)
- `compiler/stmt.rs`: `Let { .., value, .. }`; `FnDecl { name, params, body, .. }`
  maps params to `Vec<String>` before `compile_fn_decl`.
- `compiler/expr.rs`: `Lambda` maps params to names before `compile_function`.
- `compile_fn_decl` / `compile_function` still take `&[String]` — **unchanged**.

### Serialization — `show-ast --json`
`ty`/`ret` serialize as the Rust variant name (`"Int"`, `"Float"`, `"String"`,
…) or `null`. Params are `[{name, ty}]`. Schema documented in
[`../CLI.md`](../CLI.md) (`Param`, `Type`, FnDecl `ret`).

---

## Remaining work (specs for the next implementer)

### Chunk D — prescan signature table
Collect declared `(param types, return type)` per function so the checker can
verify call sites even with forward references and arity overloads.
- Extend `prescan_declarations` (`compiler/mod.rs:~657`) — it already walks
  `FnDecl` and knows arities. Store into a **compile-time side table** keyed by
  `(name, arity)`; do **not** add type fields to the IR `Term`/`FunctionDef`.
- Respect arity-only overloading (`Function_Overloading.md`): key by arity.
- Ships with unit tests only (no user-facing behavior yet). Optional to merge
  into Chunk E if a standalone commit feels too thin.

### Chunk E — the checker (the crux)
New module `rust/src/typecheck/` (model on `lint/` and `desugar.rs`). Runs on the
projected + desugared AST, invoked from `compile_module` right after
`desugar::desugar` (`compiler/mod.rs:~278`).
- **Scoped type env** mirroring the compiler's `scopes` stack: `name ->
  (declared: Option<Type>, inferred: Type)`.
- **infer(expr) -> Type** (shallow): literals → obvious type; ident → recorded
  type; cast calls `int()/float()/str()` → `Int/Float/String`; `BinaryOp` →
  promotion rules (int op int→int, any float→float, comparisons→bool, string
  `+`→string); known call → declared return; `if`/`match`/`for`/`while` value
  position → branch join (equal ⇒ that type, else `Any`); unknown ⇒ `Any`.
- **check** sites (warn when a *concrete* inferred type is not
  `is_assignable_to` the declared type; `Any` on either side suppresses):
  `let x: T = e`; call args vs declared params; fn last-expr vs declared `ret`;
  reassignment of an annotated binding.
- **Warning-only:** never returns `Err`; accumulates `Vec<Diagnostic>`.
- Tests: golden `.ptl` → expected `warnings[]` (mismatch warns; `Any`
  suppresses; explicit cast clears; `int`→`float` promotion doesn't warn).
- **Also revisit the unknown-type-name TODO** (Chunk B): to warn on
  `let x: banana = …`, the annotation must preserve the raw name. Options: add a
  richer AST field (`Option<TypeAnn { name, resolved }>`) or validate at parse
  time. Decide here.

### Chunk F — surface diagnostics
- Add a **non-fatal warning channel** (a `Vec<Diagnostic>` produced alongside the
  compiled `Program`). Today only hard `Result<_, String>` errors and deferred
  `TermOp::Error` runtime errors exist.
- `Diagnostic` carries a `SourceSpan` (`source_map.rs`, file-tagged). Render with
  `format_source_snippet` (`backend/errors.rs:~131`, already `pub`) +
  `format_position` (`errors.rs:~97`).
- Surface in `petal run` and `petal check` (stderr text; `--json` ⇒ a
  `warnings[]` array). `check` (`cli/args.rs`) becomes the check-only entry.
- Wire into MCP `CheckSnippet` / `TestSnippet`.
- Open Q (plan §12): `check --strict` exits non-zero on warnings for CI?

### Chunk G — docs & examples
- `Language_Guide.md` Types section: annotation grammar.
- `Builtins.md`: cross-link casts as the sanctioned conversions.
- `goals.md`: reconcile "Types as a projection" (now user-writable, still
  warning-only — consistent with "never enforced").
- `examples/typed.ptl`; flip `README.md:13` from aspiration to reality.

---

## Verification recipe (run after every chunk)

```bash
# Rust: unit + CST/AST differential over the repo corpus
cd rust && cargo test --lib            # expect: all pass (353+ as of Chunk C)
cargo test --lib cst_project::         # the differential + annotation projection

# TS integration (builds the binary via global-setup)
cd ts && npx vitest run test/type-annotations.test.ts
npx vitest run                         # full suite; expect no regressions (518+)

# End-to-end spot checks
B=rust/target/debug/petal
$B run -e 'let x: int = 5
fn area(r: float) -> float
  3.14159 * r * r
end
print(x, area(2.0))'
$B show-ast --json -e 'fn f(a: int, b) -> bool
  true
end'
```

---

## Gotchas / invariants
- **Four-place coordinated change** for any new syntax: `parse.rs` (consume +
  emit CST events) + `cst/mod.rs` (`SyntaxKind`) + `cst_project.rs` (projection)
  + `ast.rs` (fields). Guarded by `debug_assert_eq!` in `cst/driver.rs` comparing
  the parser's direct AST to the CST projection on **every parse in debug
  builds** — a divergence panics tests. Add annotated cases to
  `cst_project.rs`'s `assert_projects` tests so the differential covers them.
- **Type names are contextual, not reserved** — `int`/`float`/`str` remain
  callable builtins; only recognized in type position (after `:` / `->`).
- **`parse_param_list` is shared** by fn/lambda/enum. Changing it ripples;
  enum keeps names only.
- **Unknown type names** currently resolve to `None` (dropped). Preserve the raw
  name before the checker can warn on them (see Chunk E).
- **Serde:** AST types derive `Serialize` only (not `Deserialize`); `Type` must
  keep `Serialize` for `show-ast --json`.
- The `check` CLI command exists and today does **no** type checking — it's the
  natural home for the checker entry point.
