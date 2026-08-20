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
```

To execute inline code, use the `-e` flag on a subcommand, e.g. `petal run -e <code>`.

### Commands at a glance

| Command | Purpose |
|---------|---------|
| `run` | Execute a program |
| `check` | Lex + parse + compile + lower only (no execution) |
| `lint` | Normalize source formatting and idioms (`--fix` / `--check`) |
| `lint-fix` | `lint --fix <file>` under its own name |
| `lsp` | Serve the language server over stdio (editors spawn this) |
| `explain` | Run with trace, walk back from a term to its ancestors |
| `pending-report` | Run, then report every live pending resource |
| `show-tokens` | Lexer output |
| `show-ast` | Parser output |
| `show-ir` | Compiled IR (term graph) |
| `show-bytecode` | Bytecode lowering of the IR (see [Architecture.md](dev/Architecture.md) for the backend split) |
| `show-provenance` | Backward dataflow slice from a term |
| `show-dependents` | Forward dataflow slice from a term |
| `show-slice` | Dataflow subgraph for one or more targets |
| `show-graph` | Graphviz DOT-format dataflow graph |

All inspection commands support `--json` for machine-readable output. `run`
additionally supports `--trace` and `--record-trace <path>` to capture a
per-term execution trace.

## Commands

### `run` — Execute a program

```
petal run [--json] [--trace] [--record-trace <path>] [--observe] [--ir] [--no-opt] [--dup-stats] [--trace-pending] <file.ptl>
petal run [--json] [--trace] [--record-trace <path>] [--observe] [--no-opt] [--dup-stats] [--trace-pending] -e '<code>'
```

Runs the program and prints any output to stdout. Exits with code 1 on error.

Flags:

- `--json` — emit errors as structured JSON instead of a human-readable
  message. Shape: `{message, line, column, caused_by[], stack[], phase}`, plus
  an `errors[]` array for front-end failures. See
  [Error phases](#error-phases) for what `phase` can say.
- `--trace` — write per-term execution events to stderr (inputs + result
  + source location) as they happen.
- `--record-trace <path>` — write the full trace buffer to `<path>` as JSON
  after the run completes. Useful for offline analysis and for feeding
  `petal explain`. Environment variable `PETAL_DEBUG=1` enables tracing
  without the flag.
- `--observe` — after the run, dump the last value bound to every named
  variable. Names are function-qualified: a top-level `sel` is `sel`, a `sel`
  inside `fn list_row` is `list_row.sel`, so the two are separate keys rather
  than one shadowing the other. Only function bodies qualify — an `if` arm or a
  loop body does not. One slot per binding, last write wins, so a loop temp
  reports its **final** iteration; use `--record-trace` / `explain` when you
  need the history instead. A binding whose term never executed is absent, not
  null.

  The dump goes to stdout after a blank line and an `Observed values (N):`
  header, one aligned `name = value` line each, sorted by name. With `--json`
  it is a single object instead. It is printed **even when the program fails**
  — the values bound before the error are the point — and in `--json` mode it
  rides on the error object as an `observations` field, keeping stdout a single
  JSON document. A failing run still exits 1.
- `--ir` — load `<file>` as JSON IR (the output of `show-ir --json`) instead
  of source; use `-` to read from stdin, e.g.
  `petal show-ir --json -e '<code>' | petal run --ir -`.
- `--no-opt` — disable optimizations and run the clone-and-alloc baseline
  (no in-place mutation). Environment variable `PETAL_OPT=off` has the same
  effect.
- `--dup-stats` — print value-duplication and heap allocation stats to stderr
  after the run (debug builds / `dup-stats` feature).
- `--trace-pending` — record pending absorptions and print the frame pending
  report to stderr after the run. Environment variable `PETAL_TRACE_PENDING=1`
  also enables it. For the report as the primary output, see
  [`pending-report`](#pending-report--report-live-pending-resources).

### `check` — Validate without running

```
petal check [--json] [--strict] [--ir] <file.ptl>
petal check [--json] [--strict] -e '<code>'
```

Lex, parse, compile, and lower the program to bytecode but do not execute it.
Exits 0 if all of that succeeds, 1 otherwise. Lowering is part of the check on
purpose: a program can compile cleanly and still fail to lower, and `check` is
what CI and editors call, so stopping at compile would report a green build for
a program that aborts on first run.

Flags:

- `--strict` — see below.
- `--ir` — check `<file>` as JSON IR (the output of `show-ir --json`) instead
  of source; use `-` to read from stdin, exactly as
  [`run --ir`](#run--execute-a-program) does. The IR is validated
  structurally, then lowered — so a third-party IR emitter can be
  CI-validated without running its output:
  `emit-my-ir | petal check --json --ir -`. A load failure here comes from
  the IR deserializer rather than the front end and is reported with
  `"phase": "parse"`, matching `run --ir`; a graph that imports but cannot be
  lowered is reported with `"phase": "lower"`.

The compile step runs the optional type checker (see
[Type Annotations](language-guide.md#type-annotations)). Its findings are
**non-fatal warnings** and do not change the exit code: in text mode they print
to stderr with a source caret; with `--json` they appear as a `warnings` array
(`check` still exits 0). Pass `--strict` to make any warning force a non-zero
exit — useful for CI — while `run` and plain `check` stay 0.

That includes calls the program could never resolve — `f(1)` where every `f`
takes two arguments — which `run` reports as a hard error the moment the call
executes. `check` reports them up front, so `--strict` catches them without
running the program.

With `--json`, emits `{"ok": true, "warnings": [...]}` on success (each warning
is `{message, line, column, file}`, where `file` is `null` for the entry file),
or `{message, line, column, phase, errors, ...}` on a hard failure — see
[Error phases](#error-phases).

Faster than `run` when you only care about syntactic validity and type
annotations.

#### Error phases

Every `--json` error object carries a `phase` saying which stage rejected the
program. It is reported by the stage that raised the error, not inferred from
the message text, so it is exact:

| `phase` | Meaning |
| --- | --- |
| `lex` | Tokenizing failed — an unterminated string, an unexpected character, a bad color literal. |
| `parse` | The token stream is not a valid program — a missing `=`, an unclosed construct, an unexpected token. |
| `module` | An `import` could not be resolved, or the imports form a cycle. Lexing and parsing of the entry file already succeeded. |
| `compile` | The program parses but is not well-formed — writing a `var` with `=`, assigning to a binding from an outer function, importing a name a module does not export, inconsistent `export` markers on an overload set. |
| `lower` | The term graph could not be lowered to bytecode. Only `check` reaches this; it is an internal limitation rather than a user error. |
| `runtime` | The program compiled and ran, and failed during execution (`run` only). |

The `message`, `line` and `column` fields are unchanged by this: `message` is
the whole human-readable error and `line`/`column` locate its last diagnostic.
Front-end failures additionally carry `errors`, one entry per diagnostic:

```json
{
  "error": true,
  "phase": "compile",
  "message": "`x` is a `var`; use `set x = ...` to write it",
  "line": 2,
  "column": 1,
  "errors": [
    {
      "message": "`x` is a `var`; use `set x = ...` to write it",
      "line": 2,
      "column": 1,
      "file": null
    }
  ]
}
```

Each entry's `message` has no position suffix and no file prefix; `file` is the
module's display name, or `null` for the entry file. The compiler walks the
whole program before aborting, so a program with several errors reports all of
them here rather than only the last.

### `lint` — Normalize source

```
petal lint <file.ptl>            # report; exit 1 if changes needed
petal lint --fix <file.ptl>      # rewrite the file in place
petal lint --check <file.ptl>    # CI mode: exit 0/1, no output on success
petal lint -e '<code>'           # lint inline code, print result to stdout

