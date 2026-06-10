# Petal CLI Reference

The `petal` binary provides commands for running programs, validating syntax, inspecting each compiler stage (tokens, AST, IR), and querying the dataflow graph.

## Usage

```
petal <command> [options] <file>
petal <command> [options] -e <code>
```

### Shorthand

```
petal <file>           # same as: petal run <file>
petal -e <code>        # same as: petal run -e <code>
```

### Commands at a glance

| Command | Purpose |
|---------|---------|
| `run` | Execute a program |
| `check` | Lex + parse + compile only (no execution) |
| `explain` | Run with trace, walk back from a term to its ancestors |
| `show-tokens` | Lexer output |
| `show-ast` | Parser output |
| `show-ir` | Compiled IR (term graph) |
| `show-provenance` | Backward dataflow slice from a term |
| `show-dependents` | Forward dataflow slice from a term |
| `show-slice` | Minimal dataflow subgraph for one or more targets |
| `show-graph` | Graphviz DOT-format dataflow graph |

All inspection commands support `--json` for machine-readable output. `run`
additionally supports `--trace` and `--record-trace <path>` to capture a
per-term execution trace.

## Commands

### `run` — Execute a program

```
petal run [--json] [--trace] [--record-trace <path>] <file.ptl>
petal run [--json] [--trace] [--record-trace <path>] -e '<code>'
```

Runs the program and prints any output to stdout. Exits with code 1 on error.

Flags:

- `--json` — emit runtime/parse errors as structured JSON instead of a
  human-readable message. Shape: `{message, line, column, caused_by[], stack[], phase}`.
- `--trace` — write per-term execution events to stderr (inputs + result
  + source location) as they happen.
- `--record-trace <path>` — write the full trace buffer to `<path>` as JSON
  after the run completes. Useful for offline analysis and for feeding
  `petal explain`. Environment variable `PETAL_DEBUG=1` enables tracing
  without the flag.

### `check` — Validate without running

```
petal check [--json] <file.ptl>
petal check [--json] -e '<code>'
```

Lex, parse, and compile the program but do not execute it. Exits 0 if
compilation succeeds, 1 otherwise. With `--json`, emits either
`{"ok": true}` on success or `{message, line, column, phase, ...}` on
failure (`phase` is `"parse"` or `"compile"`).

Faster than `run` when you only care about syntactic validity.

### `explain` — Walk the dataflow graph backward from a term

```
petal explain [--json] --term <name|id> <file.ptl>
petal explain [--json] --term <name|id> -e '<code>'
```

Runs the program with tracing enabled, then walks the dataflow graph
backward from the target term, reporting every recorded value along the
chain of ancestors. Answers the question "why does `x` have this value?".

`--term` accepts:
- A variable name: `--term total`
- A bare numeric term id: `--term 72`
- The `t`-prefixed form: `--term t72`

With `--json`, returns `{name, term_id, chain: [{term_id, op, name, value, line, column}, ...]}`.

### `show-tokens` — Lexer token stream

```
petal show-tokens <file.ptl>
petal show-tokens -e '<code>'
petal show-tokens --json <file.ptl>
petal show-tokens --json -e '<code>'
```

Outputs the flat token stream produced by the lexer. Useful for debugging tokenization and verifying operator/keyword recognition.

**Text output** (default) — one token per line with index:

```
0: Let
1: Ident("x")
2: Assign
3: Int(1)
4: Plus
5: Int(2)
6: Eof
```

**JSON output** (`--json`) — array of tokens:

```json
["Let", {"Ident": "x"}, "Assign", {"Int": 1}, "Plus", {"Int": 2}, "Eof"]
```

#### Token JSON Encoding

Tokens use serde's externally-tagged enum representation:

