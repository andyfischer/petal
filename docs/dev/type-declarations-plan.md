# Optional Static Type Declarations — Tech Plan

Status: **proposed** · Author: investigation + plan, 2026-07-14

Adds *optional* type declarations to Petal. Annotations are written by the
programmer where they want them; everything else keeps working exactly as it
does today. The language stays dynamically typed at runtime — this adds a
compile-time checking *projection* on top, not a new execution model.

---

## 1. Goals, non-goals, and locked decisions

### Goals
- Optional type annotations on `let` bindings, function parameters, and function
  return types.
- A shallow, local **type checker** that catches obvious mismatches.
- Dynamic and static styles coexist in the same program (and same file).
- Absent annotation ⇒ an *inferred* ("implied") type where we can determine one,
  otherwise `any`.

### Non-goals (this phase)
- **No implicit casting.** The checker never inserts a conversion. Crossing
  types is done with the existing explicit cast builtins (`int()`, `float()`,
  `str()`).
- **No runtime type enforcement.** Checking is compile-time only; a dynamic
  value flowing into a typed slot is trusted at runtime (see §7).
- No generics/parameterized types (`list<int>`), no user-defined type aliases,
  no whole-program (Hindley–Milner) inference. All are noted as future work.
- No structural record types (`{name: string, age: int}`) as *declarations* —
  `record` is a single opaque type for now.

### Decisions (locked with the requester)
| Question | Decision |
|---|---|
| Enforcement on mismatch | **Warning, program still runs.** Non-fatal diagnostics; never blocks compilation. Consistent with the "forgiving types" line in `goals.md:239`. |
| Inference depth | **Shallow / local** — from literals and called function signatures within a scope; fall back to `any`. |
| Runtime boundary checks | **Static only** for phase 1. |
| Syntax & type-name style | **Lowercase**, `:` for bindings/params, `->` for return type. Names mirror `type()` output. |

### Roadmap reconciliation
`docs/dev/goals.md:142,180` currently describes types as "a projection … never
enforced." This plan is *consistent* with that: because mismatches are
**warnings, not errors**, we are surfacing inferred/declared shapes to the
programmer and tooling without enforcement. The one addition to the vision is
that shapes can now also be *declared* by the user, not only inferred. The
"Types as a projection" entry in `goals.md` should be updated to note that
optional user-written annotations feed the same projection.

---

## 2. Syntax

Lowercase type names, contextual (not reserved keywords — see §5). Colon before a
type on bindings/params; arrow before a return type.

```petal
let count: int = 0
let name: string = "Petal"

fn area(r: float) -> float
  3.14159 * r * r
end

fn greet(name: string)          // return type optional
  print("Hello,", name)
end

// mixing: annotated and un-annotated params in one signature is fine
fn scale(v, factor: float) -> float
  v * factor
end

// lambda PARAMETER annotations are unambiguous:
let double = fn(n: int) -> n * 2
```

### Lambda return-type wrinkle
Lambdas already use `->` as the **body** separator (`fn(x) -> x*2`,
`parse.rs:1306-1329`). Writing `fn(n: int) -> int -> n * 2` needs two arrows and
is ambiguous to a single-token-lookahead parser. **Phase-1 decision: support
lambda *parameter* annotations but NOT lambda *return* annotations.** Named `fn`
declarations get full param+return annotations (their body is a block delimited
by `end`, so `-> type` before the block is unambiguous). Revisit lambda return
types later if wanted (would likely need a different token, e.g. `fn(n: int): int
-> ...`).

### Type grammar (phase 1)
A type is a single lowercase name from a fixed vocabulary:

```
type      := 'any' | 'nil' | 'bool' | 'int' | 'float' | 'string'
           | 'list' | 'record' | 'function' | 'enum' | 'vec2'
           | 'f64_array' | 'element' | 'symbol' | 'dual'
           | 'handle' | 'pending'
```