petal lint-fix <file.ptl>        # same as 'lint --fix <file.ptl>'
```

`lint-fix` exists because rewriting in place is the common case and `--fix` is
easy to forget. It takes a path only (there is no file to rewrite for `-e`
code), and it makes **no change at all** when the file doesn't parse: the lint
pipeline reports the parse error and exits non-zero with the file untouched.

Two kinds of normalization (see [dev/linter-plan.md](dev/linter-plan.md)):

- **Formatting** — 2-space re-indentation driven by the token stream, plus
  trailing-whitespace trim and a single trailing newline. Only leading/trailing
  whitespace outside tokens is touched, so comments, string contents, and JSX
  text are preserved exactly.
- **Identity casts** — deletes `int(n)` where `n` is already an `int`, and
  likewise `float()` on a float and `str()` on a string. Petal's `/` on two
  ints yields an int, so `int(w / 2)` is a no-op; `int(w * 0.6)` is not, and is
  left alone. Candidates come from the type checker, which infers `any` for
  anything it cannot prove, so an un-annotated parameter or a `var` cell is
  never touched.

The cast rule removes a call, so there is no IR to hold equal. Its correctness
rests on the detection rule — `int()` on an `int` is the identity — with a
compile gate behind it: if the original source compiles here, the rewritten
source must too, or `lint` refuses to produce any output.

Parentheses follow the slot. `let m = int(a + 1)` becomes `let m = a + 1`;
`2 * int(a + 1)` becomes `2 * (a + 1)`; and a list or argument element
(`f(int(a + 1), b)`) becomes `f(a + 1, b)` — commas are required between
elements, so nothing can bind across the boundary once the parens are gone.

There is deliberately **no** rebind rule: an earlier version rewrote `x = f(x)`
to `f(@x)`. The `@` operator is still part of the language, but it reads as
sugar you have to learn, so the linter no longer pushes code into it.

### `lsp` — Serve the language server

```
petal lsp                        # speaks LSP on stdin/stdout; takes no file
```

Runs the Petal language server over the standard LSP stdio transport
(Content-Length-framed JSON-RPC). It takes no source file — documents arrive
over the protocol via `textDocument/didOpen` / `didChange`. Editors and IDEs
spawn this as a child process; it is not meant to be run interactively.

Capabilities: full-text document sync, diagnostics (published on open and
change), hover, go-to-definition, and completion (triggered on `.`, otherwise
prefix-filtered over the document's definitions plus the keyword list).

The server core lives in `rust/src/lsp/` and is transport-agnostic
(`Server::handle_message` takes a raw JSON-RPC string and returns the outgoing
messages), so an embedder can drive it in-process without the stdio loop.

The loop exits on an `exit` notification or at EOF; a broken pipe — the usual
way an editor shuts a server down — exits quietly.

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

### `pending-report` — Report live pending resources

```
petal pending-report [--json] <file.ptl>
petal pending-report [--json] -e '<code>'
```

Runs the program (with pending-absorption tracking enabled), then reports
every resource value that is still pending or loading after the run — its
state, age in frames, absorption count, and origin call site. Answers the
question "why is this region blank?".

**Text output** (default) — one line per live resource, or
`No pending resources.` when there are none.

With `--json`, emits the raw report array:
`[{id, key, state, age_frames, origin, absorbed_count}, ...]`. This is what
the MCP `PendingReport` tool wraps.

### `show-tokens` — Lexer token stream

```
petal show-tokens <file.ptl>
petal show-tokens -e '<code>'
petal show-tokens --json <file.ptl>
petal show-tokens --json -e '<code>'
```

Outputs the flat token stream produced by the lexer, with source spans. Useful for debugging tokenization and verifying operator/keyword recognition.

**Text output** (default) — one token per line: index, kind, quoted value
(when the token carries one), and compact span:

```
0: Let @1:1-1:4
1: Ident "x" @1:5-1:6
2: Assign @1:7-1:8
3: Int 1 @1:9-1:10
4: Plus @1:11-1:12
5: Int 2 @1:13-1:14
6: Eof @1:14-1:14
```

**JSON output** (`--json`) — array of uniform rows:

```json
[
  {"kind": "Let", "span": [1, 1, 1, 4]},
  {"kind": "Ident", "value": "x", "span": [1, 5, 1, 6]},
  {"kind": "Assign", "span": [1, 7, 1, 8]},
  {"kind": "Int", "value": 1, "span": [1, 9, 1, 10]},
  {"kind": "Plus", "span": [1, 11, 1, 12]},
  {"kind": "Int", "value": 2, "span": [1, 13, 1, 14]},
  {"kind": "Eof", "span": [1, 14, 1, 14]}
]
```

#### Token JSON Encoding

Every row has the same shape — `{"kind", "value"?, "span"}`:

- `kind` — the token's variant name (see the table below).
- `value` — present only on value-carrying tokens. `Int`/`Float` values are
  JSON numbers; `String`/`Ident` (and `JsxTagName`/`JsxText`/`Color`) values
  are JSON strings. Unit tokens omit the field entirely.
- `span` — `[startLine, startCol, endLine, endCol]`, 1-based, end-exclusive
  (`end` points one past the last character). The text form prints the same
  span as `@startLine:startCol-endLine:endCol`.

Kind names:

| Category | Examples |
|----------|---------|
| Keywords | `"Let"`, `"Var"`, `"Set"`, `"Fn"`, `"If"`, `"Else"`, `"For"`, `"In"`, `"While"`, `"Match"`, `"Return"`, `"Break"`, `"Continue"`, `"State"`, `"Enum"`, `"True"`, `"False"`, `"Nil"` |
| Operators | `"Plus"`, `"Minus"`, `"Star"`, `"Slash"`, `"Percent"`, `"PlusPlus"`, `"Eq"`, `"Ne"`, `"Lt"`, `"Le"`, `"Gt"`, `"Ge"`, `"And"`, `"Or"`, `"Bang"`, `"Assign"`, `"Pipe"` |
| Delimiters | `"LParen"`, `"RParen"`, `"LBrace"`, `"RBrace"`, `"LBracket"`, `"RBracket"`, `"Comma"`, `"Dot"`, `"Colon"`, `"Arrow"`, `"DotDot"` |
| Special | `"Newline"`, `"Eof"` |
| Value-carrying | `{"kind": "Int", "value": 42, …}`, `{"kind": "Float", "value": 3.14, …}`, `{"kind": "String", "value": "hello", …}`, `{"kind": "Ident", "value": "myVar", …}` |

### `show-ast` — Parsed AST

```
petal show-ast <file.ptl>
petal show-ast -e '<code>'
petal show-ast --json <file.ptl>
petal show-ast --json -e '<code>'
```

Outputs the parsed abstract syntax tree — an array of `Stmt` nodes. Useful for verifying parser behavior and understanding the tree structure before compilation.

**Text output** (default) — a compact tree, one node per line: the node kind
plus its key facts inline (names, operators, literal values), children
indented two spaces, spans as `@line:col-line:col` (end-exclusive, collapsed
to `@line:col` for single-character spans). Patterns and type annotations are
rendered source-like; default facts (`exported: false`, `is_var: false`, an
absent type annotation) are elided, and modifiers show as words after the
kind (`Let var c`, `FnDecl export f`). For this source:

```petal ignore
fn square(x: number) -> number
  x * x