| Category | Examples |
|----------|---------|
| Unit keywords/operators | `"Let"`, `"Fn"`, `"If"`, `"Else"`, `"For"`, `"In"`, `"While"`, `"Match"`, `"Return"`, `"Break"`, `"Continue"`, `"State"`, `"Enum"`, `"True"`, `"False"`, `"Nil"` |
| Unit operators | `"Plus"`, `"Minus"`, `"Star"`, `"Slash"`, `"Percent"`, `"PlusPlus"`, `"Eq"`, `"Ne"`, `"Lt"`, `"Le"`, `"Gt"`, `"Ge"`, `"And"`, `"Or"`, `"Bang"`, `"Assign"`, `"Pipe"` |
| Unit delimiters | `"LParen"`, `"RParen"`, `"LBrace"`, `"RBrace"`, `"LBracket"`, `"RBracket"`, `"Comma"`, `"Dot"`, `"Colon"`, `"Arrow"`, `"DotDot"` |
| Unit special | `"Newline"`, `"Eof"` |
| Value-carrying | `{"Int": 42}`, `{"Float": 3.14}`, `{"String": "hello"}`, `{"Ident": "myVar"}` |

### `show-ast` — Parsed AST

```
petal show-ast <file.ptl>
petal show-ast -e '<code>'
petal show-ast --json <file.ptl>
petal show-ast --json -e '<code>'
```

Outputs the parsed abstract syntax tree — an array of `Stmt` nodes. Useful for verifying parser behavior and understanding the tree structure before compilation.

**Text output** (default) — Rust `Debug` pretty-print of each statement.

**JSON output** (`--json`) — array of `Stmt` nodes:

```json
[
  {
    "Let": {
      "name": "x",
      "value": {
        "BinaryOp": {
          "op": "Add",
          "left": { "Literal": { "Int": 1 } },
          "right": { "Literal": { "Int": 2 } }
        }
      }
    }
  }
]
```

#### AST JSON Schema

All AST enum types use serde's externally-tagged representation. `Stmt` and
`Expr` are serialized as `{kind: <variant>, span: SourceSpan}` — the
`<variant>` shapes are listed in the `StmtKind` and `ExprKind` tables below.
The canonical definitions live in `rust/src/ast.rs`; the tables below cover
the common variants but are not exhaustive.

**StmtKind** (top-level statements):

| Variant | Shape |
|---------|-------|
| `Let` | `{"Let": {"name": string, "value": Expr}}` |
| `Assign` | `{"Assign": {"target": AssignTarget, "value": Expr}}` |
| `Expr` | `{"Expr": Expr}` |
| `FnDecl` | `{"FnDecl": {"name": string, "params": string[], "body": Stmt[]}}` |
| `EnumDecl` | `{"EnumDecl": {"name": string, "variants": EnumVariant[]}}` |
| `For` | `{"For": {"var": string, "iter": Expr, "body": Stmt[]}}` |
| `While` | `{"While": {"condition": Expr, "body": Stmt[]}}` |
| `Return` | `{"Return": Expr \| null}` |
| `Break` | `"Break"` |
| `Continue` | `"Continue"` |
| `State` | `{"State": {"name": string, "init": Expr, "id": number, "key": Expr \| null}}` — `key` set when the source uses the `state(expr) name = init` per-iteration form |

**ExprKind** (expressions):

| Variant | Shape |
|---------|-------|
| `Literal` | `{"Literal": Literal}` |
| `Ident` | `{"Ident": string}` |
| `BinaryOp` | `{"BinaryOp": {"op": BinOp, "left": Expr, "right": Expr}}` |
| `UnaryOp` | `{"UnaryOp": {"op": UnaryOp, "operand": Expr}}` |
| `Call` | `{"Call": {"function": Expr, "args": Expr[]}}` |
| `If` | `{"If": {"condition": Expr, "then_body": Stmt[], "else_body": ElseBranch \| null}}` |
| `Match` | `{"Match": {"subject": Expr, "arms": MatchArm[]}}` |
| `List` | `{"List": Expr[]}` |
| `Record` | `{"Record": RecordField[]}` |
| `FieldAccess` | `{"FieldAccess": {"object": Expr, "field": string}}` |
| `IndexAccess` | `{"IndexAccess": {"object": Expr, "index": Expr}}` |
| `Block` | `{"Block": Stmt[]}` |
| `Lambda` | `{"Lambda": {"params": string[], "body": Stmt[]}}` |
| `StringInterp` | `{"StringInterp": {"parts": string[], "exprs": Expr[]}}` — `parts` has one more element than `exprs` |
| `Element` | `{"Element": {"tag": string, "props": [string, Expr][], "children": JsxChild[]}}` |

