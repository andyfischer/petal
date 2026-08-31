# Optional Static Type Declarations — Progress & Handoff

Living status tracker for implementing optional static type declarations.
**Design rationale lives in [`type-declarations-plan.md`](type-declarations-plan.md)** — read it first.
This doc tracks *what is done, what remains, and how to continue*.

Last updated: 2026-08-30 (review pass: recorded chunks O–R, which had shipped
but were never written down) · Branch: `main`

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
| G — docs & examples | ✅ done | `4b3d9d7` | Language_Guide Types section, CLI/Builtins/goals reconcile, `examples/console/typed.ptl`, README |
| H — `state` annotations | ✅ done | (audit) | `state x: t = …` in all three spellings; checked like a `var` cell |
| I — editor support | ✅ done | (audit) | tree-sitter models `type_annotation`/`return_type`/`parameter`; vim `petalType` |
| J — parameterized-type error | ✅ done | (audit) | `list<int>` gets one targeted message instead of three positional ones |
| K — class names as types | ✅ done | (classes) | `Type::Class(ClassId)` + `Type::resolve`; `fn f(r: Rect)` checks, field reads are typed |
| L — receiver, field & arity diagnostics | ✅ done | (this) | fatal receiver-annotation check; undeclared-field warning; signatures carried on bindings; no-matching-arity warning for fns, constructors and methods |
| M — annotations drive static dispatch | ✅ done | `dc652e1` | `check_module` also returns the method-call sites it pinned to one class; the compiler binds those straight to `fn Class.method` |
| N — stale-label fallback | ✅ done | `53a2251` | an unpinned call carries its declaration's class, consulted only when the receiver's label names no class in this program; `Program.class_names` answers that at runtime |
| O — unused-result lint | ✅ done | `6cc18e5` | `typecheck/unused.rs`: warn when a known-pure builtin's result is discarded (`push(xs, x)` as a statement) |
| P — builtin return types | ✅ done | `67b238a` | `typecheck/builtin_types.rs`: the checker learns `len(xs)` is `int`, `sqrt(x)` is `float`, and that `abs`/`min`/`clamp` preserve int-ness |
| Q — inference gained a *rewriting* consumer | ✅ done | `67b238a` | `lint`'s drop-identity-casts rule detects via `typecheck::find_redundant_casts`, so inference now decides an edit, not just a warning |
| R — tighter unknown-type carets | ✅ done | `12e97e6` | `TypeAnn` carries its own `SourceSpan`; the unknown-type warning underlines just the type name |

Legend: ✅ done · 🚧 in progress · ⬜ todo

> **Chunks O–R were shipped but unrecorded** until the 2026-08-30 review. O, P
> and Q predate N chronologically (2026-07-24 and 2026-08-02); the letters are
> labels, not an order. R closes what the follow-up list called "tighter
> unknown-type carets".

### Review pass (2026-08-30)

Re-verified against the current tree. Everything the board claims still holds;
`cargo test --lib` is now **775** passing (was 587 at the audit), with 84
`typecheck::` unit tests, 8 `type_annotations`, and 16 `static_dispatch`. Spot
checks reconfirmed: the unknown-type caret underlines just `banana`,
`check --strict` exits 1, and `list<int>` still gets its targeted parse error.

The checker has been kept current with the syntax that landed since the audit,
each in the same commit as the feature: `get` (`94b3982`), `??` on an absent
field (`83c1f63`), `?.` (`cadf2e8`), and `config let` (`15d9da0`). The builtin
return-type table grew twice, for the new string/list builtins (`6fcabef`) and
for int-preserving `clamp`/`round` and the failable parsers (`405562a`).

**Raised stakes worth knowing (chunk Q).** Inference is no longer advisory. When
`lint --fix` drops an identity cast it is acting on `find_redundant_casts`, so a
*wrong* inferred type becomes a wrong source rewrite rather than a spurious
warning. The mitigation is the rule already stated in `builtin_types.rs`: list
only certainties, and compute argument-dependent results rather than assuming
them. Treat any addition to that table as a correctness change.

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

## What exists now (A–R shipped)

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
- `check_module(stmts, &fn_signatures, &classes) -> (Vec<Diagnostic>,
  MethodDispatch)`, invoked from `compile_module` after
  `prescan_declarations`. The second product is chunk M's `span -> class name`
  map of statically resolved method calls; the compiler keeps it and consumes
  it in `compile_expr`. Scoped
  `Vec<HashMap<String, VarType>>` env; folded `check_expr` doing conservative
  shallow inference (any ambiguity ⇒ `Any`, which suppresses); check sites:
  unknown type name, typed `let`/`state`, reassignment, call args, fn return
  tail, a field read on a class-typed value, and the argument *count* of a call
  (fn, constructor, method). Never errors. The entire un-annotated corpus stays
  silent — `petal check` over every `.ptl` in the repo must not gain a warning.