end

let m = match n
  when 0 -> "zero"
  when k if k > 1 -> "big"
end
```

the dump is:

```
FnDecl square (x: number) -> number @1:1-3:4
  Expr @2:3-2:8
    BinaryOp Mul @2:3-2:8
      Ident x @2:3
      Ident x @2:7
Let m @5:1-8:4
  Match @5:9-8:4
    Ident n @5:15
    Arm 0
      Literal "zero" @6:13-6:19
    Arm k
      Guard
        BinaryOp Gt @7:13-7:18
          Ident k @7:13
          Literal 1 @7:17
      Literal "big" @7:22-7:27
```

Structural sub-parts that are not themselves nodes get label lines without
spans: `Then`/`Else` under an `If`, `Arm <pattern>` and `Guard` under a
`Match`, `Key` under an explicit-key `State`, `Part`/`Prop`/`Text` inside
string interpolations and elements, `Field`/`Spread` inside records. This
form is a debug view and may change; `--json` is the stable
machine-readable output.

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
`Expr` are serialized as `{kind: <variant>, span: SourceSpan}` — `SourceSpan`
is the same compact array encoding the IR uses
(`[startLine, startCol, startOffset, endLine, endCol, endOffset, file?]`, see
[Program JSON Schema](#program-json-schema)), and the `<variant>` shapes are
listed in the `StmtKind` and `ExprKind` tables below.
The canonical definitions live in `rust/src/ast.rs`; the tables below cover
the common variants but are not exhaustive.

**StmtKind** (top-level statements):

| Variant | Shape |
|---------|-------|
| `Let` | `{"Let": {"name": string, "ty": TypeAnn \| null, "value": Expr, "is_var": bool}}` — `ty` is the optional declared type annotation; `is_var` is true for `var x = …` |
| `Assign` | `{"Assign": {"target": AssignTarget, "value": Expr}}` |
| `Expr` | `{"Expr": Expr}` |
| `FnDecl` | `{"FnDecl": {"name": string, "class": string \| null, "params": Param[], "ret": TypeAnn \| null, "body": Stmt[]}}` — `ret` is the optional declared return-type annotation; `class` is set for a method declaration (`fn Rect.center_x(…)` → `class: "Rect"`, `name: "Rect.center_x"`, receiver as `params[0]`) |
| `EnumDecl` | `{"EnumDecl": {"name": string, "variants": EnumVariant[]}}` |
| `ClassDecl` | `{"ClassDecl": {"name": string, "fields": ClassField[]}}` — a `class Name … end` declaration; fields are in declaration (and constructor-argument) order |
| `For` | `{"For": {"var": string, "iter": Expr, "body": Stmt[]}}` |
| `While` | `{"While": {"condition": Expr, "body": Stmt[]}}` |
| `Return` | `{"Return": Expr \| null}` |
| `Break` | `"Break"` |
| `Continue` | `"Continue"` |
| `State` | `{"State": {"name": string, "ty": TypeAnn \| null, "init": Expr, "id": number, "key": Expr \| null, "is_var": bool}}` — `ty` is the optional declared type annotation; `key` set when the source uses the `state(expr) name = init` per-iteration form; `is_var` for `state var` |

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
| `Lambda` | `{"Lambda": {"params": Param[], "body": Stmt[]}}` |
| `StringInterp` | `{"StringInterp": {"parts": string[], "exprs": Expr[]}}` — `parts` has one more element than `exprs` |
| `Element` | `{"Element": {"tag": string, "props": [string, Expr][], "children": JsxChild[]}}` |

**Param**: `{"name": string, "ty": TypeAnn | null}` — a function/lambda parameter with its optional declared type annotation.

**ClassField**: one field of a `ClassDecl`, as `{"name": string, "ty": TypeAnn | null}` — the same optional-annotation shape as a **Param**.

**TypeAnn**: a written type annotation as an object `{"name": string, "resolved": Type | null}` — `name` is the type name exactly as written in the source (`"int"`, `"str"`, `"banana"`), and `resolved` is the recognized static type (or `null` for an unrecognized name, e.g. `{"name": "banana", "resolved": null}`). An absent annotation is `null` (not an object). Annotations appear on `Let`, `State`, `Param`, `ClassField`, and `FnDecl.ret`; a class name resolves to a static type too, but only against the compilation's class table, so a `TypeAnn` naming one carries `resolved: null` in the AST dump; they are type-checked (warnings only — see [`check`](#check--validate-without-running)) and dropped before codegen, so they never appear in the IR.

**Type**: a string naming the recognized static type — one of `"Any"`, `"Nil"`, `"Bool"`, `"Int"`, `"Float"`, `"String"`, `"List"`, `"Record"`, `"Function"`, `"Enum"`, `"Vec2"`, `"F64Array"`, `"Element"`, `"Symbol"`, `"Dual"`, `"Handle"`, `"Pending"`. Appears as the `resolved` field of a **TypeAnn**.

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
petal show-ir [--json] [--all] [--user-only] <file.ptl>
petal show-ir [--json] [--all] [--user-only] -e '<code>'
```