**RecordField**: `{"Named": [string, Expr]}` or `{"Spread": Expr}`.

**JsxChild**: `{"Text": string}` or `{"Expr": Expr}`.

**Literal**: `"Nil"`, `{"Bool": bool}`, `{"Int": number}`, `{"Float": number}`, `{"String": string}`

**BinOp**: `"Add"`, `"Sub"`, `"Mul"`, `"Div"`, `"Mod"`, `"Eq"`, `"Ne"`, `"Lt"`, `"Le"`, `"Gt"`, `"Ge"`, `"And"`, `"Or"`, `"Concat"`

**UnaryOp**: `"Neg"`, `"Not"`

**AssignTarget**: `{"Name": string}`, `{"Field": [Expr, string]}`, `{"Index": [Expr, Expr]}`

**ElseBranch**: `{"Block": Stmt[]}`, `{"ElseIf": Expr}`

**MatchArm**: `{"pattern": Pattern, "guard": Expr | null, "body": Expr}`

**Pattern**: `"Wildcard"`, `{"Literal": Literal}`, `{"Variable": string}`, `{"Variant": {"name": string, "fields": Pattern[]}}`, `{"List": {"elements": Pattern[], "rest": string | null}}`, `{"Record": [string, Pattern][]}`

**EnumVariant**: `{"name": string, "fields": string[]}`

### `show-ir` — Compiled IR (term graph)

```
petal show-ir [--json] [--all] <file.ptl>
petal show-ir [--json] [--all] -e '<code>'
```

Outputs the compiled intermediate representation — the term graph that the evaluator executes. This is the primary command for GUI and tooling integration.

By default, builtin "phantom" terms (one per registered native function, see
below) are **hidden** so output starts with user code. Pass `--all` to
include them.

**Text output** (default):

```
=== Constants ===
  c0: true
  c1: 1
  c2: 2

=== Functions ===
  fn0: add params=["a", "b"] body=block1 captures=[]

=== Blocks ===
block0 [root] regs=24
  t21 r21 = Constant(c0) []
  t22 r22 = Branch [t21] -> block1, block2 ; x

block1 (parent: t22) regs=1
  t23 r0 = Constant(c1) []
```

Each term line: `t{id} r{register} = {op} [{inputs}] -> {child_blocks} ; {name}`

**JSON output** (`--json`) — the full `Program` object:

```json
{
  "id": 0,
  "source": "...",
  "terms": [...],
  "blocks": [...],
  "root_block": 0,
  "constants": {"values": [...]},
  "source_map": {"term_spans": {...}},
  "has_errors": false,
  "functions": [...],
  "match_arms": {...}
}
```

#### Program JSON Schema

The IR JSON is the complete compiled `Program` struct. All ID newtypes serialize as their inner integer (e.g. `TermId(5)` becomes `5`).

**Top-level Program**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Program ID (always 0 for CLI) |
| `source` | `string` | Original source code |
| `terms` | `Term[]` | All terms in the program |
| `blocks` | `Block[]` | All blocks in the program |
| `root_block` | `number` | BlockId of the root/entry block |
| `constants` | `{"values": ConstantValue[]}` | Constant table |
| `source_map` | `{"term_spans": {}}` | TermId → SourceSpan mapping (string keys) |
| `has_errors` | `boolean` | Whether the program has parse errors |
| `functions` | `FunctionDef[]` | All function definitions |
| `match_arms` | `{[termId: string]: MatchArmMeta[]}` | Match term → arm metadata (string keys) |

**Term**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Unique term ID |
| `op` | `TermOp` | The operation (see TermOp table below) |
| `inputs` | `number[]` | TermIds of input dataflow edges |
| `block_id` | `number` | BlockId this term belongs to |
| `block_next` | `number \| null` | Next term in block's linked list |
| `block_prev` | `number \| null` | Previous term in block's linked list |
| `name` | `string \| null` | Variable name if this is a binding |
| `register` | `number` | Register index for evaluation |
| `state_key` | `number \| null` | State key for StateInit/StateRead/StateWrite |
| `child_blocks` | `number[]` | BlockIds of child blocks (for control flow) |
| `in_loop` | `boolean` | Omitted when `false`. Marks state terms inside a loop body for per-iteration state. |