These are exactly the strings `Value::type_name()` returns (`value.rs:83-104`),
plus `any`. Mirroring `type()` output means "the name you see at runtime is the
name you write in an annotation." Note two naming points to decide in review:
- Cast builtin is `str()` but the type name is `string`. Annotations use
  `string` (matches `type()`) and **also accept `str` as an alias** in type
  position (resolved in review); `str()` the cast is left as-is.
- `Map` surfaces as `record` and all callables as `function` (`value.rs:92-95`) —
  annotations follow those collapsed names.

Parameterized forms (`list<T>`, function arrow types, structural records) are
**future work** — the grammar is written so they can be added without breaking
the bare-name forms.

---

## 3. Type representation (new)

New module `rust/src/types.rs` (or `compiler/types.rs`):

```rust
/// A declared or inferred static type. `Any` is the dynamic escape hatch:
/// it is compatible with everything and suppresses checking.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Type {
    Any,
    Nil,
    Bool,
    Int,
    Float,
    String,
    List,        // element type deferred (future: List(Box<Type>))
    Record,      // structural fields deferred
    Function,    // param/return arrow deferred
    Enum,        // named enums deferred (future: Enum(String))
    Vec2,
    F64Array,
    Element,
    Symbol,
    Dual,
    Handle,
    Pending,
}

impl Type {
    /// Parse a type-position identifier. Returns None for unknown names
    /// (checker emits an "unknown type" warning, treats as Any).
    pub fn from_name(s: &str) -> Option<Type>;
    /// The canonical spelling, == Value::type_name for concrete types.
    pub fn name(&self) -> &'static str;
}
```

**Compatibility / assignability** (the core check), `warning`-only:
- `Any` is compatible with any type in either direction (dynamic ↔ static).
- Otherwise types must be equal, **except** the documented numeric promotion:
  an `int` is assignable to a `float` slot (mixed arithmetic promotes to float,
  `Language_Guide.md:56`). `float`→`int` is **not** assignable (needs explicit
  `int()`), consistent with "no implicit casting."
- Everything else: mismatch ⇒ one warning diagnostic.

Keep `Type` out of the serialized IR for now (compile-time only side tables,
§6) so we don't churn the IR schema / `program.rs` `Term` layout.

---

## 4. Pipeline changes for parsing annotations

The parser drives **both** a direct AST and a CST event stream; the
CST-projected AST is authoritative and a `debug_assert_eq!` checks the two match
(`cst/driver.rs:49-57`). So every new piece of syntax is a **four-place**
coordinated change, or debug builds/tests fail:

| Place | File | Change |
|---|---|---|
| 1. Token consume + AST build + CST events | `parse.rs` | consume `:`/`->` + type name; emit `ev_open(TypeAnnotation)…ev_close` |
| 2. New node kind | `cst/mod.rs:56-118` | add `SyntaxKind::TypeAnnotation` (and `ReturnType`) |
| 3. Projection | `cst_project.rs` | re-derive the type field from the CST node |
| 4. AST fields | `ast.rs` | add optional type fields |

No lexer change needed: `Token::Colon` (`lexer.rs:71`) and `Token::Arrow`
(`lexer.rs:74`) already exist, and type names stay `Token::Ident`.

### AST field changes (`ast.rs`)
- `StmtKind::Let { name, value }` → add `ty: Option<Type>` (`ast.rs:184-187`).
- Introduce a `Param { name: String, ty: Option<Type> }` struct and change
  `FnDecl.params: Vec<String>` → `Vec<Param>` (`ast.rs:193-197`), plus add
  `ret: Option<Type>`.
- `ExprKind::Lambda.params: Vec<String>` → `Vec<Param>` (`ast.rs:100-103`); no
  `ret` for lambdas (see §2).
- **Shared-function ripple:** `parse_param_list` (`parse.rs:449-462`) and
  `param_names` (`cst_project.rs:1004-1009`) are reused by fn decls, lambdas,
  **and enum variants** (`parse.rs:308`). Enum variant fields (`Circle(radius)`)
  keep name-only semantics in phase 1 — either fork the param parser or have
  enum projection ignore the `ty` slot. Recommend: make the shared parser
  produce `Param` everywhere and have `EnumVariant` simply not read `ty`.