Outputs the compiled intermediate representation — the term graph that the evaluator executes. This is the primary command for GUI and tooling integration.

By default the **text output hides compiler noise**: builtin "phantom" terms
(one per registered native function, see below) and everything that belongs
to a non-entry file — the auto-loaded `std` prelude and imported modules.
Ids are never renumbered by hiding; the lines are simply omitted. Pass
`--all` to include everything.

**Text output** (default). For this program:

```petal
fn double(x)
  x * 2
end
let msg = match double(4)
  when 0 -> "zero"
  when n if n > 5 -> "big"
  when _ -> "small"
end
print(msg)
```

`show-ir` prints:

```
=== Constants ===
  c0: 2
  c1: 4
  c2: "zero"
  c3: 5
  c4: "big"
  c5: "small"
  c6: "print"

=== Functions ===
  fn0: double params=["x"] body=block1 captures=[]

=== Blocks ===
block0 [root] regs=124
  t122 r117 = MakeClosure(fn0) [] ; double @1:1
  t123 r118 = Copy [t122] @4:17
  t124 r119 = Constant(4) [] @4:24
  t125 r120 = Call [t123, t124] @4:17
  t134 r121 = Match [t125] -> block2, block3, block5 ; msg @4:11
    arm0: when 0 -> block2
    arm1: when n if block4 -> block3
    arm2: when _ -> block5
  t135 r122 = Copy [t134] @9:7
  t136 r123 = BuiltinCall("print") [t135] @9:1

block2 (parent: t134) regs=1
  t126 r0 = Constant("zero") [] @5:13

block4 (guard for t134 arm1) regs=4
  binds: n=t127:r0
  t128 r1 = Copy [t127] @6:13
  t129 r2 = Constant(5) [] @6:17
  t130 r3 = Gt [t128, t129] @6:13

...

block1 (body of fn0 double) regs=5
  params: x=t117:r0  self: double=t118:r1
  t119 r2 = Copy [t117] @2:3
  t120 r3 = Constant(2) [] @2:7
  t121 r4 = Mul [t119, t120] @2:3
```

