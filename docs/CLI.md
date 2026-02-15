# Petal CLI Reference

The `petal` binary provides commands for running programs and inspecting the intermediate representations produced by each compiler stage: tokens, AST, and IR.

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

## Commands

### `run` — Execute a program

```
petal run <file.ptl>
petal run -e '<code>'
```

Runs the program and prints any output to stdout. Exits with code 1 on error.

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
| Unit keywords/operators | `"Let"`, `"Fn"`, `"If"`, `"Else"`, `"For"`, `"In"`, `"While"`, `"Match"`, `"Return"`, `"Break"`, `"State"`, `"Enum"`, `"True"`, `"False"`, `"Nil"` |
| Unit operators | `"Plus"`, `"Minus"`, `"Star"`, `"Slash"`, `"Percent"`, `"PlusPlus"`, `"Eq"`, `"Ne"`, `"Lt"`, `"Le"`, `"Gt"`, `"Ge"`, `"And"`, `"Or"`, `"Bang"`, `"Assign"` |
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

All AST types use serde's externally-tagged enum representation.

**Stmt** (top-level statements):

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
| `State` | `{"State": {"name": string, "init": Expr, "id": number}}` |

**Expr** (expressions):

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
| `Record` | `{"Record": [string, Expr][]}` |
| `FieldAccess` | `{"FieldAccess": {"object": Expr, "field": string}}` |
| `IndexAccess` | `{"IndexAccess": {"object": Expr, "index": Expr}}` |
| `Block` | `{"Block": Stmt[]}` |
| `Lambda` | `{"Lambda": {"params": string[], "body": Stmt[]}}` |

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
petal show-ir <file.ptl>
petal show-ir -e '<code>'
petal show-ir --json <file.ptl>
petal show-ir --json -e '<code>'
```

Outputs the compiled intermediate representation — the term graph that the evaluator executes. This is the primary command for GUI playground integration.

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
| Assign | `{"Assign": tid}` | [value] | none | Write to outer scope register; tid = target TermId |
| Branch | `"Branch"` | [condition] | [then_block, else_block] | if/else |
| ForLoop | `"ForLoop"` | [iterable] | [body_block] | for-in loop |
| WhileLoop | `"WhileLoop"` | none | [cond_block, body_block] | while loop |
| Break | `"Break"` | none | none | |
| Return | `"Return"` | [value] or [] | none | |
| MakeClosure | `{"MakeClosure": fid}` | [captured_values...] | none | Create closure for FunctionId |
| Call | `"Call"` | [callable, arg0, arg1, ...] | none | |
| StateInit | `"StateInit"` | [init_value] | none | `state_key` set |
| StateRead | `"StateRead"` | none | none | `state_key` set |
| StateWrite | `"StateWrite"` | [value] | none | `state_key` set |
| AllocList | `"AllocList"` | [elem0, elem1, ...] | none | |
| AllocMap | `{"AllocMap": {"fields": [cid, ...]}}` | [val0, val1, ...] | none | Field names as ConstantIds |
| GetField | `{"GetField": cid}` | [object] | none | |
| SetField | `{"SetField": cid}` | [object, value] | none | |
| GetIndex | `"GetIndex"` | [object, index] | none | |
| SetIndex | `"SetIndex"` | [object, index, value] | none | |
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

## Builtin Phantom Terms

Every program starts with 21 phantom terms (t0–t20) in the root block, one per builtin function. These are `Copy` terms with empty inputs and their `name` set to the builtin name:

| ID | Name | ID | Name | ID | Name |
|----|------|----|------|----|------|
| 0 | `print` | 7 | `floor` | 14 | `pop` |
| 1 | `range` | 8 | `ceil` | 15 | `keys` |
| 2 | `len` | 9 | `float` | 16 | `values` |
| 3 | `push` | 10 | `int` | 17 | `contains` |
| 4 | `str` | 11 | `random` | 18 | `min` |
| 5 | `abs` | 12 | `type` | 19 | `max` |
| 6 | `sqrt` | 13 | `append` | 20 | `round` |

User-defined terms start at t21. Builtin phantom terms are not connected to the block's linked list (`block_next`/`block_prev` are null, and the block's `entry` points to the first user term).

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