- Update the exhaustive walkers `walk_expr`/`walk_stmt` and their `_mut` twins
  (`ast.rs:269-412`, `425-568`) for any changed shapes.

### Parser insertion points (`parse.rs`)
- `let`: between `expect_ident()` and `expect(Assign)` (`parse.rs:206-207`).
- fn params: after `expect_ident()` in `parse_param_list` (`parse.rs:453`).
- fn return: after the `ParamList` close, before the body block
  (`parse.rs:293-295`).
- Precedent for colon parsing already exists (import lists `parse.rs:258`,
  record fields `parse.rs:1052`).

### Projection (`cst_project.rs`)
- `LetStmt` at `:181-185`, `FnDecl` at `:188-193`, `param_list`/`param_names`
  at `:361-367`/`:1004-1009`. Each pulls the new `TypeAnnotation` child.
- The existing colon-handling in `import_stmt` (`:332-336`) is the pattern to
  copy.

---

## 5. Type names are contextual, not reserved

`int`, `float`, `str` are **existing builtin functions** (the cast functions).
They must remain callable as identifiers. Therefore type names are recognized
**only in type position** (immediately after `:` in a binding/param, or after
`->` in a return). The lexer keeps emitting `Token::Ident`; the parser decides
"this identifier is a type" purely from grammar position. This keeps casts
(`int(x)`) working and avoids reserving a pile of common words.

---

## 6. Carrying annotations to the checker

- **Desugar** (`desugar.rs`) preserves everything as long as new
  `StmtKind`/`ExprKind` shapes are handled in `lift_stmt` (`:96`) and
  `LiftAt::visit_expr` (`:153`); it already preserves spans on moved nodes. The
  checker runs **after** desugar so it sees canonical `x = f(x)` form.
- **Function signatures:** `prescan_declarations` (`compiler/mod.rs:657`) already
  does a forward-reference pre-pass collecting function names/arities. Extend it
  to also collect declared `(param types, return type)` into a compile-time side
  table keyed by function/overload. This makes call-site checking possible even
  with forward references, and respects arity-based overloading
  (`Function_Overloading.md:91`).
- **Bindings:** the checker keeps its own scoped type environment mirroring the
  compiler's `scopes` stack (`compiler/mod.rs:50`) — no need to add a `ty` field
  to IR `Term` (`program.rs:242`) in phase 1.

---

## 7. The checker pass

New module `rust/src/typecheck/` (modeled on `lint/` and `desugar.rs` — both are
established AST-pass templates). Runs on the projected+desugared AST, before/
alongside `compile_stmt`, ideally invoked from `compile_module` right after
`desugar::desugar` (`compiler/mod.rs:278`).

### Algorithm (shallow, local, warning-only)
1. Walk statements in order, maintaining a scope stack of
   `name -> (declared: Option<Type>, inferred: Type)`.
2. **infer(expr) -> Type**, bottom-up:
   - Literals → their obvious type (`42`→`int`, `3.14`→`float`, `"x"`→`string`,
     `true`→`bool`, `nil`→`nil`, `[…]`→`list`, `{…}`→`record`, `#f80`→`record`,
     `EnumVariant(...)`→`enum`, `vec2(…)`→`vec2`).
   - Identifier → its recorded type (declared if present, else inferred, else
     `any`).
   - **Cast calls** `int(x)`/`float(x)`/`str(x)` → `int`/`float`/`string`. This is
     how the programmer explicitly satisfies a type — the checker treats these as
     the sanctioned conversions.
   - `BinaryOp` → result type from operand types using runtime promotion rules
     (`int op int`→`int`; any `float`→`float`; comparisons→`bool`;
     string `+`→`string`). Unknown operand ⇒ `any`.
   - Call to a known function → its declared/return type, else `any`.
   - `if`/`match`/`for`/`while` in value position → join of branch types (equal ⇒
     that type; differ ⇒ `any` in phase 1). (These are value-expressions per the
     control-flow work.)
   - Anything unknown ⇒ `any`.