Each term line: `t{id} r{register} = {op} [{inputs}] -> {child_blocks} ; {name} @{line}:{col}`

The text form is designed to be read without cross-referencing:

- **Constants are resolved inline** in ops, following the bytecode
  disassembler's convention: `Constant(1)`, `Constant("zero")`,
  `BuiltinCall("print")`, `Error("message")`, `GetField(.x)` /
  `GetFieldOpt(.x)` / `SetField(.x)`, `MethodCall(.dist2)` (plus
  `, hint=Rect` when a class hint is present), `MakeEnumVariant(Circle)`,
  `AllocMap{x, y}` (plus ` class=Rect` for class constructors), and
  `AllocElement(div, props=[width])`.
- **Match arms** print under their `Match` term, one line per arm, with the
  pattern rendered source-like (`0`, `n`, `_`, `[a, ...rest]`, `{x: xx}`,
  `Circle(r)`), the guard block if any (`if block4`), and the body block.
  Guard blocks are labeled in their own header: `(guard for t134 arm1)`.
- **Blocks are listed as a tree**: the root block first, then its descendant
  child blocks depth-first in term order (a match arm's guard block just
  before its body); each function's body-block tree follows as its own
  top-level section, labeled `(body of fn0 double)`.
- **Block headers name their bindings** — params, captures, the self
  reference, and match-pattern variables — with term id and register
  (`params: x=t117:r0  self: double=t118:r1`, `binds: n=t127:r0`). These
  binding terms are hidden phantoms, so the header is what ties `Copy [t117]`
  to the `x` parameter. A reference to any *other* hidden term is annotated
  inline with its name: `Copy [t162(std::sum)]`.
- **State ops show their identity**: `StateInit(count, key=0x5dff…)` /
  `StateRead(...)` / `StateWrite(...)` carry the state's name and key, so a
  write visibly links to its init.
- **Phi carry-outs** print as a block footer: `phi-out: t126 -> t120 (x)`
  means "when this block's frame pops, copy `t126`'s value into `t120`'s
  register" (the `Phi` term for `x` in the parent block; see `Block.phi_outs`).
- Every term line with a source span ends with a compact `@line:col`
  location (prefixed with the file's display name for non-entry files, e.g.
  `@std:21:1`).

**JSON output** (`--json`) — the full `Program` object, always complete
(phantoms and prelude included): this is the interchange format that
`run --ir` / `check --ir` load, so it is never filtered by default.

```json
{
  "schema": "0.2",
  "id": 0,
  "source": "...",
  "terms": [...],
  "blocks": [...],
  "root_block": 0,
  "constants": {"values": [...]},
  "source_map": {"term_spans": {...}},
  "functions": [...],
  "match_arms": {...}
}
```

**Filtered JSON view** (`--json --user-only`) — the same Program JSON shape
with the noise filtered out of the arrays: builtin phantom terms, prelude /
imported-module terms, blocks, and functions, and constants referenced only
by them. Ids are preserved as-is (nothing is renumbered), which is why
`constants.values` becomes an **id-keyed object** here
(`{"0": {"Int": 2}, "6": {"String": "print"}}`) instead of an array;
`source_map.term_spans` and `match_arms` are filtered to the kept terms. The
param/capture/self binding phantoms of user blocks are kept so no kept term
references a missing id. This is a debugging **view**, not an interchange
format — it is **not loadable** by `run --ir`. (The MCP `ShowIR` tool
returns this view by default.)

#### Program JSON Schema

The IR JSON is the complete compiled `Program` struct — **schema 0.2** (see
[docs/dev/ir-as-target.md](dev/ir-as-target.md) for the full emit-target
contract and the legacy-v0 tolerance rules). All ID newtypes serialize as
their inner integer (e.g. `TermId(5)` becomes `5`). **Defaults are omitted on
the wire**: any field whose value is its default — a `null` option, an empty
array/string/map, a `false` boolean — is simply absent, and loaders treat
absence as the default.

**Top-level Program**:

| Field | Type | Description |
|-------|------|-------------|
| `schema` | `string` | Wire-format version, `"0.2"`. The loader also accepts documents with no `schema` field (legacy v0 shapes) |
| `id` | `number` | Program ID (always 0 for CLI) |
| `source` | `string` | Original source code (omitted when empty) |
| `terms` | `Term[]` | All terms in the program |
| `blocks` | `Block[]` | All blocks in the program |
| `root_block` | `number` | BlockId of the root/entry block |
| `constants` | `{"values": ConstantValue[]}` | Constant table |
| `source_map` | `{"term_spans": {}}` | TermId → SourceSpan mapping (string keys); omitted when empty |
| `has_errors` | `boolean` | Whether the program has parse errors (omitted when `false`) |
| `functions` | `FunctionDef[]` | All function definitions (omitted when empty) |
| `match_arms` | `{[termId: string]: MatchArmMeta[]}` | Match term → arm metadata (string keys); omitted when empty |

