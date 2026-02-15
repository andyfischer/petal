# Plan: Petal CLI Tooling & Vitest Test Suite

## Context

The Petal implementation has a clean internal pipeline (Lexer → Parser → AST → Compiler → IR) but no way to inspect intermediate representations. We need CLI commands to dump these formats (for debugging, testing, and future GUI tooling), and a vitest-based test suite that asserts on the JSON output of those commands.

**Answer to "multiple commands" question**: Yes — there are **three** distinct internal formats worth exposing:
- **Tokens** (`show-tokens`) — flat token stream from the lexer
- **AST** (`show-ast`) — parsed tree of Stmt/Expr nodes
- **IR** (`show-ir`) — compiled term graph (Terms, Blocks, Functions, Constants)

All three are useful: tokens for lexer debugging, AST for parser verification, IR for the dataflow graph visualization the GUI playground will need.

## Changes Overview

### Rust Changes (8 files modified, 3 new files)

| File | Action | Purpose |
|------|--------|---------|
| `rust-impl/Cargo.toml` | Modify | Add serde, serde_json deps |
| `rust-impl/src/lib.rs` | Modify | Register 3 new modules |
| `rust-impl/src/main.rs` | Rewrite | Delegate to cli module |
| `rust-impl/src/ast.rs` | Modify | Add `Serialize` derives |
| `rust-impl/src/lexer.rs` | Modify | Add `Serialize` derive to Token |
| `rust-impl/src/program.rs` | Modify | Add `Serialize` derives to all types |
| `rust-impl/src/constant_table.rs` | Modify | Add `Serialize` derives, `#[serde(skip)]` on dedup, add accessor methods |
| `rust-impl/src/source_map.rs` | Modify | Add `Serialize` derives |
| `rust-impl/src/cli.rs` | **New** | CLI arg parsing & subcommand dispatch |
| `rust-impl/src/ir_display.rs` | **New** | Human-readable IR text formatting |
| `rust-impl/src/ir_serialize.rs` | **New** | Serde helpers (HashMap key serialization) |

### TypeScript/Test Changes (8 new files, 1 modified)

| File | Action | Purpose |
|------|--------|---------|
| `package.json` | Modify | Add `@facetlayer/subprocess-wrapper` |
| `vitest.config.ts` | **New** | Vitest config pointing at `test/` |
| `test/helpers.ts` | **New** | Shared utilities: ensureBuild, showIrJson, showAstJson, etc. |
| `test/ir-basics.test.ts` | **New** | Constants, variables, arithmetic, registers |
| `test/ir-functions.test.ts` | **New** | Functions, closures, captures, lambdas, recursion |
| `test/ir-control-flow.test.ts` | **New** | if/else, for, while, match, short-circuit |
| `test/ir-data-structures.test.ts` | **New** | Lists, records, enums, field/index access |
| `test/ir-state.test.ts` | **New** | State keyword IR output |

---

## Phase 1: Rust Dependencies

**`rust-impl/Cargo.toml`** — Add serde ecosystem:
```toml
smallvec = { version = "1", features = ["serde"] }  # SmallVec serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

The `serde` feature on smallvec is required because `Term.inputs: SmallVec<[TermId; 4]>` must be serializable.

## Phase 2: Serialize Derives (mechanical pass)

Add `#[derive(Serialize)]` (or `#[derive(serde::Serialize)]`) to all types that appear in the output:

- **program.rs**: All 9 ID newtypes, `TermOp`, `Term`, `Block`, `FunctionDef`, `MatchArmMeta`, `Program`
- **ast.rs**: `Literal`, `BinOp`, `UnaryOp`, `Expr`, `ElseBranch`, `MatchArm`, `Pattern`, `AssignTarget`, `EnumVariant`, `Stmt`
- **lexer.rs**: `Token`
- **constant_table.rs**: `ConstantId`, `ConstantValue`, `ConstantTable` (with `#[serde(skip)]` on `dedup` field). Add `pub fn len()` and `pub fn values()` accessors for the text display.
- **source_map.rs**: `SourcePosition`, `SourceSpan`, `SourceMap`

**HashMap key issue**: `match_arms: HashMap<TermId, ...>` and `source_map: HashMap<TermId, ...>` have non-string keys. serde_json rejects integer map keys. Solution: **`ir_serialize.rs`** with a helper that converts TermId keys to string keys, used via `#[serde(serialize_with = "...")]`.

