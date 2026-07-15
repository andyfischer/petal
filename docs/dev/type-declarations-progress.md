# Optional Static Type Declarations — Progress & Handoff

Living status tracker for implementing optional static type declarations.
**Design rationale lives in [`type-declarations-plan.md`](type-declarations-plan.md)** — read it first.
This doc tracks *what is done, what remains, and how to continue*.

Last updated: 2026-07-14 (Chunks A–G complete — feature shipped) · Branch: `main`

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
| E1 — preserve raw type names | ✅ done | `a85ea3d` | `Option<Type>` → `Option<TypeAnn { name, resolved }>`; unknown names kept |
| D — prescan signature table | ✅ done | `c90adf7` | `collect_fn_signatures` → `Compiler.fn_signatures` keyed by `(name, arity)` |
| E2 — the checker | ✅ done | `12f1a45` | `rust/src/typecheck/`: scoped env, shallow infer, 5 check sites, `Diagnostic` |
| E3 — surface (run/check) | ✅ done | `a9bf3e3` | `Program.warnings`; `check`/`run` stderr carets + `check --json warnings[]` |
| F — surface + MCP + strict | ✅ done | `fada42e`,`f638449` | run/check text+JSON+carets; `check --strict`; MCP CheckSnippet/TestSnippet warnings |
| G — docs & examples | ✅ done | `4b3d9d7` | Language_Guide Types section, CLI/Builtins/goals reconcile, `examples/typed.ptl`, README |

Legend: ✅ done · 🚧 in progress · ⬜ todo

> **Ordering note:** the unknown-type-name decision (Chunk E spec) was resolved by
> *preserving the raw name* — landed as its own commit (E1) ahead of D, since it
> changed the annotation representation D and the checker both read.

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

### Chunks D + E — DONE (this pass)
The prescan signature table, the checker, and basic diagnostic surfacing all
landed. What now exists on top of the Chunk-C foundation:
- **`rust/src/ast.rs`** — `TypeAnn { name: String, resolved: Option<Type> }`
  replaces the bare `Option<Type>` on `Let.ty` / `Param.ty` / `FnDecl.ret`. An
  unrecognized name (`let x: banana`) is preserved with `resolved: None` instead
  of being dropped. `show-ast --json` emits `ty`/`ret` as `{ name, resolved }`.
- **`rust/src/compiler/mod.rs`** — `pub(crate) fn collect_fn_signatures(&[Stmt])
  -> HashMap<(String,usize), FnSignature>`; result folded into the new
  `Compiler.fn_signatures` side table during `prescan_declarations`. IR
  untouched. `FnSignature` lives in `types.rs`.
- **`rust/src/typecheck/mod.rs`** — `check_module(stmts, &fn_signatures) ->
  Vec<Diagnostic>`, invoked from `compile_module` after prescan. Scoped
  `Vec<HashMap<String, VarType>>` env; folded `check_expr` doing conservative
  shallow inference (any ambiguity ⇒ `Any`, which suppresses); five check sites
  (unknown type name, typed `let`, reassignment, call args, fn return tail).
  Never errors. 18 unit tests; the entire un-annotated corpus stays silent.
- **`rust/src/diagnostic.rs`** — `Diagnostic { span: SourceSpan, message }`.
- **`rust/src/program.rs`** — `#[serde(skip)] Program.warnings: Vec<Diagnostic>`
  (compile-time artifact, not in portable IR).
- **`rust/src/cli/handlers.rs`** — `petal check` prints carets to stderr / a
  `warnings[]` array under `--json` (exit stays 0); `petal run` prints warnings
  to stderr before executing (stdout + runtime untouched).
- **`rust/src/source_map.rs`** — `SourceSpan`/`SourcePosition` gained
  `PartialEq, Eq` (needed by `Diagnostic`).

**Design notes for the next implementer:**
- Inference is deliberately conservative — prefer a false negative to a false
  positive. `Div` on two ints is `Int` (integer division); `+` on strings is a
  *runtime* error so it infers `Any`; string concat is the separate `Concat`
  (`++`) op. `Concat`/`Coalesce`/field/index and any non-obvious case ⇒ `Any`.
- Call-site checks read the global `fn_signatures` table (handles forward refs +
  arity overloads); local bindings follow lexical scope order.
- Match-arm pattern vars, `for`/`while` loop vars, `state` names, and lambda
  params are all bound as `Any` so they never mis-trigger against an outer typed
  binding of the same name.

### Chunk F — surfacing + MCP + strict — DONE
- `run`/`check` text carets + `--json warnings[]` via `format_source_snippet`
  (E3). `warnings_json` / `render_warnings_text` helpers in `cli/handlers.rs`.
- `check --strict` exits 1 when warnings exist (plan §12 Q2); plain `check`/`run`
  stay 0. Parsed in `cli/args.rs::parse_check_args`.
- MCP: `CheckSnippet` already forwards `check --json` (so it carries
  `warnings[]`); `TestSnippet` shows them via `run`'s stderr. Tool descriptions +
  `docs/dev/mcp-server.md` updated to say so.

### Chunk G — docs & examples — DONE
- `Language_Guide.md` gained a **Type Annotations** section (syntax, warning-only,
  promotion, explicit casts). `CLI.md` `check` documents warnings + `--strict`.
  `Builtins.md` cross-links the casts. `goals.md` "Types as a projection" rows
  reconciled (🟡, user-writable + warning-only). `README.md` types line flipped
  to a shipped feature. `examples/typed.ptl` (+ `test/example-golden/typed.json`)
  runs clean and is in the manifest.

### Possible future work (not scheduled)
- Give `TypeAnn` its own span so the unknown-type caret underlines just the type
  name, not the whole statement.
- Parameterized types (`list<int>`, arrow types, structural records), user type
  aliases, deeper inference — all explicitly deferred by the plan.
- A per-file `// @strict` pragma to opt individual files into error-level
  enforcement (plan §12 Q3).

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
- **Unknown type names** are now preserved as `TypeAnn { name, resolved: None }`
  (Chunk E1) and warned on by the checker — no longer dropped.
- **Serde:** AST types derive `Serialize` only (not `Deserialize`); `Type` must
  keep `Serialize` for `show-ast --json`.
- The `check` CLI command now runs the type checker and prints warnings
  (stderr text / `--json warnings[]`), still exiting 0. `--strict` (non-zero on
  warnings) is the remaining open decision — see Chunk F.