**Term**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Unique term ID |
| `op` | `TermOp` | The operation (see TermOp table below) |
| `inputs` | `number[]` | TermIds of input dataflow edges (omitted when empty) |
| `block_id` | `number` | BlockId this term belongs to |
| `name` | `string` | Variable name if this is a binding (omitted when none) |
| `register` | `number` | Register index for evaluation. Optional for imports — when any term omits it, the loader recomputes the whole assignment |
| `state_key` | `number` | State key for StateInit/StateRead/StateWrite (omitted otherwise) |
| `child_blocks` | `number[]` | BlockIds of child blocks, for control flow (omitted when empty) |
| `in_loop` | `boolean` | Omitted when `false`. Marks state terms inside a loop body for per-iteration state. |
| `collect` | `boolean` | Omitted when `false`. On a `ForLoop`/`NumericForLoop` term, marks a value-position loop (`x = for …`) that collects each iteration's body result into a list. |

Execution order within a block is the block's ordered `terms` array (below).
The in-memory `entry`/`block_next`/`block_prev` linked list is not serialized;
legacy documents that carry it (and no block `terms` arrays) still load.

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
| Coalesce | `"Coalesce"` | [left] | [rhs_block] | `??` — yields the RHS when the left is Nil/Pending |
| Concat | `"Concat"` | [left, right] | none | String concatenation (`++`) |
| Copy | `"Copy"` | [source] or [] | none | Variable reference. No inputs + a `name` = binding phantom (builtin/param/capture/self) — never listed in a block's `terms` array |
| Phi | `"Phi"` | [init] | none | Pure-dataflow join for names rebound inside child blocks. Sits in the parent block before the control-flow term; child frames overwrite via `Block.phi_outs`. |
| Branch | `"Branch"` | [condition] | [then_block, else_block] | if/else |
| ForLoop | `"ForLoop"` | [iterable] | [body_block] | for-in loop. Yields a list of each iteration's body result when `collect` is set (value-position `x = for …`). |
| NumericForLoop | `"NumericForLoop"` | [start, end] | [body_block] | non-allocating `for x in range(a, b)` integer loop. Collects like `ForLoop` when `collect` is set. |
| WhileLoop | `"WhileLoop"` | none | [cond_block, body_block] | while loop |
| Break | `"Break"` | none | none | |
| Continue | `"Continue"` | none | none | |
| Return | `"Return"` | [value] or [] | none | |
| MakeClosure | `{"MakeClosure": fid}` | [captured_values...] | none | Create closure for FunctionId |
| MakeOverloadSet | `"MakeOverloadSet"` | [closure0, closure1, ...] | none | Bundle arity-overloaded closures. See [function-overloading.md](function-overloading.md). |
| Call | `"Call"` | [callable, arg0, arg1, ...] | none | |
| MethodCall | `{"MethodCall": {"name": cid, "hint": cid?}}` | [object, arg0, arg1, ...] | none | Method name as ConstantId; tries record field first, then the receiver's class, then a builtin with `object` prepended. `hint` (omitted when absent) names a class for live-edit dispatch. |
| BuiltinCall | `{"BuiltinCall": cid}` | [arg0, arg1, ...] | none | Direct builtin call; `cid` is a String constant holding the builtin's name, resolved by name at lower time. |
| StateInit | `"StateInit"` | [] or [explicit_key] | [init_block] | `state_key` set. Init expression lives in `child_blocks[0]` for lazy evaluation — only entered when the runtime key isn't yet in the persistent state map. Optional `explicit_key` is the value computed for `state(expr) name`. |
| StateRead | `"StateRead"` | none | none | `state_key` set |
| StateWrite | `"StateWrite"` | [value] or [value, explicit_key] | none | `state_key` set. Forwards the same `explicit_key` from the matching `StateInit` so the runtime resolves to the same `RuntimeStateKey`. |
| CellNew | `"CellNew"` | [init] | none | Allocate the cell behind a `var`. The one op that produces a `Value::Cell`. |
| CellRead | `"CellRead"` | [cell] | none | Dereference a cell — every source-level read of a `var`. |
| CellWrite | `"CellWrite"` | [cell, value] | none | Write through a cell (`set x = …`). Yields the written value. |
| AllocList | `"AllocList"` | [elem0, elem1, ...] | none | |
| AllocMap | `{"AllocMap": {"fields": [cid, ...], "class": cid?}}` | [val0, val1, ...] | none | Field names as ConstantIds. `class` is present only for a class constructor's allocation, naming the class the record is tagged with (see [language-guide.md](language-guide.md#classes--methods)); it is omitted for a plain record literal. |
| AllocMapSpread | `{"AllocMapSpread": {"entries": [...]}}` | [spread_src..., named_value...] | none | Record literal with `...spread`. Each entry is `Spread(idx)` or `Named{key, idx}` referencing positions in `inputs`. |
| GetField | `{"GetField": cid}` | [object] | none | |
| GetFieldOpt | `{"GetFieldOpt": cid}` | [object] | none | Tolerant read: missing field / Nil object yields Nil (left side of `??`) |
| SetField | `{"SetField": cid}` | [object, value] | none | |
| GetIndex | `"GetIndex"` | [object, index] | none | |
| GetIndexOpt | `"GetIndexOpt"` | [object, index] | none | Tolerant read: missing key / Nil object yields Nil |
| SetIndex | `"SetIndex"` | [object, index, value] | none | |
| AllocElement | `{"AllocElement": {"tag": cid, "prop_keys": [cid, ...]}}` | [prop_val0, ..., child0, ...] | none | JSX-like element. `prop_keys.len()` separates prop values from children in `inputs`. |
| MakeEnumVariant | `{"MakeEnumVariant": cid}` | [field_values...] | none | Variant name as ConstantId |
| Match | `"Match"` | [subject] | [arm_body_blocks...] | Arm metadata in `match_arms` |