3. **check** at these sites, emitting a warning when an inferred **concrete**
   (non-`any`) type is *incompatible* (per §3) with a declared type:
   - `let x: T = expr` — infer(expr) vs `T`.
   - Call arguments vs declared parameter types.
   - Function body's last-expression type vs declared return type.
   - Reassignment (`rebind_name`, `stmt.rs:383`) of an annotated binding.
4. `any` on **either** side suppresses the warning (dynamic ↔ static boundary is
   trusted). This is what lets dynamic and static code interoperate freely.

Because it is warning-only, the checker **never** returns `Err`; it accumulates
a `Vec<Diagnostic>` and compilation proceeds regardless.

### Why "no implicit cast" falls out naturally
The checker has no cast-insertion path at all. When types don't line up and
neither is `any`, it just warns; the fix the programmer applies is an explicit
`int()`/`float()`/`str()` call, which the checker then sees as producing the
target type. Nothing implicit ever happens.

---

## 8. Diagnostics (new non-fatal channel)

Today there are two error models (`compiler` agent findings):
- Hard `Result<_, String>` compile/parse errors (abort).
- Deferred `TermOp::Error` runtime errors (fire only if executed).

Type warnings need a **third, non-fatal** channel: a `Vec<Diagnostic>` produced
alongside the compiled `Program` and surfaced without aborting. A `Diagnostic`
carries a `SourceSpan` (`source_map.rs:24-39`, already file-tagged for
multi-module) and a message.

Rendering reuses existing infrastructure: **`format_source_snippet`**
(`backend/errors.rs:131`, already `pub`) renders the gutter+caret underline from
a raw source string + span, and `format_position` (`errors.rs:97`) prints
`[file line N, column M]`. The checker has `Expr.span`/`Stmt.span` and the module
source, so it can produce carets directly with no IR involvement.

Surfacing points:
- `petal run` / `petal check` — print warnings to stderr (JSON mode: a
  `warnings[]` array). `check` (`cli/args.rs:17`) currently does "no type check,
  no execution" — it becomes the natural type-check-only entry point.
- MCP `CheckSnippet` / `TestSnippet` — include warnings in output for the
  agent/editor loop.
- Editor support / LSP-ish tooling can consume the structured `warnings[]`.

---

## 9. Interactions to keep in mind
- **Overloading is by arity** (`Function_Overloading.md:91`). The checker groups
  candidates by arity and, for a given call, checks against the matching-arity
  overload's declared types. It does **not** introduce type-based dispatch.
- **Pending / Dual / Vec2 / f64_array / Handle** are real runtime types with
  `type_name`s; they get vocabulary entries so they can be annotated, but most
  users won't. `pending` is strict-absorbing at runtime — the checker treats a
  `pending`-typed value conservatively (usually `any`).
- **Immutability & reassignment:** `let` can be reassigned; an annotated binding
  keeps its declared type across reassignments and warns if a later value
  conflicts.
- **Color literals** (`#f80`) desugar to records (`Language_Guide.md:34`), so
  they infer as `record`.

---

## 10. Implementation phases (TDD, one chunk per commit)

Each chunk lands with tests first (unit + `.ptl` integration via the vitest
suite / `TestSnippet` MCP), matching the repo's `swe-work` discipline.

- **Chunk A — `Type` core.** New `types.rs`: enum, `from_name`, `name`,
  `is_assignable_to`. Pure unit tests. No parsing yet.
- **Chunk B — Parse binding & param annotations.** `let x: T`, `fn f(p: T)` across
  `parse.rs` + `cst/mod.rs` (`TypeAnnotation`) + `cst_project.rs` + `ast.rs`
  (`Param`, `Let.ty`). AST snapshot / round-trip tests; the `debug_assert_eq!`
  differential must stay green.