- `VarType.fns: Vec<FnSignature>` carries what a *binding* may be called with,
  one entry per arity: `Type::Function` has no arrow inside it, so `let f =
  fn(n: int) -> n` / `let h = g` would otherwise lose the signature. Filled by
  `fn_candidates` (lambda literal, bound name, un-shadowed module fn), cleared
  on re-assignment, and empty for anything unknown — which is what keeps
  builtins, parameters and imported callables unchecked. Nested `fn`
  declarations are bound into the block's scope by `bind_nested_fns`, so an
  inner declaration shadows a same-named module one instead of being checked
  against it.
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
`{ "name": "banana" }` for an unknown name (`resolved` is omitted); the
`ty`/`ret` field itself is omitted when un-annotated. Schema documented in
[`../CLI.md`](../CLI.md) (`TypeAnn`, `Type`).

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
  to a shipped feature. `examples/console/typed.ptl` (+ `test/example-golden/typed.json`)
  runs clean and is in the manifest.

### `var` / `set` cells — DONE
See [`var.md`](../var.md) (Cells).
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

## What's next

Recommended: **`Type::Num`** (plan §12 Q5). It is the only follow-up with a
consumer already blocked on it in-tree, and it is small.

`rust/src/classes.rs:286` reads `const RECT_FIELD_TYPE: Option<Type> = None;`
with a comment explaining why: a rect edge is a *number* — `int` for pixel
geometry, `float` for the sub-pixel geometry layout and animation produce — and
the language has no name for "int or float". Declaring `int` would be a lie the
constructor could only keep by truncating, which is the implicit cast Petal
does not do. So the built-in class the whole UI corpus is written against has
un-annotated fields, and the static catch is given up:

```
$ petal check -e 'let r = Rect("a", 1, 2, 3)'      # silent, exit 0
$ petal run   -e 'let r = Rect("a", 1, 2, 3)'
Error: Rect(): field `x` expects a number, got string
```

Scope is contained — the same shape as chunk A:

- `types.rs`: a `Type::Num` variant; `from_name("num")`; `name()` returns
  `"num"`. It has no runtime `type_name` (like `Any` and `Class`), so exclude it
  from `concrete_types()` in `from_name_round_trips_every_type` and
  `name_matches_value_type_name_for_concretes`.
