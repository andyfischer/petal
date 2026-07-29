# `var` / `set`: rules of record and remaining work

`var` (a mutable cell) and `set` (the only way to write one) are landed and in
use. This doc is what survived the investigation that produced them
(`docs/lowering-confusion-20260726.md`, deleted 2026-07-29 once its plan was
executed): the design rules the implementation is built on, the work that is
still open, and the followups nobody has picked up yet.

User-facing documentation lives elsewhere and is the place to send readers:
[`var` and `set`](../language-guide.md#var-and-set) in the Language Guide, the
[syntax overview](../syntax/overview.md#var-and-set), the exported-`var` rule in
the [module system](../module-system.md#an-exported-var-is-read-only-to-importers),
and [Cells and the frontier](../CLI.md#cells-and-the-frontier) for what
mutation costs the dataflow queries.

---

## 1. Rules of record

The load-bearing decisions. Code comments cite these by anchor; none is
re-litigated without a reason.

### Why the feature exists

Assigning to a name bound outside the current function does **not** modify that
binding — it would create a function-local shadow, silently. One control-flow
step further (inside an `if`, `while` or `for`) the same code did not even
compile: the phi would have had to initialize from a term in another function.
The split was an implementation detail showing through as a language rule, and
the honest half was the half that failed. Both halves are now a compile error
at the assignment site (§2a).

Whole functions in the corpus were no-ops because of it. `var`/`set` is the
escape hatch for the code that genuinely wanted mutation and could not have it;
code that was silently shadowing is **not** correctly migrated to `var` (it
would start actually mutating and change behavior) — that becomes a `let` local.

### Cells

`var x = e` allocates a `Value::Cell` — a one-value box in a heap slab — via
`CellNew`. Every source-level *read* of the name is a `CellRead`, every `set` a
`CellWrite`. The binding itself is never rebound, so a `var` leaves the SSA/phi
machinery entirely; that is what makes writes from inside a conditional, a
function, or a closure work at all. Closure capture needed no changes: captures
are by value and the captured value *is* the cell id, giving Lua/JS upvalue
semantics for free. `state var` puts the `CellNew` inside the `StateInit` block,
so the slot holds the cell and persistence is automatic — including one cell per
key for `state(key) var`.

### Containment

**No expression ever evaluates to a cell.** Reads dereference; there is no
syntax that yields the box. Storing a `var` in a record, passing it to a
function, or printing it all move the *contents* as of that moment, so the only
way to share a box is closure capture, which is lexically visible.

This invariant is what keeps the feature small: equality, hashing, `print`,
`value_to_json`, `get_state_json`, the type checker's value domain and
`HostData` all needed no changes. It is also the one thing that has actually
been breached in practice — an exported `var` reached through a selective
import used to forward the raw cell (`print(x)` → `<cell 0>`), because binding
kind rode only on the qualified name. Fixed in `bind_imports`; treat any new
name-binding path as a place to check the invariant again.

### Two write keywords, disjoint in both directions

`=` writes a `let` and errors on a `var`; `set` writes a `var` and errors on
anything else. Erroring in only one direction would leave `=` meaning two
opposite things depending on a declaration that may be far away — the exact
ambiguity `set` was chosen to remove. `set` never declares (an unknown name is
an error), takes field/index/compound targets, and `@` stays a `let`-only rebind
because it desugars to `x = f(x)`.

An exported `var` is readable by importers under every import form and writable
only by the module that declared it: a cross-module `set` names the owner.

### Provenance: a cell operand is an identity edge

A cell operand — of a `CellRead`, a `CellWrite`, or a `MakeClosure` capture —
names *which box*, not which value. The backward walk is defined over value
edges only, so it terminates at every `CellRead` and reports a first-class
`CellFrontier` (var name, declaration, complete static write set,
`host_writable`). A result with a non-empty frontier is by definition
incomplete: incompleteness lives in the return type, not in a convention.

- Backward is a *must* question, so may-writes are inadmissible as edges and go
  in the frontier as possibilities. Forward is already a *may* question, so
  `EdgeKind::CellMay` edges (decl → writes, decl → reads, write → reads) belong
  in it.
- Four consumers share the walk: `trace_provenance`/`slice`,
  `trace_dependents`, `TraceBuffer::explain`, and
  `backend::errors::format_provenance` (the "Caused by:" block).
- `slice` exposes `minimal()` (fallible, byte-identical to the old behaviour on
  cell-free programs) and `conservative()` (closes over cells to a fixed point).
  Conservative is sufficient in *terms*, not faithful in *order* — neither
  accessor yields an extractable program.
- Dynamic resolution matches on `CellId`, not on the declaration term: one
  declaration mints a fresh cell per execution (per key, per loop entry, per
  call). With the trace on, `explain` re-roots across the boundary and the chain
  is complete; the escape hatch's cost is paid only when the trace is off.

### Lexical shadowing (`let`, not `var`)

A `let`/`state` shadows from its own line onward: an assignment that lexically
precedes the declaration targets the outer binding and carries out, and one
after it is block-local.

```petal
let x = 1
for i in [1, 2, 3] do
  x = 5              // targets the outer x, and reaches it
  let x = i * 10
  x = x + 1          // body-local
end
print(x)             // 5
```

Two halves make this work and both are required: the phi pre-scan
(`AssignedNames` in `compiler/phi.rs`) is scope-aware, and `Compiler::note_shadow`
freezes the value the block carries out at the declaration. Making the pre-scan
lexical *alone* was worse than the original bug — `wire_phi_outs` reads the
block's final binding, so the shadowed local's value carried out to the outer
name.

The bug this fixed: a phi hoisted past the block that owns a `let` resolved the
name at the outer level, where it hit a prelude function of the same name
(`std::take`). It broke correct, mutation-free code in the shipped `petal-ui`
prelude, and the only difference was the local's *name*. Every addition to
`std` was a potential break of any user code using that name as a local.
Regression coverage: `ts/test/loop-carry-limitations.test.ts`, the walker tests
in `compiler/phi.rs`, and the `_wrap_segment` shape in
`ts/test/check-lowers.test.ts`.

---

## 2. Remaining work

### 2a. Cross-function `=` is an error — **done**

Assignment to a name bound outside the current function is a **compile error**,
at the assignment site, uniformly across all four declaration sites (module
`let`, module `state`, lambda capture, enclosing fn local) and all four
syntactic forms (`x =`, `xs[i] =`, `r.f =`, `@x` — which desugars to `=` and so
is caught by the same check). `var`/`set` is exempt: a `set` really does modify
the outer binding, which is the entire point of the escape hatch.

`Compiler::check_assign_to_outer_function_binding` returns false and the
statement is abandoned, mirroring `check_write_keyword`. That matters beyond
tidiness: a rejected assignment no longer emits the phi that would fail to
lower, so the compiles-but-does-not-lower state is gone for this shape and the
program stops at compile.

**Corpus.** In-repo migrated at `9ce440e` (51 sites across the five game files).
Before the flip, a sweep of all 115 in-repo and 52 downstream `.ptl` files —
`~/garden`, `~/biz/hotlaps`, `~/biz/experiment-cube-browser`,
`~/biz/experiment-todo-app`, `~/biz/petal-lang.org` — reported zero sites. The
measurement is the compiler, not a regex (a text sweep produces hundreds of
false positives from top-level assignments inside top-level `if`s, which are
legal and untouched):

```sh
find <roots> -name '*.ptl' -not -path '*/node_modules/*' -not -path '*/target/*' \
  | xargs -n1 petal check --json \
  | grep -c 'bound outside this function'
```

Re-run it for any root not in that list. Migration is per-site judgment: a
silent shadow becomes a `let` local, genuine intended mutation becomes `var` +
`set`. Note the vendored WASM build means `petal-lang.org` and `hotlaps` only
see the flip when they re-vendor
(`~/biz/petal-lang.org/docs/how-to-update-petal.md`).

### 2b. A real phase channel for compiler errors — **done**

Made much more visible by 2a, though it predates it. `Compiler` errors reached
the CLI as a plain `String`, so `classify_load_error` tagged them
`"phase": "parse"` by sniffing text — including the `var`/`set` disjointness
errors, which are neither parse nor lower.

`crate::error::{Phase, ErrorItem, LoadError}` now carries the phase from the
site that raises the error (`lex` / `parse` / `module` / `compile` / `lower`),
and `classify_load_error` is gone. The typed error is an **internal** channel:
every public `Env` API keeps `Result<_, String>` and is the typed one plus
`.to_string()`, so there is no caller ripple outside `rust/src/`. That works
only because `LoadError`'s `Display` reproduces the old strings byte for byte
(pinned by the `display_*` tests in `error.rs`); the internal typed entry points
are `cst::parse_source_phased`, `module::load_modules`,
`Compiler::compile_modules`, `Env::compile_source` and `Env::load_program_diag`.
`petal --json` gains an additive `errors[]` array (one entry per diagnostic);
`message`, `line` and `column` are unchanged. Coverage:
`ts/test/error-phase.test.ts`; documented at
[Error phases](../CLI.md#error-phases).

Not done here, deliberately: the lexer (~15 raise sites) and parser (~60
`Result<_, String>` methods) still format `" [line N, column M]"` into their
messages, so `ErrorItem::from_legacy` parses it back out. That is the one
remaining string-shape parser, isolated in one place so giving those two real
spans — a much larger job — can delete it.

The LSP was the first consumer of the typed channel and the reason it was worth
having. It used to re-derive positions from the message string, looking for a
`[line N, column M]` *prefix* that no producer emits as a prefix — so every
error landed at 0:0, and a multi-error compile collapsed into one diagnostic
whose message was several lines of text. It now emits one diagnostic per
`ErrorItem` at its own span, and collects definitions *before* compiling, so
go-to-definition and document symbols survive a semantic error instead of dying
with the whole file. Coverage: `rust/tests/lsp_tests.rs`.

---

## 3. Followups

The first two are done and are recorded here because *why* they are built the
way they are is not evident from the code. The rest are not scheduled, and each
stands on its own.

- **The grammars are now tied to the lexer** by `rust/tests/keyword_sync.rs`
  (done). The lexer exports `KEYWORDS` / `CONTEXTUAL_KEYWORDS` and that test
  re-derives the four downstream lists — the LSP completion list, the generated
  tree-sitter `src/grammar.json`, the vim syntax file, and the tree-sitter
  `highlights.scm` — from their real source files and asserts set equality, so
  a new keyword that misses one of them fails by name. Adding a keyword means
  touching all five files in one commit. Note also that the AST is built twice
  — `parse.rs` and `cst_project.rs`, reconciled by a `debug_assert_eq!` in
  `cst/driver.rs` plus whole-corpus differential tests — so any syntax change
  must land in both in the same commit.
- **`ts/test/check-lowers.test.ts`'s negative case is restored, via injected
  IR.** That file exists to prove `petal check` runs *lowering* and not just
  compilation — it is the CLI-level regression gate for the shadowed-name phi
  bug that shipped in the `petal-ui` prelude, and removing it is how that bug
  survived. Its original compiles-but-does-not-lower program was a
  cross-function assignment, which 2a made a compile error, so it stopped
  reaching lowering. **No source program can replace it.** Lowering has exactly
  two failure sites: the "unlowered op" arm in `lower.rs` is now unreachable
  (every `TermOp` variant is handled), and `FnLowerer::flat`'s "term tN in
  block bN not in this function" needs an input edge crossing a function
  boundary, which the compiler no longer builds from any source — ~50 candidate
  shapes were probed (match-arm phi, loop-carried closures, nested capture
  chains, `state var` in nested scopes, an exported `var` written through a
  nested fn, break/continue phi carry-outs, …) and every one lowers cleanly.
  So the gate injects the edge instead: it takes a real program's
  `show-ir --json`, repoints one root-block term's input at a term inside a
  function body, and feeds it to `check --ir -`. `Program::validate`
  (`rust/src/ir_validate.rs`) range-checks ids and arities but not
  function-boundary edges, so the corrupted graph imports cleanly and dies in
  lowering — which is the observation the gate needs. The IR is built in the
  test rather than checked in as a blob, so it cannot drift from the IR format,
  and the uncorrupted IR is asserted to pass first so the negative result is
  about lowering and not about `--ir` import being broken. `check --ir` is a
  real documented flag mirroring `run --ir` ([CLI](../CLI.md#check--validate-without-running)),
  not a test hook: its production use is CI-validating IR from a third-party
  emitter.
- **No cross-module write syntax, by design.** `set m.x = 1` is rooted at a
  module alias, which is not a binding; the owning module exports a function
  instead. If a first-class *shared* cell is ever wanted (an OCaml-style `ref`
  as a value, escaping into records and arguments), that is a different feature
  and has to be argued on its own — it would give up the containment invariant
  above.
- **`docs/examples/aspirational/`** (`metaprogramming.ptl`, `provenance.ptl`)
  still fails to parse. Unrelated to cells; they document syntax that does not
  exist yet.