**TermOp** — serde's externally-tagged encoding:

| Op | JSON | Inputs | Child Blocks | Notes |
|----|------|--------|-------------|-------|
| Constant | `{"Constant": cid}` | none | none | Load constant by ConstantId |
| Error | `{"Error": cid}` | none | none | Parse error |
| Add | `"Add"` | [left, right] | none | |
| Sub | `"Sub"` | [left, right] | none | |
| Mul | `"Mul"` | [left, right] | none | |
| Div | `"Div"` | [left, right] | none | |
| Mod | `"Mod"` | [left, right] | none | |
| Neg | `"Neg"` | [operand] | none | Unary minus |
| Eq | `"Eq"` | [left, right] | none | |
| Ne | `"Ne"` | [left, right] | none | |
| Lt | `"Lt"` | [left, right] | none | |
| Le | `"Le"` | [left, right] | none | |
| Gt | `"Gt"` | [left, right] | none | |
| Ge | `"Ge"` | [left, right] | none | |
| Not | `"Not"` | [operand] | none | Logical not |
| And | `"And"` | [left] | [rhs_block] | Short-circuit; rhs_block evaluates right operand |
| Or | `"Or"` | [left] | [rhs_block] | Short-circuit; rhs_block evaluates right operand |
| Concat | `"Concat"` | [left, right] | none | String concatenation (`++`) |
| Copy | `"Copy"` | [source] or [] | none | Variable reference. Empty inputs = phantom (builtin/param) |
| Phi | `"Phi"` | [init] | none | Pure-dataflow join for names rebound inside child blocks. Sits in the parent block before the control-flow term; child frames overwrite via `Block.phi_outs`. See [MutabilityPlan.md](MutabilityPlan.md). |
| Branch | `"Branch"` | [condition] | [then_block, else_block] | if/else |
| ForLoop | `"ForLoop"` | [iterable] | [body_block] | for-in loop |
| NumericForLoop | `"NumericForLoop"` | [start, end] | [body_block] | non-allocating `for x in range(a, b)` integer loop |
| WhileLoop | `"WhileLoop"` | none | [cond_block, body_block] | while loop |
| Break | `"Break"` | none | none | |
| Continue | `"Continue"` | none | none | |
| Return | `"Return"` | [value] or [] | none | |
| MakeClosure | `{"MakeClosure": fid}` | [captured_values...] | none | Create closure for FunctionId |
| MakeOverloadSet | `"MakeOverloadSet"` | [closure0, closure1, ...] | none | Bundle arity-overloaded closures. See [Function_Overloading.md](Function_Overloading.md). |
| Call | `"Call"` | [callable, arg0, arg1, ...] | none | |
| MethodCall | `{"MethodCall": cid}` | [object, arg0, arg1, ...] | none | Method name as ConstantId; tries record field first, then scope/builtin lookup with `object` prepended. |
| StateInit | `"StateInit"` | [] or [explicit_key] | [init_block] | `state_key` set. Init expression lives in `child_blocks[0]` for lazy evaluation — only entered when the runtime key isn't yet in the persistent state map. Optional `explicit_key` is the value computed for `state(expr) name`. |
| StateRead | `"StateRead"` | none | none | `state_key` set |
| StateWrite | `"StateWrite"` | [value] or [value, explicit_key] | none | `state_key` set. Forwards the same `explicit_key` from the matching `StateInit` so the runtime resolves to the same `RuntimeStateKey`. |
| AllocList | `"AllocList"` | [elem0, elem1, ...] | none | |
| AllocMap | `{"AllocMap": {"fields": [cid, ...]}}` | [val0, val1, ...] | none | Field names as ConstantIds |
| AllocMapSpread | `{"AllocMapSpread": {"entries": [...]}}` | [spread_src..., named_value...] | none | Record literal with `...spread`. Each entry is `Spread(idx)` or `Named{key, idx}` referencing positions in `inputs`. |
| GetField | `{"GetField": cid}` | [object] | none | |
| SetField | `{"SetField": cid}` | [object, value] | none | |
| GetIndex | `"GetIndex"` | [object, index] | none | |
| SetIndex | `"SetIndex"` | [object, index, value] | none | |
| AllocElement | `{"AllocElement": {"tag": cid, "prop_keys": [cid, ...]}}` | [prop_val0, ..., child0, ...] | none | JSX-like element. `prop_keys.len()` separates prop values from children in `inputs`. |
| MakeEnumVariant | `{"MakeEnumVariant": cid}` | [field_values...] | none | Variant name as ConstantId |
| Match | `"Match"` | [subject] | [arm_body_blocks...] | Arm metadata in `match_arms` |

