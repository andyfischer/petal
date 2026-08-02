# Optional Static Type Declarations — Progress & Handoff

Living status tracker for implementing optional static type declarations.
**Design rationale lives in [`type-declarations-plan.md`](type-declarations-plan.md)** — read it first.
This doc tracks *what is done, what remains, and how to continue*.

Last updated: 2026-08-02 (chunk K: class names are type names) · Branch: `main`

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
| H — `state` annotations | ✅ done | (audit) | `state x: t = …` in all three spellings; checked like a `var` cell |
| I — editor support | ✅ done | (audit) | tree-sitter models `type_annotation`/`return_type`/`parameter`; vim `petalType` |
| J — parameterized-type error | ✅ done | (audit) | `list<int>` gets one targeted message instead of three positional ones |
| K — class names as types | ✅ done | (classes) | `Type::Class(ClassId)` + `Type::resolve`; `fn f(r: Rect)` checks, field reads are typed |

Legend: ✅ done · 🚧 in progress · ⬜ todo

### Audit pass (2026-08-02)

The feature was re-verified end-to-end against the real binary. Everything the
board claimed for A–G held: `fn add(a: int, b: int) -> int`, `let x: int = 1`,
`var x: float = 1.0` + `set`, lambda *parameter* annotations, unknown names
preserved-and-warned, and warnings surfaced with correct carets through
`petal check`, `petal run`, `check --json`, `check --strict`, and the MCP
`CheckSnippet`/`TestSnippet` tools. Three gaps were found and closed (H/I/J
above). Two apparent gaps were confirmed as *intended*, not bugs:

- **No lambda return type.** `fn(n: int) -> int -> n * 2` is rejected. A
  lambda's `->` introduces its body, so this needs two arrows (plan §2). Pinned
  by `rust/tests/type_annotations.rs::lambdas_take_param_annotations_but_no_return_type`.
- **No parameterized types.** A type is a single bare name; `list` and `record`
  are opaque. Locked non-goal (plan §1) — what changed is only the *error*.

> **Ordering note:** the unknown-type-name decision (Chunk E spec) was resolved by
> *preserving the raw name* — landed as its own commit (E1) ahead of D, since it
> changed the annotation representation D and the checker both read.

---

## What exists now (A–G shipped)

Annotations parse, type-check (warning-only), and surface through the CLI/MCP.
The runtime is untouched — annotations are stripped to names for codegen.

### Type representation — `rust/src/types.rs`
- `pub enum Type { Any, Nil, Bool, Int, Float, String, List, Record, Function,
  Enum, Vec2, F64Array, Element, Symbol, Dual, Handle, Pending, Class(ClassId) }`
  (derives `Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize`).
- `Type::from_name(&str) -> Option<Type>` — lowercase vocab + `str` alias;
  unknown ⇒ `None`. A **class name is `None` here**: resolving one needs the
  compilation's `ClassTable`, so use `Type::resolve(name, classes)` (chunk K),
  which is what the checker and `collect_fn_signatures` call.
- `Type::name(&self) -> &'static str` — canonical spelling, == `Value::type_name`
  for concretes, `"any"` for `Any`, `"class"` for `Class` (which has no static
  spelling). Diagnostics use `Type::display(&ClassTable) -> Cow<str>`, which
  prints the class's real name.
- `Type::is_assignable_to(&self, &Type) -> bool` — `Any` both ways; `Int`→`Float`
  yes, `Float`→`Int` no; `Class(_)`→`Record` yes (an instance *is* a record) but
  not the reverse; else equality.
- `pub struct FnSignature { params: Vec<Option<Type>>, ret: Option<Type> }` —
  a function's declared signature (resolved types only). Compile-time; not in IR.

### AST — `rust/src/ast.rs`
- `pub struct TypeAnn { name: String, resolved: Option<Type> }` — a written
  annotation: the raw name plus its resolution (`resolved: None` = unrecognized
  name, preserved for diagnostics, not dropped). `TypeAnn::new(name)` builds it.
- `pub struct Param { pub name: String, pub ty: Option<TypeAnn> }`
- `StmtKind::Let { name, ty: Option<TypeAnn>, value, is_var }`
- `StmtKind::State { name, ty: Option<TypeAnn>, init, id, key, is_var }`
- `StmtKind::FnDecl { name, params: Vec<Param>, ret: Option<TypeAnn>, body }`
- `ExprKind::Lambda { params: Vec<Param>, body }` (no return type — plan §2)
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
- `language-guide.md` gained a **Type Annotations** section (syntax, warning-only,
  promotion, explicit casts). `CLI.md` `check` documents warnings + `--strict`.
  `Builtins.md` cross-links the casts. `goals.md` "Types as a projection" rows
  reconciled (🟡, user-writable + warning-only). `README.md` types line flipped
  to a shipped feature. `examples/typed.ptl` (+ `test/example-golden/typed.json`)
  runs clean and is in the manifest.

### `var` / `set` cells — DONE
See [`var-next-steps.md`](var-next-steps.md) (Cells).
A `var` binds a heap cell, so it breaks the checker's usual assumption that a
binding's initializer describes every later read.

