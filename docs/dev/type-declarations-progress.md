# Optional Static Type Declarations — Progress & Handoff

Living status tracker for implementing optional static type declarations.
**Design rationale lives in [`type-declarations-plan.md`](type-declarations-plan.md)** — read it first.
This doc tracks *what is done, what remains, and how to continue*.

Last updated: 2026-07-14 (Chunks A–G complete — feature shipped) · Branch: `main`

---

## Locked decisions (do not re-litigate)
- **Optional** annotations; absence ⇒ inferred type or `any`.
- **Enforcement = warnings only.** The checker never blocks compilation.
  Delivered via the non-fatal `Diagnostic` channel (`rust/src/diagnostic.rs` +
  `Program.warnings`).
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

## What exists now (A–G shipped)

Annotations parse, type-check (warning-only), and surface through the CLI/MCP.
The runtime is untouched — annotations are stripped to names for codegen.

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
- `pub struct FnSignature { params: Vec<Option<Type>>, ret: Option<Type> }` —
  a function's declared signature (resolved types only). Compile-time; not in IR.

### AST — `rust/src/ast.rs`
- `pub struct TypeAnn { name: String, resolved: Option<Type> }` — a written
  annotation: the raw name plus its resolution (`resolved: None` = unrecognized
  name, preserved for diagnostics, not dropped). `TypeAnn::new(name)` builds it.
- `pub struct Param { pub name: String, pub ty: Option<TypeAnn> }`
- `StmtKind::Let { name, ty: Option<TypeAnn>, value }`
- `StmtKind::FnDecl { name, params: Vec<Param>, ret: Option<TypeAnn>, body }`
- `ExprKind::Lambda { params: Vec<Param>, body }` (no return type)
- `EnumVariant.fields` stays `Vec<String>` (field types deferred).

### Parser / CST — `rust/src/parse.rs`, `cst/mod.rs`, `cst_project.rs`
- `parse_type_annotation()` / `parse_return_type()` return
  `Result<Option<TypeAnn>, String>`, wrapping the `:`/`->` + name in a
  `SyntaxKind::TypeAnnotation` / `ReturnType` CST node.
- `type_from_annotation_node(&SyntaxNode) -> Option<TypeAnn>`;
  `projected_params(&SyntaxNode) -> Vec<Param>`. `param_names` (names only)
  retained for enums. Both parse paths build `TypeAnn` via `TypeAnn::new` so the
  `debug_assert_eq!` differential stays green.

### Checker — `rust/src/typecheck/mod.rs`, `diagnostic.rs`
- `check_module(stmts, &fn_signatures) -> Vec<Diagnostic>`, invoked from
  `compile_module` after `prescan_declarations`. Scoped
  `Vec<HashMap<String, VarType>>` env; folded `check_expr` doing conservative
  shallow inference (any ambiguity ⇒ `Any`, which suppresses); five check sites
  (unknown type name, typed `let`, reassignment, call args, fn return tail).
  Never errors. 18 unit tests; the entire un-annotated corpus stays silent.
- `pub(crate) collect_fn_signatures(&[Stmt]) -> HashMap<(String,usize),
  FnSignature>` (`compiler/mod.rs`) → `Compiler.fn_signatures` side table.
- `Diagnostic { span: SourceSpan, message }`; carried on
  `#[serde(skip)] Program.warnings` (compile-time artifact, not in IR).

### Compiler codegen — annotations dropped to names (unchanged)
- `compiler/stmt.rs` / `expr.rs` still map params to `Vec<String>` and drop
  `ty`/`ret` before `compile_fn_decl` / `compile_function` (which take
  `&[String]`). Type info lives only in the checker + side table.

### Surfacing — `cli/handlers.rs`, `cli/args.rs`, MCP
- `petal check` prints carets to stderr / a `warnings[]` array under `--json`
  (exit 0); `--strict` exits 1 when warnings exist. `petal run` prints warnings
  to stderr before executing (stdout + runtime untouched). Helpers:
  `warnings_json`, `render_warnings_text`.
- MCP `CheckSnippet` forwards `check --json` (carries `warnings[]`);
  `TestSnippet` shows them via `run` stderr.

### Serialization — `show-ast --json`
`ty`/`ret` serialize as `{ "name": "int", "resolved": "Int" }`, or
`{ "name": "banana", "resolved": null }` for an unknown name, or `null` when
un-annotated. Schema documented in [`../CLI.md`](../CLI.md) (`TypeAnn`, `Type`).

---

## What landed & implementation notes

- **`SourceSpan`/`SourcePosition`** gained `PartialEq, Eq` (needed by
  `Diagnostic`).
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

---

## Follow-up ideas (not scheduled)

- **Tighter unknown-type carets.** Give `TypeAnn` its own `SourceSpan` so the
  unknown-type warning underlines just the type name, not the whole statement.
  (Today the checker uses the enclosing stmt/expr span since `TypeAnn` carries
  no span — the four-place differential makes threading a span through both
  parse paths the fiddly part.)
- **Structured warnings in `run --json`.** `run` prints warnings as stderr text
  only; a `warnings[]` channel on `run --json` (reusing `warnings_json`) would
  let `TestSnippet` return them as data, not just text.
- **`return`-statement return checks.** The checker only compares a function's
  *tail expression* to its declared `ret`; explicit `return e` mid-body isn't
  checked yet.
- **Parameterized / richer types** — `list<int>`, arrow types, structural
  records, user type aliases, deeper (non-local) inference. All explicitly
  deferred by the plan.
- **Per-file `// @strict` pragma** to opt individual files into error-level
  enforcement (plan §12 Q3).
- **Enum variant field annotations** (`Circle(radius: float)`) — the shared
  param parser makes this a cheap future add (plan §12 Q4).

---

## Verification recipe (run after every chunk)

```bash
# Rust: unit + CST/AST differential over the repo corpus
cd rust && cargo test --lib            # expect: all pass (375 as of Chunk G)
cargo test --lib typecheck::           # checker unit tests
cargo test --lib prescan_tests         # signature side-table tests

# TS integration (builds the binary via global-setup)
cd ts && npx vitest run test/type-annotations.test.ts test/type-warnings.test.ts
npx vitest run                         # full suite; expect no regressions (526)
cd .. && npm run test-examples         # example goldens incl. typed.ptl (26)

# End-to-end spot checks
B=rust/target/debug/petal
$B run examples/typed.ptl                            # runs clean, no warnings
$B check --json -e 'let x: int = "hi"'               # {"ok":true,"warnings":[…]}
$B check --strict -e 'let x: int = "hi"'; echo $?    # exit 1
$B show-ast --json -e 'let x: banana = 5'            # ty: {name:"banana",resolved:null}
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
- The `check` CLI command runs the type checker and prints warnings (stderr
  text / `--json warnings[]`), exiting 0; `check --strict` exits 1 when warnings
  exist. `run` prints warnings to stderr and always exits on runtime status.