- `is_assignable_to` (`types.rs:150`) gains two arms: `Int`/`Float`/`Dual` →
  `Num` yes; `Num` → `Int`/`Float` **no** (needs an explicit cast, keeping "no
  implicit casting"). Extend the existing truth-table test.
- Flip `RECT_FIELD_TYPE` to `Some(Type::Num)` and pin the new compile-time
  warning with a test.
- Docs: the type vocabulary in `plan.md` §2, the Language Guide's Type
  Annotations section, and the tree-sitter/vim keyword lists (the fifth place —
  see Gotchas).

Consider alongside it, in rough order of value:

- **Method return types in inference.** `r.center_x()` infers `any` even when
  the receiver's class is pinned and the method declares a return type. The
  blocker is confirmed structural: `classes::MethodDef` carries only
  `{ name, arity }` (`classes.rs:41-47`), so the class table would need to hold
  an `FnSignature` per method. Worth doing after `Num`, since the built-in
  `Rect` methods are exactly the ones that would become typed.
- **Enum variant field annotations** (`Circle(radius: float)`, plan §12 Q4).
  The shared param parser already *parses* these; `EnumVariant.fields` is still
  `Vec<String>` (`ast.rs:207`), so the types are dropped on the floor. Cheap.
- **Structured warnings in `run --json`.** `run` deliberately prints warnings to
  stderr even under `--json`, so stdout stays clean for JSON consumers
  (`cli/handlers.rs:88-92`). A `warnings[]` channel on the run report (reusing
  `warnings_json`) would let `TestSnippet` return them as data. Small, but it
  needs a decision about whether the report gains the field or `--json` gains a
  flag.

## Follow-up ideas (not scheduled)
- **Parameterized / richer types** — `list<int>`, arrow types, structural
  records, user type aliases, deeper (non-local) inference. All explicitly
  deferred by the plan; writing one now gets a targeted parse error naming the
  bare type instead of a misleading downstream one (audit chunk J).
- **Per-file `// @strict` pragma** to opt individual files into error-level
  enforcement (plan §12 Q3).
- **Compile-time unknown-method warnings.** Calling a method a class does not
  have is a *runtime* error (`No method 'nope' on class Rect`). A check-time
  warning was considered and dropped: dispatch also reaches every global native,
  including ones an embedder registers that the checker cannot see, so the
  warning would fire on working code — the one outcome this pass avoids. The
  *arity* of a method the class **does** declare is checked (chunk L): dispatch
  matches a user method by name alone, so a mismatched count cannot fall through
  to a global and is always a runtime error. A callable field of the same name
  wins over the method, so a class that declares such a field is skipped.

> Method return types in inference, enum variant field annotations, and
> structured `run --json` warnings moved up to [What's next](#whats-next).

> `return`-statement checks are **done**, not pending: `check_return_type` is
> called from both the body's tail expression and every explicit `return e`
> (`typecheck::early_return_mismatch_warns` and friends). An earlier revision of
> this list said otherwise.

---

## Verification recipe (run after every chunk)

```bash
# Rust: unit + CST/AST differential over the repo corpus
cd rust && cargo test --lib            # expect: all pass (775 as of 2026-08-30)
cargo test --lib typecheck::           # checker unit tests (84)
cargo test --lib prescan               # signature side-table tests (4)
cargo test --test type_annotations     # the annotation *grammar* (8)
cargo test --test static_dispatch      # chunk M/N pinning + its guards (16)

# TS integration (builds the binary via global-setup)
cd ts && npx vitest run test/type-annotations.test.ts test/type-warnings.test.ts
npx vitest run                         # full suite; expect no regressions
cd .. && ./ts/bin/test-examples.ts     # example goldens incl. typed.ptl (27)

# Editor support (the grammar must keep up with the parser)
cd editor-support/tree-sitter-petal && npx tree-sitter generate && npx tree-sitter test

# End-to-end spot checks
B=rust/target/debug/petal
$B run examples/console/typed.ptl                            # runs clean, no warnings
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

---

## Chunk M — annotations drive static dispatch

An annotation now buys something beyond a warning, which is worth knowing when
weighing future checker work: it decides *where a method call is bound*.

`typecheck::check_module` returns a second product alongside the diagnostics —
`MethodDispatch`, a `span -> class name` map of the `recv.m()` sites whose
receiver the pass pinned to exactly one class. `Compiler::compile_module` keeps
it (replaced per module: spans are file-local) and `compile_expr`'s method-call
arm binds those sites to the `fn Class.m` scope binding, emitting an ordinary
`TermOp::Call` instead of a `TermOp::MethodCall`.

**Why.** Runtime dispatch reads the class label the *receiver* carries, and a
value in `state` outlives the edit that reshaped its class. Binding the call to
the declaration is what lets a live edit take effect on values that predate it;
the label stays a label. A slice also gets the exact callee rather than the
`dispatch_targets` may-edge.

**The guards are the design.** Resolution must never change what a working
program does, so a site is left dispatched whenever the two mechanisms could
disagree — a field of the same name (data beats declarations), a method the
class does not declare (it can still reach a global native), an arity no
overload accepts, and — the subtle one — *a declaration written below the call*.
Nothing in Petal hoists, so binding to a later declaration would either read nil
or, from inside a function body, reference a term the caller's block cannot
see. `Compiler::declared_methods` records qualified names at emission order to
enforce that; built-in class methods are exempt, being natives that exist before
the program starts. Each guard has a test in `rust/tests/static_dispatch.rs`.

**Consequence for the checker's binding rules.** An un-annotated `state`/`var`
is `any` by deliberate decision, so it is *not* pinned. Chunk N covers that gap
without touching the rule.

---

## Chunk N — the stale-label fallback

Pinning left one case failing: an un-annotated `state c = C(1)` kept
dispatching on the label, so renaming `C` reported `No method 'get' on class C`
against a value that had outlived its class.

The fix deliberately did **not** widen what the checker types. Inferring a class
from a mutable binding's initializer would break polymorphism through a single
binding, which works today and is pinned by
`class_live_edit.rs::a_live_label_still_wins_over_the_declarations_class` —
`state shape = Circle(2)` then `shape = Square(3)` must run `Square.area`.

Instead the class travels with the call as a *last resort*:

- `VarType::class_hint` records the class a declaration named for bindings the
  pass types as `any`. It is **not a type**: nothing is checked against it and
  it produces no warning.
- `MethodDispatch` gained a second map, `hints`, beside `pinned`. Both are
  guarded identically (a field of that name outranks the method; an arity no
  overload accepts is not a candidate).
- `TermOp::MethodCall` became a struct variant carrying `hint:
  Option<ConstantId>`, threaded through `Inst::MethodCall` to `do_method_call`,
  which consults it at step 3.5 — after the receiver's own class, before the
  handle and global-native fallbacks.
- `Program.class_names` (a `BTreeSet`, so IR serialization stays byte-stable
  for the lint round-trip) is what lets the VM ask the one question the class
  table cannot answer at runtime: *is this label a class that still exists
  here?* Only a label that fails that test — or an absent label — reaches the
  hint, so a live class always wins.

**Degradation:** IR loaded from JSON without `class_names` (`#[serde(default)]`)
treats every label as live, so hints never fire and dispatch behaves exactly as
it did before this chunk.