- **Writes are checked.** `set` shares the `Assign` arm, so a `set` whose value
  conflicts with the var's *declared* type raises the same
  ``type mismatch: `n` declared `int` but assigned `string` `` diagnostic a
  conflicting `=` does — including from inside a function or a closure, under
  control flow, which is the whole point of cells.
- **Un-annotated reads infer `Any`.** `var n = 0` no longer types `n` as `int`,
  because a `set` can retype the cell from anywhere and this pass cannot see it.
  Trusting the initializer warned on *correct* programs (`var n = 0` / `set n =
  "hi"` / `let s: string = n`), at all three read sites — binding, call argument,
  fn return — which is the one outcome the pass is built to avoid. An annotated
  `var` still types its reads, and earns that by constraining every `set`.
- **`state` now has an annotation slot** (audit chunk H), and it behaves exactly
  like a `var` cell, for the same reason: a reactive binding's initializer
  describes at most its *first* read — the next frame re-runs against a persisted
  value, and a `set` on a `state var` can replace it from anywhere. So an
  un-annotated `state` still binds `Any` in both directions, and an annotated one
  types every read *and* checks every write, including the initializer, an `=`
  reassignment, and a `set` from inside a closure under control flow. The key
  expression of `state(k) n: t = …` is still walked for nested diagnostics.
- **Field/index `set` targets are unchecked** — `set r.a = …` / `set xs[0] = …`
  walk their subexpressions (so nested mismatches still report) but the written
  value is not checked, because `record`/`list` are opaque: there is no field or
  element type to conflict with. Blocked on parameterized types, a locked
  non-goal. Not `var`-specific — `r.a = …` on a `let` is equally unchecked.

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
- **Parameterized / richer types** — `list<int>`, arrow types, structural
  records, user type aliases, deeper (non-local) inference. All explicitly
  deferred by the plan; writing one now gets a targeted parse error naming the
  bare type instead of a misleading downstream one (audit chunk J).
- **Per-file `// @strict` pragma** to opt individual files into error-level
  enforcement (plan §12 Q3).
- **Method return types in inference.** A `r.center_x()` call infers `any`, even
  when the receiver's class is known and the method has a declared return type.
  Deliberately conservative for now (a callable field of the same name would
  make the inferred type wrong); the class table would need the method's
  signature, not just its arity, to do better.
- **Compile-time unknown-method warnings.** Calling a method a class does not
  have is a *runtime* error (`No method 'nope' on class Rect`). A check-time
  warning was considered and dropped: dispatch also reaches every global native,
  including ones an embedder registers that the checker cannot see, so the
  warning would fire on working code — the one outcome this pass avoids.
- **Enum variant field annotations** (`Circle(radius: float)`) — the shared
  param parser already *parses* these; `EnumVariant.fields` is `Vec<String>`, so
  the types are dropped. Keeping them is the remaining work (plan §12 Q4).

> `return`-statement checks are **done**, not pending: `check_return_type` is
> called from both the body's tail expression and every explicit `return e`
> (`typecheck::early_return_mismatch_warns` and friends). An earlier revision of
> this list said otherwise.

---

## Verification recipe (run after every chunk)

```bash
# Rust: unit + CST/AST differential over the repo corpus
cd rust && cargo test --lib            # expect: all pass (587 as of the audit)
cargo test --lib typecheck::           # checker unit tests
cargo test --lib prescan_tests         # signature side-table tests
cargo test --test type_annotations     # the annotation *grammar* (all binding forms)

# TS integration (builds the binary via global-setup)
cd ts && npx vitest run test/type-annotations.test.ts test/type-warnings.test.ts
npx vitest run                         # full suite; expect no regressions
cd .. && ./ts/bin/test-examples.ts     # example goldens incl. typed.ptl (27)

# Editor support (the grammar must keep up with the parser)
cd editor-support/tree-sitter-petal && npx tree-sitter generate && npx tree-sitter test

# End-to-end spot checks
B=rust/target/debug/petal
$B run examples/typed.ptl                            # runs clean, no warnings
$B check --json -e 'let x: int = "hi"'               # {"ok":true,"warnings":[…]}
$B check --strict -e 'let x: int = "hi"'; echo $?    # exit 1
$B show-ast --json -e 'let x: banana = 5'            # ty: {name:"banana",resolved:null}
$B check -e 'state var n: int = 0
set n = "hi"'                                        # warns on the cell write
$B check -e 'let xs: list<int> = [1]'                # "parameterized types are not supported"
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
- **A fifth place: the tree-sitter grammar.** `editor-support/tree-sitter-petal`
  is the reference editor implementation and must model any new syntax, or `.ptl`
  files using it stop highlighting. Edit `grammar.js`, re-run `tree-sitter
  generate`, and commit the regenerated `src/`. `editor-support/vim/syntax` is a
  stock-Vim mirror of the same thing.
- **Unknown type names** are now preserved as `TypeAnn { name, resolved: None }`
  (Chunk E1) and warned on by the checker — no longer dropped.
- **Serde:** AST types derive `Serialize` only (not `Deserialize`); `Type` must
  keep `Serialize` for `show-ast --json`.
- The `check` CLI command runs the type checker and prints warnings (stderr
  text / `--json warnings[]`), exiting 0; `check --strict` exits 1 when warnings
  exist. `run` prints warnings to stderr and always exits on runtime status.