**Block**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Unique block ID |
| `parent_term_id` | `number` | TermId that created this block (omitted for root and function bodies) |
| `terms` | `number[]` | The block's TermIds in execution order (omitted when empty). Binding phantoms are not listed. |
| `param_names` | `string[]` | Parameter names (function params, for-loop variable); omitted when empty |
| `register_count` | `number` | Total registers needed for this block's frame. Optional for imports — recomputed/filled by the loader |
| `phi_outs` | `PhiOut[]` | Carry-outs: when this block's frame pops, copy each `src_term`'s register into the parent block's `Phi` term register. Drives the rebinding-as-pure-dataflow model. Omitted when empty. |

**FunctionDef**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | FunctionId |
| `name` | `string` | Function name (omitted for lambdas) |
| `params` | `string[]` | Parameter names (omitted when empty) |
| `body_block` | `number` | BlockId of the function body |
| `capture_names` | `string[]` | Names of captured variables (omitted when empty) |
| `capture_registers` | `number[]` | Which body registers receive captured values (parallel to `capture_names`). Optional for imports — re-derived from the body block's binding phantoms when registers are omitted |
| `self_ref_register` | `number` | Body register for self-reference (enables recursion). Optional for imports, like `capture_registers` |
| `register_count` | `number` | Total registers for function frame. Optional for imports |

**MatchArmMeta**:

| Field | Type | Description |
|-------|------|-------------|
| `pattern` | `Pattern` | The pattern to match (same AST Pattern type) |
| `guard_block` | `number` | BlockId for guard expression (omitted when none) |
| `body_block` | `number` | BlockId for the arm body |

**ConstantValue**: `"Nil"`, `{"Bool": true}`, `{"Int": 42}`, `{"Float": 12345678901234}` (u64 bits), `{"String": "hello"}`

**SourceSpan** — a compact lossless array, shared by the IR `source_map` and
the AST JSON:
`[startLine, startCol, startOffset, endLine, endCol, endOffset]`, with a
seventh element (the `source_map.files` index) appended only when nonzero
(non-entry file). Lines and columns are 1-based, offsets 0-based bytes. On
input, the loader also accepts the legacy object form
`{"start": {"line", "column", "offset"}, "end": {…}, "file"?}`.

### `show-bytecode` — Bytecode lowering

```
petal show-bytecode [--json] <file.ptl>
petal show-bytecode [--json] -e '<code>'
```

Displays the bytecode the VM executes: the compiled IR lowered — through the
same optimization pipeline a `run` uses — to linear, register-based
instructions. One section per function; the program's top level is the
implicit root function. See [Architecture.md](dev/Architecture.md) for the
backend split.

For `let xs = map([1, 2, 3], fn(x) x * 2 end)`:

```
fn <root>  (122 regs, 0 loop slots)
     0  r116 = const 1
     1  r117 = const 2
     2  r118 = const 3
     3  r119 = list [r116, r117, r118]
     4  r120 = closure f0 caps=[]
     5  r121 = builtin "map" [r119, r120]

fn f0  (4 regs, 0 loop slots)
  params:   r0
     0  r2 = const 2
     1  r3 = r0 * r2
```

Text-form conventions: `rN` is a register in the function's flat register
file, `fN` a FunctionId, `slotN` a loop-cursor slot, `kN` a state key.
Constant-table operands are resolved inline (`const 1`, `builtin "map"`).
Jump targets (`jump -> 5`) are instruction indexes within the same function —
the left-hand column.

#### Bytecode JSON Encoding

`--json` emits `{"functions": [Function]}`, root function first.

**Function**:

| Field | Type | Description |
|-------|------|-------------|
| `fn` | `number \| null` | FunctionId (null for the implicit root function) |
| `name` | `string \| null` | Function name (null for the root and for lambdas) |
| `reg_count` | `number` | Size of the function's flat register file |
| `loop_slots` | `number` | Number of loop-cursor slots the function needs |
| `param_regs` | `number[]` | Registers that receive positional parameters, in order |
| `capture_regs` | `number[]` | Registers that receive captured values, in capture order |
| `self_ref_reg` | `number \| null` | Register holding the self-reference (recursion), if any |
| `code` | `InstRow[]` | The instruction stream |

**InstRow** — each instruction appears twice, structured for tooling and
rendered for reading:

| Field | Type | Description |
|-------|------|-------------|
| `ip` | `number` | Instruction index within this function (what jump targets refer to) |
| `inst` | `object` | Structured instruction: `{"<Opcode>": {operands}}` |
| `text` | `string` | The disassembled text form of the same instruction |