**Block**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Unique block ID |
| `parent_term_id` | `number \| null` | TermId that created this block (null for root and function bodies) |
| `entry` | `number \| null` | TermId of first term in this block's linked list |
| `param_names` | `string[]` | Parameter names (function params, for-loop variable) |
| `register_count` | `number` | Total registers needed for this block's frame |
| `phi_outs` | `PhiOut[]` | Carry-outs: when this block's frame pops, copy each `src_term`'s register into the parent block's `Phi` term register. Drives the rebinding-as-pure-dataflow model. |

**FunctionDef**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | FunctionId |
| `name` | `string \| null` | Function name (null for lambdas) |
| `params` | `string[]` | Parameter names |
| `body_block` | `number` | BlockId of the function body |
| `capture_names` | `string[]` | Names of captured variables |
| `capture_registers` | `number[]` | Which body registers receive captured values (parallel to `capture_names`) |
| `self_ref_register` | `number \| null` | Body register for self-reference (enables recursion) |
| `register_count` | `number` | Total registers for function frame |

**MatchArmMeta**:

| Field | Type | Description |
|-------|------|-------------|
| `pattern` | `Pattern` | The pattern to match (same AST Pattern type) |
| `guard_block` | `number \| null` | BlockId for guard expression, if any |
| `body_block` | `number` | BlockId for the arm body |

**ConstantValue**: `"Nil"`, `{"Bool": true}`, `{"Int": 42}`, `{"Float": 12345678901234}` (u64 bits), `{"String": "hello"}`

**SourceSpan**: `{"start": SourcePosition, "end": SourcePosition}`

**SourcePosition**: `{"line": number, "column": number, "offset": number}`

## Dataflow query commands

The remaining commands query the compiled dataflow graph without running
the program (except `explain`, which needs execution for values). They
all accept `--term <name|id>` with the same resolution rules as `explain`.

### `show-provenance` — Backward slice

```
petal show-provenance [--json] --term <name|id> <file.ptl>
```

Returns the set of terms that feed into the target term, along with the
edges connecting them. "What does this value depend on?"

JSON shape: `{root: Term, ancestors: Term[], edges: [{from, to}, ...]}`.

### `show-dependents` — Forward slice

```
petal show-dependents [--json] --term <name|id> <file.ptl>
```

Symmetric to `show-provenance`, but walks forward through the reverse
`inputs` index. "What downstream values does this term influence?".

### `show-slice` — Minimal subgraph for multiple targets

```
petal show-slice [--json] --term <a> [--term <b> ...] <file.ptl>
```

Returns the smallest subgraph that connects one or more target terms back
to their common ancestors. Useful for focused visualizations and for
extracting the "interesting" part of a larger program.

### `show-graph` — Graphviz DOT export

```
petal show-graph [--all] <file.ptl>
petal show-graph [--all] -e '<code>'
```

Emits the dataflow graph in DOT format, ready to pipe into `dot -Tpng`.
By default hides phantom builtin terms; `--all` includes them.

Nodes are colored by role (constants = light blue, state = pink, user
bindings = white) so the output stays readable even on mid-sized programs.

## Builtin Phantom Terms

Every program starts with **74 phantom terms (t0–t73)** in the root block,
one per registered built-in function. These are `Copy` terms with empty
inputs; their `name` field holds the builtin name. The table below reflects
the registration order from `rust/src/builtins/mod.rs`. Registration order
is load-bearing: reordering it would renumber every IR snapshot, so
built-ins can only be appended.