- **Chunk C — Parse return types.** `fn f(...) -> T`. Add `FnDecl.ret`,
  `SyntaxKind::ReturnType`. (Lambda param annotations here; lambda return types
  deferred.)
- **Chunk D — Preserve through desugar + prescan side table.** Handle new shapes
  in `desugar.rs`; collect signatures in `prescan_declarations`.
- **Chunk E — The checker.** `typecheck/` pass: scoped env, shallow inference,
  assignability checks, `Vec<Diagnostic>` sink. Unit tests over crafted ASTs +
  golden warning snapshots.
- **Chunk F — Surface diagnostics.** Non-fatal warning channel through
  `compile_modules` → `env` → CLI; `petal check` runs the checker; JSON
  `warnings[]`; MCP `CheckSnippet`/`TestSnippet` wiring; caret rendering via
  `format_source_snippet`.
- **Chunk G — Docs & examples.** `Language_Guide.md` Types section gains the
  annotation grammar; `Builtins.md` cross-links casts as the sanctioned
  conversions; update `goals.md` "Types as a projection"; add an
  `examples/typed.ptl`; update `README.md:13` from aspiration to reality.

---

## 11. Testing strategy
- **Unit:** `Type::is_assignable_to` truth table (incl. `int→float` yes,
  `float→int` no, `any` both ways); `from_name` incl. unknown → warning.
- **Parser:** AST snapshots for annotated `let`/`fn`/params/return; confirm
  un-annotated code is byte-for-byte unchanged in AST; differential
  `debug_assert_eq!` stays green; lint/format round-trips over annotated source.
- **Checker:** golden tests pairing a `.ptl` snippet with its expected
  `warnings[]` (mismatch warns; `any` suppresses; explicit cast clears the
  warning; numeric promotion doesn't warn).
- **Integration:** existing example programs must produce **zero** warnings
  (they're all un-annotated ⇒ `any`) and run identically — proves opt-in-ness.
- **CLI:** `petal check typed.ptl` exits 0 with warnings on stderr; `--json`
  emits `warnings[]`.

---

## 12. Open questions for review
1. ~~Accept `str` as an alias for `string` in type position?~~ **Resolved: yes**,
   `str` and `string` are both accepted in type position; `from_name` maps both
   to `Type::String`.
2. Should `petal check` exit non-zero when warnings exist (useful for CI), even
   though `run` never fails? Recommend: `check --strict` exits non-zero;
   plain `check`/`run` stay zero.
3. Do we want a per-file pragma (e.g. `// @strict`) to opt individual files into
   error-level enforcement later? Out of scope now; the warning channel is
   designed so this is a small future addition.
4. Enum variant field annotations (`Circle(radius: float)`) — deferred, but the
   shared param parser makes it a cheap future add. Confirm deferral.

---

## 13. Key file references
- Value model / type names: `value.rs:22-104`; casts `builtins/math.rs:85-112`,
  `builtins/io.rs:19-32`.
- Parse insertion: `parse.rs:203-211` (let), `285-299` (fn), `449-462`
  (params); tokens `lexer.rs:71,74`.
- CST: `cst/mod.rs:56-118` (SyntaxKind), `cst/driver.rs:37-59` (pipeline +
  differential), `cst_project.rs:181-193,361-367,1004-1009`.
- AST: `ast.rs:100-103,184-197,269-568`.
- Compile / scopes / prescan: `compiler/mod.rs:50,274-297,536-565,657`;
  `stmt.rs:22-26,383`; `expr.rs:7-73`.
- Desugar template: `desugar.rs:59,96,153`.
- Diagnostics: `source_map.rs:24-131`, `backend/errors.rs:97,131`.
- Pass template + `check` command: `lint/mod.rs`, `cli/args.rs:17`.
- Roadmap to reconcile: `goals.md:142,180,239`; README claim `README.md:13`.