`inst` is the externally tagged encoding of the instruction enum — one key,
the opcode name, mapping to an object of named operands (the same convention
as a term's `op` in the [IR JSON](#program-json-schema)):

```json
{
  "ip": 5,
  "inst": { "BuiltinCall": { "dst": 121, "name": 3, "args": [119, 120], "in_place": false } },
  "text": "r121 = builtin \"map\" [r119, r120]"
}
```

Operand encoding:

- Registers are plain numbers (`dst`, `a`, `b`, `src`, `args`, …).
- Constant-table operands (`k`, `name`, `field`, `tag`, `class`, `msg`, …)
  are ConstantId indexes into the program's constant table — the same table
  `show-ir --json` emits as `constants`. The `text` form resolves them inline.
- Jump-target operands (`to`, `exit`, `next`, `after`) are `ip` indexes
  within the same function.
- Function operands (`func`) are FunctionIds; state operands (`base`) are
  state keys.
- Optional operands are present as `null` rather than omitted, so each opcode
  has a fixed field set.

The full instruction set is documented in `rust/src/backend/bytecode/isa.rs`.

## Dataflow query commands

The remaining commands query the compiled dataflow graph without running
the program (except `explain`, which needs execution for values). They
all accept `--term <name|id>` with the same resolution rules as `explain`.

All three stop at every `var` cell. See
[Cells and the frontier](#cells-and-the-frontier) below — the short version is
that none of them promises an unqualified "minimal" answer on a program that
uses `var`, and each one says so in its output.

### `show-provenance` — Backward slice

```
petal show-provenance [--json] --term <name|id> <file.ptl>
```

Returns the set of terms that feed into the target term, along with the
edges connecting them. "What does this value depend on?"

JSON shape:
`{root: Term, ancestors: Term[], edges: [{from, to, kind}, ...], frontier: [...], complete: bool}`.
`kind` is always `"dataflow"` here — a backward walk answers a *must* question,
so the only edges it will cross are value edges.

### `show-dependents` — Forward slice

```
petal show-dependents [--json] --term <name|id> <file.ptl>
```

Symmetric to `show-provenance`, but walks forward through the reverse
`inputs` index. "What downstream values does this term influence?".

This direction answers a *may* question, so it also carries `"kind": "may"`
edges from a `var` declaration and from every `set` to every read of that
cell (`t96 ~> t97 (cell 'x', may)` in text mode). Which write actually
supplied a given read is a dynamic fact; over-approximating it is correct,
while under-approximating it would mean reporting that a `set` affects
nothing.

### `show-slice` — Subgraph for multiple targets

```
petal show-slice [--json] --term <a> [--term <b> ...] <file.ptl>
```

Returns a subgraph that connects one or more target terms back to their
common ancestors. Useful for focused visualizations and for extracting the
"interesting" part of a larger program.

On a cell-free program this is the smallest such subgraph. On a program that
reads a `var` it is deliberately **not** minimal: it also pulls in the
declaration and every `set` site, transitively, and reports
`"minimal": false` with a `Not minimal — N cell reads crossed:` block. Too
small silently computes a *different value*; too big only loses precision.
Even the conservative answer is sufficient in *terms*, not faithful in
*order* — a dataflow slice never carried the control flow that selects among
the writes.

### Cells and the frontier

A `var` binds a mutable heap cell (see
[`var` and `set`](language-guide.md#var-and-set); the design rationale is in
`docs/dev/var-next-steps.md`, Cells). The cell operand of a `CellRead`,
a `CellWrite` or a closure capture is an *identity* edge — it names which box,
not which value — so a backward walk never crosses one. Every stop is reported
as a **frontier** entry naming the var, its declaration, and the complete set
of writes that could have supplied the value:

```
Frontier (1):
  t97: read of var 'x' (not traced)
    possible write: t96 [line 2, column 1]
```

The write set is complete because no expression evaluates to a cell (§6d), so
the only way to reach one is a name lexically bound to its declaration. The
exception is a `state var`, whose slot the host can also write through
`set_state`; that is printed rather than glossed.

`show-*` commands never run the program, so they always degrade to this
static answer (`"resolution": "not_traced"`) — never to silence. `explain`
does run it, and therefore resolves the boundary to the exact write and
continues the chain through it.

Note that `--term x` on a `var` resolves to the *last* `CellWrite` on that
name, since term lookup is last-name-wins. That is no longer worth special
casing: `explain` from the write shows that write's own chain and the var
header, and `show-dependents` from it shows every read the write may reach.
Pass an explicit `--term tNN` to start somewhere else.

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

Every **compiled** program starts with one phantom term per registered
built-in function (`t0`, `t1`, …) in the root block. These are `Copy` terms
with no inputs; their `name` field holds the builtin name. The IDs follow
the registration order in `rust/src/builtins/mod.rs`. Registration order is
load-bearing for compiled snapshots: reordering it would renumber every IR
snapshot, so built-ins can only be appended.

The phantoms exist for name resolution and for using a builtin as a
first-class *value*; a direct call like `print(x)` compiles to a
`BuiltinCall` naming the builtin via a string constant instead. The runtime
seeds native function values into phantom registers **by name** when the
root frame is pushed — so an imported IR document (schema 0.2) needs no
phantoms at all unless it wants a builtin as a value, and never depends on
registration order. See [docs/dev/ir-as-target.md](dev/ir-as-target.md).

`includes` is a JS-compat alias for `contains`. `map`, `filter`, `reduce`,
and `forEach` are declared as natives so name resolution finds them, but
the evaluator dispatches them as intrinsics (they need access to the
evaluator to call their function argument).

User-defined terms are numbered after the phantom terms (the first user
term's ID is the number of registered builtins). Phantom terms are **not
listed** in any block's `terms` array — they don't execute.

Host embeddings (petal-sdl, petal-web, petal-diagram-canvas) register
additional natives before compiling programs. Those natives add more
phantom terms, so the starting ID of user code shifts accordingly. In
`show-ir --json` output (and `show-ir --all` text), everything before the
first non-phantom term is host-provided; the default text output and the
`--user-only` JSON view hide phantoms entirely.

## Traversing the IR

### Walking a block's terms

Each block carries its TermIds in execution order:

```javascript
function walkBlock(program, blockId) {
  const block = program.blocks.find(b => b.id === blockId);
  return (block.terms ?? []).map(tid => program.terms[tid]); // terms[i].id === i
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