## Phase 3: New File — `ir_display.rs`

Human-readable text format for `petal show-ir <file>`. Structure:

```
=== Constants ===
  c0: nil
  c1: 42
  c2: "hello"

=== Functions ===
  fn0: add params=["a", "b"] body=block2 captures=[]

=== Blocks ===
block0 [root] regs=25
  t0 r0 = Constant(c1) []; x
  t1 r1 = Add [t0, t0]
  ...

block1 (parent: t5) params=["i"] regs=3
  ...
```

Each term shows: `t{id} r{register} = {op} [{inputs}] -> {child_blocks} ; {name}`

## Phase 4: New File — `cli.rs` + Rewrite `main.rs`

CLI grammar:
```
petal run <file>                    # execute program
petal run -e <code>                 # execute inline code
petal show-ir [--json] <file>       # display IR (text or JSON)
petal show-ir [--json] -e <code>
petal show-ast [--json] <file>      # display AST (text or JSON)
petal show-ast [--json] -e <code>
petal show-tokens [--json] <file>   # display tokens (text or JSON)
petal show-tokens [--json] -e <code>
petal <file>                        # backward compat → same as "run"
petal -e <code>                     # backward compat → same as "run"
```

Hand-rolled arg parsing (no clap). Data types:
```rust
pub enum Command { Run, ShowIr { json: bool }, ShowAst { json: bool }, ShowTokens { json: bool } }
pub enum SourceInput { File(String), Inline(String) }
pub struct CliArgs { pub command: Command, pub source: SourceInput }
```

`main.rs` becomes ~10 lines delegating to `cli::parse_args()` + `cli::execute()`.

Backward compat: if first arg doesn't match a known subcommand, treat as `run <file>`.

## Phase 5: Vitest Test Suite

**`vitest.config.ts`**: points at `test/**/*.test.ts`, 30s timeout for cold cargo builds.

**`test/helpers.ts`**:
- `ensureBuild()` — runs `cargo build` once per suite
- `showIrJson(code)` — calls `petal show-ir --json -e '<code>'`, parses JSON
- `showAstJson(code)`, `showTokensJson(code)`, `runPetal(code)` — similar
- `shellEscape()` — single-quote wrapping for safe shell embedding
- Uses `runShellCommand` from `@facetlayer/subprocess-wrapper`

**Test files** (each ~15-25 assertions):

- **ir-basics.test.ts**: integer/string/nil constants in table, Add/Sub/Mul/Div/Mod terms, root_block validity, register sequencing, Copy terms for variable refs
- **ir-functions.test.ts**: FunctionDef creation, MakeClosure emission, capture_names populated for closures, Call terms, lambda (null name), self_ref_register for recursive fns
- **ir-control-flow.test.ts**: Branch with 2 child_blocks, ForLoop with 1 child_block, WhileLoop with 2 child_blocks, Match with N child_blocks, And/Or short-circuit blocks
- **ir-data-structures.test.ts**: AllocList with correct input count, AllocMap with field constants, GetField/SetField, GetIndex/SetIndex, MakeEnumVariant
- **ir-state.test.ts**: StateInit emission, state_key non-null, StateWrite for assignment

## JSON Shape (for test authors)

serde's default externally-tagged enum representation:
- Unit variants: `"Add"`, `"Sub"`, `"Call"`, `"ForLoop"`
- Single-value variants: `{"Constant": 3}`, `{"MakeClosure": 0}`, `{"Assign": 5}`
- Struct variants: `{"AllocMap": {"fields": [1, 2]}}`
- ID newtypes transparent: `TermId(5)` → `5`

## Verification

1. `cd rust-impl && cargo build` — compiles with new deps
2. `petal show-ir examples/hello.ptl` — human-readable output
3. `petal show-ir --json examples/hello.ptl | jq .` — valid JSON
4. `petal show-ast --json -e 'let x = 1 + 2' | jq .` — valid JSON
5. `petal show-tokens --json -e 'let x = 1' | jq .` — valid JSON
6. `petal examples/hello.ptl` — backward compat still works
7. `./bin/test-each.sh` — all 16 examples still pass
8. `npx vitest run` — all vitest tests pass