| ID | Name | ID | Name | ID | Name | ID | Name |
|----|------|----|------|----|------|----|------|
| 0  | `print`      | 17 | `contains`  | 34 | `cos`        | 51 | `noise`      |
| 1  | `range`      | 18 | `min`       | 35 | `tan`        | 52 | `noise_seed` |
| 2  | `len`        | 19 | `max`       | 36 | `atan2`      | 53 | `random_int` |
| 3  | `push`       | 20 | `round`     | 37 | `pi`         | 54 | `choose`     |
| 4  | `str`        | 21 | `dual`      | 38 | `clamp`      | 55 | `hsv`        |
| 5  | `abs`        | 22 | `value_of`  | 39 | `lerp`       | 56 | `hsl`        |
| 6  | `sqrt`       | 23 | `deriv_of`  | 40 | `map_range`  | 57 | `color_lerp` |
| 7  | `floor`      | 24 | `sort`      | 41 | `distance`   | 58 | `vec2`       |
| 8  | `ceil`       | 25 | `reverse`   | 42 | `mag`        | 59 | `normalize`  |
| 9  | `float`      | 26 | `join`      | 43 | `pow`        | 60 | `dot`        |
| 10 | `int`        | 27 | `split`     | 44 | `sign`       | 61 | `limit`      |
| 11 | `random`     | 28 | `enumerate` | 45 | `fract`      | 62 | `map`        |
| 12 | `type`       | 29 | `zip`       | 46 | `smoothstep` | 63 | `filter`     |
| 13 | `append`     | 30 | `slice`     | 47 | `radians`    | 64 | `reduce`     |
| 14 | `pop`        | 31 | `flat`      | 48 | `degrees`    | 65 | `forEach`    |
| 15 | `keys`       | 32 | `includes`  | 49 | `exp`        | 66 | `assert`     |
| 16 | `values`     | 33 | `sin`       | 50 | `log`        | 67 | `assert_eq`  |

Appended after the originals: 68 `f64_array`, 69 `get`, 70 `set`, 71 `swap`
(the typed numeric array builtins), then 72 `hsv_deg`, 73 `hsl_deg` (degree
variants of the colour builtins).

`includes` is a JS-compat alias for `contains`. `map`, `filter`, `reduce`,
and `forEach` are declared as natives so name resolution finds them, but
the evaluator dispatches them as intrinsics (they need access to the
evaluator to call their function argument).

User-defined terms start at t74. Phantom terms are **not connected to
the block's linked list** (`block_next`/`block_prev` are `null`, and the
block's `entry` points to the first user term).

Host embeddings (petal-sdl, petal-web, petal-diagram-canvas) register
additional natives before compiling programs. Those natives add more
phantom terms, so the starting ID of user code shifts accordingly. In
show-ir output, everything before the first non-phantom term is
host-provided.

## Traversing the IR

### Walking a block's terms

Each block has an `entry` field pointing to its first term. Follow `block_next` to walk the linked list:

```javascript
function walkBlock(program, blockId) {
  const block = program.blocks.find(b => b.id === blockId);
  const terms = [];
  let tid = block.entry;
  while (tid !== null) {
    const term = program.terms.find(t => t.id === tid);
    terms.push(term);
    tid = term.block_next;
  }
  return terms;
}
```

### Resolving dataflow edges

Each term's `inputs` array contains TermIds. Look up the referenced term to find what value flows in:

```javascript
function getInputTerms(program, term) {
  return term.inputs.map(id => program.terms.find(t => t.id === id));
}
```

### Building the block tree

Blocks form a tree rooted at `root_block`. A block's parent is the term that created it (`parent_term_id`). Function body blocks have `parent_term_id: null` — connect them via `FunctionDef.body_block` and the `MakeClosure` term.

```javascript
function getChildBlocks(program, blockId) {
  return program.blocks.filter(b => {
    if (b.parent_term_id === null) return false;
    const parentTerm = program.terms.find(t => t.id === b.parent_term_id);
    return parentTerm.block_id === blockId;
  });
}
```

### Constant lookup

TermOp values like `{"Constant": 0}` reference the constants table by index:

```javascript
function resolveConstant(program, constantId) {
  return program.constants.values[constantId];
}
```
