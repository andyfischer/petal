# Petal CLI Reference

The `petal` binary runs programs, checks them without running, tidies source,
dumps each compiler stage (tokens, AST, IR, bytecode), and answers questions
about a program's dataflow.

## Usage

```
petal <command> [options] <file>
petal <command> [options] -e '<code>'
petal <file>                      # same as: petal run <file>
petal help <command>              # the manual page for one command
petal --version
```

### Common options

Every command that compiles a program accepts these:

- `-e '<code>'` — read the program from the command line instead of a file.
- `-I <dir>` — add a module search directory. Repeatable. `import util` also
  looks in the importing file's directory and in the directories listed in
  the `PETAL_PATH` environment variable.

### Commands at a glance

| Command | Purpose |
|---------|---------|
| `run` | Execute a program |
| `check` | Compile without executing |
| `lsp` | Serve the language server over stdio |
| `lint` | Report or apply source normalization |
| `lint-fix` | `lint --fix <file>` under its own name |
| `ir-equal` | Compare two files' compiled IR |
| `show-tokens` | Lexer output |
| `show-ast` | Parser output |
| `show-ir` | Compiled IR (term graph) |
| `show-bytecode` | Bytecode lowering of the IR |
| `show-graph` | Dataflow graph as Graphviz DOT |
| `explain` | Run, then show the value chain that produced a term |
| `show-provenance` | Backward dataflow slice from a term |
| `show-dependents` | Forward dataflow slice from a term |
| `show-slice` | Minimal dataflow subgraph for one or more targets |
| `pending-report` | Run, then report every live pending resource |
| `propose-edit` | Propose source edits that change an emitted value |

Every command except `lsp`, `lint`, `lint-fix` and `show-graph` accepts
`--json` for machine-readable output.

## Commands

### `run` — Execute a program

```
petal run [options] <file.ptl>
petal run [options] -e '<code>'
petal <file.ptl>
```

Compiles the program and runs it. Output goes to stdout. Exits 1 on error.

```
$ petal run -e 'print(1 + 2)'
3
```

Options:

- `--json` — emit errors as structured JSON:
  `{message, line, column, caused_by[], stack[], phase}`, plus an `errors[]`
  array for front-end failures. See [Error phases](#error-phases). With
  `--observe` or `--trace-emits`, those reports are JSON too.
- `--error-format full|bare` — how errors print on stderr. `full` (the
  default) is `Error: <message> [line N, column M]` followed by the source line
  and a caret. `bare` prints only the message, so two sources that differ only
  in layout fail identically (see
  [refactor verification](dev/refactor-verification.md)). `--json` output is
  unaffected.
- `--ir` — load `<file>` as JSON IR (the output of `show-ir --json`) instead of
  source. Use `-` to read from stdin:
  `petal show-ir --json -e 'print(42)' | petal run --ir -`.
- `--seed <n>` — seed the random-number generator, so `random`, `random_int`
  and `choose` replay identically. Decimal or `0x`-hex. Without it the seed
  comes from the clock. `PETAL_SEED=<n>` does the same for every command; the
  flag wins when both are set.
- `--no-opt` — skip the optimizer. Output must be identical either way, so a
  difference is a bug in an optimization pass. `PETAL_OPT=off` does the same.

Tracing and inspection options:

- `--trace` — write per-term execution events to stderr as they happen.
  `PETAL_DEBUG=1` does the same.
- `--record-trace <path>` — write the full execution trace to `<path>` as JSON
  after the run.
- `--observe` — after the run, dump the last value bound to every named
  variable. Names are function-qualified: a `b` inside `fn f` is `f.b`, so it
  does not collide with a top-level `b`. One slot per binding, last write wins,
  so a loop variable reports its final value. A binding that never executed is
  absent.

  ```
  $ petal run --observe -e 'let a = 1
  fn f(y)
    let b = y * 2
    b
  end
  print(f(a))'
  2

  Observed values (3):
    a   = 1
    f   = "<function>"
    f.b = 2
  ```

  The dump is printed even when the run fails, since the values bound before
  the error are the point. With `--json` it is a single object; on a failing
  run it rides on the error object as an `observations` field. A failing run
  still exits 1.
- `--trace-emits` — attribute every buffered emit (`push_output`, draw
  commands) to the call that produced it, and dump the values with their call
  sites and per-argument edit info after the run. This is the observing half of
  [`propose-edit`](#propose-edit--propose-source-edits-that-change-an-emitted-value);
  see [direct-manipulation.md](direct-manipulation.md).
- `--trace-pending` — record pending absorptions and print the frame pending
  report to stderr after the run. `PETAL_TRACE_PENDING=1` does the same. For
  the report as the main output, see
  [`pending-report`](#pending-report--report-live-pending-resources).
- `--profile` — count instructions, builtin calls and collections during the
  run and print the histogram to stderr.
- `--dup-stats` — print value-duplication and heap allocation stats to stderr
  after the run. Debug builds / the `dup-stats` feature only.

### `check` — Compile without running

```
petal check [--json] [--strict] [--ir] [--error-format full|bare] <file.ptl>
petal check [--json] [--strict] [--error-format full|bare] -e '<code>'
```

Lexes, parses, compiles and lowers the program to bytecode, then stops. Exits 0
when all of that succeeds and 1 when it does not. This is the cheap gate for
editors and CI.

Options:

- `--json` — emit errors and warnings as JSON. On success:
  `{"ok": true, "warnings": [...]}`, each warning
  `{message, line, column, file}` (`file` is `null` for the entry file). On a
  hard failure: `{message, line, column, phase, errors, ...}` — see
  [Error phases](#error-phases).
- `--strict` — exit 1 when there are warnings. Plain `check` exits 0 for a
  program that only has warnings.
- `--ir` — check `<file>` as JSON IR instead of source; `-` reads stdin, as
  with `run --ir`. The IR is validated, then lowered, so a third-party IR
  emitter can be checked without running its output:
  `emit-my-ir | petal check --json --ir -`. A load failure is reported with
  `"phase": "parse"`; IR that loads but cannot be lowered gets `"phase": "lower"`.
- `--error-format full|bare` — as on [`run`](#run--execute-a-program).

#### Warnings

Compiling runs the optional type checker (see
[Type Annotations](language-guide.md#type-annotations)). Its findings are
warnings: they print to stderr with a source caret, or appear in the `warnings`
array with `--json`, and do not change the exit code unless `--strict` is set.

```
$ petal check -e 'let x: int = "s"'
warning: type mismatch: `x` declared `int` but assigned `string`
 --> [line 1, column 14]
  |
1 | let x: int = "s"
  |              ^^^
$ echo $?
0
```

The same channel carries a few lints that are not about types: a discarded
pure call (`push(xs, x)` whose result is thrown away), a function that captures
a module `state` rebound below it, a call to a declaration further down the
file that could not be hoisted, and a call no overload could accept (`f(1)`
where every `f` takes two arguments). `run` reports that last one as a hard
error only when the call executes; `check --strict` catches it up front.

#### Error phases

Every `--json` error object carries a `phase` naming the stage that rejected
the program:

| `phase` | Meaning |
| --- | --- |
| `lex` | Tokenizing failed — an unterminated string, an unexpected character, a bad color literal. |
| `parse` | The tokens are not a valid program — a missing `=`, an unclosed construct, an unexpected token. |
| `module` | An `import` could not be resolved, or the imports form a cycle. |
| `compile` | The program parses but is not well-formed — writing a `var` with `=`, assigning to a binding from an outer function, importing a name a module does not export. |
| `lower` | The term graph could not be lowered to bytecode. Only `check` reaches this. |
| `runtime` | The program compiled and ran, and failed during execution (`run` only). |

`message` is the whole human-readable error and `line`/`column` locate its last
diagnostic. Front-end failures also carry `errors`, one entry per diagnostic;
the compiler reports every error it finds, not only the last:

```
$ petal check --json -e 'var x = 1
x = 2'
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

Each entry's `message` has no position suffix; `file` is the module's display
name, or `null` for the entry file.

### `lint` — Normalize source

```
petal lint <file.ptl>                  # report; exit 1 if changes are needed
petal lint --fix <file.ptl>            # rewrite the file in place
petal lint --check <file.ptl>          # CI mode: exit 0/1, no output on success
petal lint -e '<code>'                 # lint inline code, print result to stdout

petal lint --fix --verify <file.ptl>          # prove the rewrite before writing it
petal lint --fix --verify=strict <file.ptl>   # demand full IR equality

petal lint-fix <file.ptl>              # same as: lint --fix <file.ptl>
```

`lint-fix` exists because rewriting in place is the common case and `--fix` is
easy to forget. It takes a single path and no options. If the file does not
parse, it reports the parse error, exits non-zero, and leaves the file
untouched.

```
$ petal lint -e 'fn f(x: int)
let y = int(x)
    y
end'
lint: removed 1 redundant cast(s)
fn f(x: int)
  let y = x
  y
end
```

Three passes (see [dev/linter-plan.md](dev/linter-plan.md)):

- **Formatting** — 2-space re-indentation, trailing-whitespace trim, and a
  single trailing newline. Only whitespace outside tokens is touched, so
  comments, strings and JSX text are preserved exactly.
- **Identity casts** — deletes `int(n)` where `n` is already an `int`, and
  likewise `float()` on a float and `str()` on a string. Candidates come from
  the type checker, which infers `any` for anything it cannot prove, so an
  un-annotated parameter or a `var` is never touched. Parentheses follow the
  slot: `2 * int(a + 1)` becomes `2 * (a + 1)`.
- **`if`-chain to `match`** — rewrites an `if`/`elsif` chain that tests one
  subject against string, bool or nil literals into a `match`.

The last two change tokens, so `lint` checks that the rewritten source still
compiles whenever the original did, and refuses to produce output otherwise.

#### `--verify` — prove the rewrite

`--verify` compiles the original and the rewritten text and compares their IR
(the same comparison as [`ir-equal`](#ir-equal--are-two-files-the-same-program)).
Nothing is written unless the comparison is acceptable; a rewrite that cannot
be accepted exits **3**, distinct from the plain "needs changes" exit 1. It
works with `--fix` and with `--check`.

| Mode | Demands | When a semantic pass fired |
|---|---|---|
| `--verify` (= `--verify=ir`, the default) | The formatting pass must not change the IR | Allowed: reported as an expected IR change, file still written |
| `--verify=strict` | The whole rewrite must be IR-equal | Refused: exit 3, file untouched |

Formatting is the only pass that is meant to leave the IR unchanged. The cast
and `match` passes change it by design. On such a file the default mode proves
the part it can, prints the first difference, and says that a run diff is what
would prove the rest:

```
$ petal lint --fix --verify app.ptl
lint: rewrote 1 if/elsif chain(s) as match
verify: rewrite changed IR (0 cast(s) removed, 1 if-chain(s) rewritten as match);
        formatting alone was proven IR-equal. First difference:
function `label` body: statement count differs (at 2:6)
  original: 4
  rewritten: 2
verify: run-diff verification needed for the semantic passes
```

If formatting alone ever moves the IR, that is a linter bug: `--verify` reports
it as one and refuses to write, whatever the mode.

### `ir-equal` — Are two files the same program?

```
petal ir-equal <a.ptl> <b.ptl>          # exit 0 equal, 1 different, 2 can't tell
petal ir-equal --json <a.ptl> <b.ptl>   # {"equal": bool, "diff"?: {...}}
```

Compiles both files and compares their IR, ignoring everything positional:
spans, comments, whitespace, and the numeric ids of terms, blocks and constants
(constants are compared by value). `<a.ptl>` is the original; reported
differences point at its line and column.

The exit code is a three-way answer: **0** equivalent, **1** different (the
first difference is printed), **2** a side failed to compile. Exit 2 means
"can't tell" and must not be read as "not equal". `--json` mirrors this:
`{"equal": true}`, `{"equal": false, "diff": {location, what, left, right,
line, column}}`, or `{"equal": false, "error": "..."}`.

Two things are deliberately treated as semantic, so a difference in them is
reported:

- **Variable names.** Petal keeps binding names in the IR and hashes them into
  `state` keys, so a rename is a difference.
- **Call structure.** A `state` slot is keyed by the call path that reaches
  it, so extracting a helper, inlining one, moving a call to another function,
  or adding an earlier call to the same callee in the same function all change
  which state slots the callees reach. `ir-equal` reports these as different
  even in a program with no `state` — it compares the IR, not reachability.
  Reformatting, renaming a local that is not a callee, and reordering unrelated
  statements stay IR-equal.

When a refactor is meant to move calls, prove it with a run diff
(`ts/bin/verify.ts`, see [dev/refactor-verification.md](dev/refactor-verification.md))
rather than `ir-equal`.

### `lsp` — Serve the language server

```
petal lsp
```

Speaks the Language Server Protocol over stdio (Content-Length-framed
JSON-RPC). It takes no file and no options: documents arrive over the
protocol. Editors spawn this as a child process.

It provides full-text document sync, diagnostics on open and change, hover,
go-to-definition, and completion. The loop exits on an `exit` notification or
at EOF.

### `explain` — Show the value chain that produced a term

```
petal explain [--json] --term <name|id> <file.ptl>
petal explain [--json] --term <name|id> -e '<code>'
```

Runs the program with tracing on, then walks backward from the target term
and prints every recorded value along the chain of ancestors. It answers "why
does `x` have this value?".

```
$ petal explain --term total -e 'let a = 2
let b = 3
let total = a * b'
Explain t120 (total):
  Provenance chain:
    => t120 total [line 3, column 13] = 6
     . t118 - [line 3, column 13] = 2
     . t119 - [line 3, column 17] = 3
     . t116 a [line 1, column 9] = 2
     . t117 b [line 2, column 9] = 3
```

`--term` accepts a variable name (`--term total`), a bare term id
(`--term 72`), or the `t`-prefixed form (`--term t72`). A name resolves to the
last term bound to it.

With `--json`, returns
`{name, term_id, chain: [{term_id, op, name, value, line, column, boundary}, ...], complete, truncated}`.

### `pending-report` — Report live pending resources

```
petal pending-report [--json] <file.ptl>
petal pending-report [--json] -e '<code>'
```

Runs the program, then reports every resource value that is still pending or
loading at the end: its state, age in frames, absorption count, and origin
call site. It answers "why is this region blank?".

Text output is one line per live resource, or `No pending resources.` when
there are none. With `--json`, it emits the raw report array:
`[{id, key, state, age_frames, origin, absorbed_count}, ...]`.

### `propose-edit` — Propose source edits that change an emitted value

```
petal propose-edit --channel <name> --emit <n> (--arg <k> --to <value>)...
                   [--configurable <var>]... [--static <var>]...
                   [--apply] [--json] <file.ptl>
```

Runs the program with emit tracing, then works backwards: given an emitted
value, it proposes the source edits that would make argument `<k>` of the call
that produced it evaluate to `<value>`. This is the writing half of direct
manipulation: the host says where the user dragged something, and gets back
the edits that mean it. See [direct-manipulation.md](direct-manipulation.md).

Use `run --trace-emits` first to find the emit's address. For this file:

```petal
let x = 10
let offset = 5
push_output(symbol("shapes"), x + offset)
```

```
$ petal run --trace-emits sketch.ptl

Channel 'shapes' (1 emits):
  [0] push_output [line 3] <- 15
      arg 0: computed
      arg 1: computed

$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 sketch.ptl
2 proposals:
  1. set `x` to 37.5 (line 1)
  2. set `offset` to 32.5 (line 2)
Narrow with --configurable <var> / --static <var>, or apply one by hand.

$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 --static x --apply sketch.ptl
1 proposal:
  1. set `offset` to 32.5 (line 2)
Applied.
```

Options:

- `--channel <name>` — the output channel the emit was pushed into. Required.
- `--emit <n>` — 0-based index of the emit within that channel. Required.
- `--arg <k> --to <value>` — argument `<k>` (0-based) of the producing call
  should evaluate to `<value>`, written as source text: `55`, `2.5`, `true`,
  `hello`. At least one pair is required. Repeat the pair for a multi-goal
  batch (one gesture changing several arguments); each `--to` binds to the
  `--arg` before it.
- `--configurable <var>` — a variable the host prefers to edit. Repeatable.
- `--static <var>` — a variable that must not be edited. Repeatable.
- `--apply` — rewrite the file in place, but only when every goal resolves to
  exactly one proposal.
- `--json` — emit the proposals as JSON, with exact spans and replacement
  text: one entry per goal under `goals`, plus the flat `proposals` key when
  there is exactly one goal.

Declaring knobs in the source with `config let` narrows proposals without
flags: config bindings are treated as configurable and every other binding as
static.

### Dump format conventions

The four stage dumps — [`show-tokens`](#show-tokens--lexer-token-stream),
[`show-ast`](#show-ast--parsed-ast),
[`show-ir`](#show-ir--compiled-ir-term-graph), and
[`show-bytecode`](#show-bytecode--bytecode-lowering) — share these
conventions, documented once here and referenced from each section.

**Spans.** Lines and columns are 1-based; span ends are exclusive (one past
the last character); byte offsets, where present, are 0-based.

- *Text dumps* print spans as `@line:col-line:col`, collapsed to `@line:col`
  when the end adds nothing (a single-character AST span; IR term lines,
  which print only the start). A location in a non-entry file is prefixed
  with the file's display name: `@std:21:1`.
- *JSON dumps* use compact arrays. Token rows carry a 4-element
  `[startLine, startCol, endLine, endCol]`. The AST and IR dumps share the
  lossless **SourceSpan** encoding
  `[startLine, startCol, startOffset, endLine, endCol, endOffset]`, with a
  seventh element — the `source_map.files` index — appended only when
  nonzero (non-entry file).

**Id prefixes.** In text dumps every id kind gets a prefix; in JSON the same
ids are bare integers (see the schema sections below for which field is
which):

| Prefix | Names | Appears as |
|--------|-------|------------|
| `tN` | TermId — a node in the IR term graph | `t117` |
| `rN` | register index in a frame / function register file | `r0` |
| `cN` | ConstantId — index into the constant table | `c3` |
| `fnN` | FunctionId (`fn0`); the bytecode text abbreviates it to `fN` (`closure f0`) | `fn0`, `f0` |
| `blockN` | BlockId | `block1` |
| `kN` | state key (a `u64`) — bytecode text, decimal; the IR text prints the same key as `key=0x…` hex | `k677…`, `key=0x5dff…` |
| `slotN` | loop-cursor slot (bytecode only) | `slot0` |

A state key names the *declaration*; the slot it selects at runtime is that key
plus the call path that reached it (see
[state-call-paths.md](dev/state-call-paths.md) §2). The path
is a runtime value, so no static dump prints one — what the dumps carry is its
static half: a call term's `call_site` id in `show-ir --json`, and `popN` on a
`state_read`/`state_write` in the bytecode text (the number of enclosing loop
steps the access drops to land on its declaration's slot, omitted when zero). A
live path appears only in host state dumps (as `counter#1/[3]/count`).

**Defaults are omitted in JSON.** Every JSON dump skips a field whose value
is its default — a `null` option, a `false` boolean, an empty
array/string/map — and readers must treat absence as the default. (Token
rows omit `value` on unit tokens; the AST omits `exported`/`is_var`/`ty`/…;
the IR omits `name`/`inputs`/`child_blocks`/… .) The one exception is the
structured `inst` object in the bytecode JSON, whose operands keep explicit
`null`s so each opcode has a fixed field set.

**Contract vs. debug view.** Exactly one dump is a stable interchange
format: the unfiltered IR JSON (`show-ir --json`), which `run --ir` /
`check --ir` load back and foreign front-ends emit (see
[ir-as-target.md](dev/ir-as-target.md)). Everything else — all four text
forms, the tokens/AST/bytecode JSON, and the filtered `--user-only` IR view
— is a debug view: kept accurate here, but versionless and subject to
change, and not loadable by anything.

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
- `span` — `[startLine, startCol, endLine, endCol]` per the shared span
  rules ([Dump format conventions](#dump-format-conventions)); token rows
  are the one 4-element span form — no byte offsets. The text form prints
  the same span as `@startLine:startCol-endLine:endCol`.

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
string interpolations and elements, `Field`/`Spread` inside records. Both
forms are debug views (see
[Dump format conventions](#dump-format-conventions)); `--json` is the
machine-readable one.

**JSON output** (`--json`) — array of `Stmt` nodes. For `let x = 1 + 2`:

```json
[
  {
    "kind": {
      "Let": {
        "name": "x",
        "value": {
          "kind": {
            "BinaryOp": {
              "op": "Add",
              "left": { "kind": { "Literal": { "Int": 1 } }, "span": [1, 9, 8, 1, 10, 9] },
              "right": { "kind": { "Literal": { "Int": 2 } }, "span": [1, 13, 12, 1, 14, 13] }
            }
          },
          "span": [1, 9, 8, 1, 14, 13]
        }
      }
    },
    "span": [1, 1, 0, 1, 14, 13]
  }
]
```

Note the absent `ty`/`is_var`/`is_config`/`exported` fields on the `Let`:
default-valued fields are omitted (see below).

#### AST JSON Schema

All AST enum types use serde's externally-tagged representation. `Stmt` and
`Expr` are serialized as `{kind: <variant>, span: SourceSpan}` (a `Stmt` also
carries `"exported": true` when declared with the `export` modifier) —
`SourceSpan`
is the same compact array encoding the IR uses
(`[startLine, startCol, startOffset, endLine, endCol, endOffset, file?]`, see
[Dump format conventions](#dump-format-conventions)), and the `<variant>`
shapes are listed in the `StmtKind` and `ExprKind` tables below.
The canonical definitions live in `rust/src/ast.rs`; the tables below cover
the common variants but are not exhaustive.

**Defaults are omitted** (the shared rule — see
[Dump format conventions](#dump-format-conventions)). A field holding its
default value is left out of
the JSON rather than written explicitly: `false` flags (`exported`, `is_var`,
`is_config`), absent options (`ty`, `ret`, `resolved`, `class`, `key`,
`guard`, `else_body`, `alias`, `names`, a list pattern's `rest`), and empty
variant field lists (`EnumVariant.fields`, a `Variant` pattern's `fields`).
Read a missing field as its default. The tables below write these as
`Type (omitted when …)`.

Consumers should treat the single key of the `kind` object as the node type
(serde's externally-tagged enum encoding).

**StmtKind** (top-level statements):

| Variant | Shape |
|---------|-------|
| `Let` | `{"Let": {"name": string, "ty": TypeAnn (omitted when un-annotated), "value": Expr, "is_var": bool (omitted when false), "is_config": bool (omitted when false)}}` — `ty` is the optional declared type annotation; `is_var` is true for `var x = …`; `is_config` for `config let` |
| `Assign` | `{"Assign": {"target": AssignTarget, "value": Expr}}` |
| `Expr` | `{"Expr": Expr}` |
| `FnDecl` | `{"FnDecl": {"name": string, "class": string (omitted for ordinary functions), "params": Param[], "ret": TypeAnn (omitted when un-annotated), "body": Stmt[]}}` — `ret` is the optional declared return-type annotation; `class` is set for a method declaration (`fn Rect.center_x(…)` → `class: "Rect"`, `name: "Rect.center_x"`, receiver as `params[0]`) |
| `EnumDecl` | `{"EnumDecl": {"name": string, "variants": EnumVariant[]}}` |
| `ClassDecl` | `{"ClassDecl": {"name": string, "fields": ClassField[]}}` — a `class Name … end` declaration; fields are in declaration (and constructor-argument) order |
| `For` | `{"For": {"var": string, "iter": Expr, "body": Stmt[]}}` |
| `While` | `{"While": {"condition": Expr, "body": Stmt[]}}` |
| `Return` | `{"Return": Expr \| null}` |
| `Break` | `"Break"` |
| `Continue` | `"Continue"` |
| `State` | `{"State": {"name": string, "ty": TypeAnn (omitted when un-annotated), "init": Expr, "id": number, "key": Expr (omitted when unkeyed), "is_var": bool (omitted when false)}}` — `ty` is the optional declared type annotation; `key` set when the source uses the `state(expr) name = init` per-iteration form; `is_var` for `state var` |

**ExprKind** (expressions):

| Variant | Shape |
|---------|-------|
| `Literal` | `{"Literal": Literal}` |
| `Ident` | `{"Ident": string}` |
| `BinaryOp` | `{"BinaryOp": {"op": BinOp, "left": Expr, "right": Expr}}` |
| `UnaryOp` | `{"UnaryOp": {"op": UnaryOp, "operand": Expr}}` |
| `Call` | `{"Call": {"function": Expr, "args": Expr[]}}` |
| `If` | `{"If": {"condition": Expr, "then_body": Stmt[], "else_body": ElseBranch (omitted when there is no else)}}` |
| `Match` | `{"Match": {"subject": Expr, "arms": MatchArm[]}}` |
| `List` | `{"List": Expr[]}` |
| `Record` | `{"Record": RecordField[]}` |
| `FieldAccess` | `{"FieldAccess": {"object": Expr, "field": string}}` |
| `IndexAccess` | `{"IndexAccess": {"object": Expr, "index": Expr}}` |
| `Block` | `{"Block": Stmt[]}` |
| `Lambda` | `{"Lambda": {"params": Param[], "body": Stmt[]}}` |
| `StringInterp` | `{"StringInterp": {"parts": string[], "exprs": Expr[]}}` — `parts` has one more element than `exprs` |
| `Element` | `{"Element": {"tag": string, "props": [string, Expr][], "children": JsxChild[]}}` |

**Param**: `{"name": string, "ty": TypeAnn (omitted when un-annotated)}` — a function/lambda parameter with its optional declared type annotation.

**ClassField**: one field of a `ClassDecl`, as `{"name": string, "ty": TypeAnn (omitted when un-annotated)}` — the same optional-annotation shape as a **Param**.

**TypeAnn**: a written type annotation as an object `{"name": string, "resolved": Type (omitted when unrecognized)}` — `name` is the type name exactly as written in the source (`"int"`, `"str"`, `"banana"`), and `resolved` is the recognized static type (omitted for an unrecognized name, e.g. `{"name": "banana"}`). An absent annotation omits the `ty`/`ret` field entirely. Annotations appear on `Let`, `State`, `Param`, `ClassField`, and `FnDecl.ret`; a class name resolves to a static type too, but only against the compilation's class table, so a `TypeAnn` naming one carries no `resolved` in the AST dump; they are type-checked (warnings only — see [`check`](#check--compile-without-running)) and dropped before codegen, so they never appear in the IR.

**Type**: a string naming the recognized static type — one of `"Any"`, `"Nil"`, `"Bool"`, `"Int"`, `"Float"`, `"String"`, `"List"`, `"Record"`, `"Function"`, `"Enum"`, `"Vec2"`, `"F64Array"`, `"Element"`, `"Symbol"`, `"Dual"`, `"Handle"`, `"Pending"`. Appears as the `resolved` field of a **TypeAnn**.

**RecordField**: `{"Named": [string, Expr]}` or `{"Spread": Expr}`.

**JsxChild**: `{"Text": string}` or `{"Expr": Expr}`.

**Literal**: `"Nil"`, `{"Bool": bool}`, `{"Int": number}`, `{"Float": number}`, `{"String": string}`

**BinOp**: `"Add"`, `"Sub"`, `"Mul"`, `"Div"`, `"Mod"`, `"Eq"`, `"Ne"`, `"Lt"`, `"Le"`, `"Gt"`, `"Ge"`, `"And"`, `"Or"`, `"Concat"`

**UnaryOp**: `"Neg"`, `"Not"`

**AssignTarget**: `{"Name": string}`, `{"Field": [Expr, string]}`, `{"Index": [Expr, Expr]}`

**ElseBranch**: `{"Block": Stmt[]}`, `{"ElseIf": Expr}`

**MatchArm**: `{"pattern": Pattern, "guard": Expr (omitted when unguarded), "body": Expr}`

**Pattern**: `"Wildcard"`, `{"Literal": Literal}`, `{"Variable": string}`, `{"Variant": {"name": string, "fields": Pattern[] (omitted when empty)}}`, `{"List": {"elements": Pattern[], "rest": string (omitted when there is no `...rest`)}}`, `{"Record": [string, Pattern][]}` — the same encoding the IR JSON's `match_arms` uses (see [ir-as-target.md](dev/ir-as-target.md))

**EnumVariant**: `{"name": string, "fields": string[] (omitted when empty)}`

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
(id prefixes and span form per
[Dump format conventions](#dump-format-conventions)).

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
  write visibly links to its init. The key identifies the *declaration*; which
  slot it lands in also depends on the runtime call path, which no static dump
  can show (see [Dump format conventions](#dump-format-conventions)).
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
  "match_arms": {...},
  "class_names": ["Rect"]
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
format — it is **not loadable** by `run --ir`.

#### Program JSON Schema

The IR JSON is the complete compiled `Program` struct — **schema 0.2** (see
[docs/dev/ir-as-target.md](dev/ir-as-target.md) for the full emit-target
contract and the legacy-v0 tolerance rules). All ID newtypes serialize as
their inner integer (e.g. `TermId(5)` becomes `5`). **Defaults are omitted on
the wire**: any field whose value is its default — a `null` option, an empty
array/string/map, a `false` boolean — is simply absent, and loaders treat
absence as the default (the shared rule — see
[Dump format conventions](#dump-format-conventions)).

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
| `source_map` | `{"term_spans": {}, "files": []}` | `term_spans`: TermId → SourceSpan mapping (string keys). `files`: file table for multi-file programs — entry file at index 0, imported modules after (omitted for single-file programs). The whole `source_map` is omitted when empty |
| `has_errors` | `boolean` | Whether the program has parse errors (omitted when `false`) |
| `functions` | `FunctionDef[]` | All function definitions (omitted when empty) |
| `match_arms` | `{[termId: string]: MatchArmMeta[]}` | Match term → arm metadata (string keys); omitted when empty |
| `class_names` | `string[]` | Sorted names of every class the program declares, built-ins included (the prelude's `Rect` makes it non-empty for any CLI compile). The runtime's answer to "is this value's class label a live class here?" — it gates `MethodCall` hint dispatch. Omitted when empty |

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
| `path_pop` | `number` | Omitted when `0`. On a `StateRead`/`StateWrite`: how many enclosing loop steps the access drops from the live call path to address the slot its declaration owns (a `state` declared outside a loop and reassigned inside one). Always `0` on a `StateInit`, which *is* the declaration. |
| `call_site` | `number` | Omitted when absent. On a `Call`/`MethodCall`/`BuiltinCall`: the callsite id (a `u64`) the callee's frame pushes onto its call path, which is what gives each callsite of a function its own `state` slots. Derived from the callee's spelling, its ordinal among identically-spelled callees in the enclosing function, and that function's module/name chain — never from term ids or spans, so it survives a hot reload. Hand-written IR may omit it; a call with no id contributes a shared id of `0`, i.e. one slot per declaration as before call-path keying. |
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

**SourceSpan** — the compact lossless array shared by the IR `source_map`
and the AST JSON, defined in
[Dump format conventions](#dump-format-conventions):
`[startLine, startCol, startOffset, endLine, endCol, endOffset, file?]`. On
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

Text-form conventions (see
[Dump format conventions](#dump-format-conventions) for the shared prefixes):
`rN` is a register in the function's flat register file, `fN` a FunctionId,
`slotN` a loop-cursor slot, `kN` a state key.
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
  has a fixed field set — the one exception to the shared
  [omit-defaults rule](#dump-format-conventions).

The full instruction set is documented in `rust/src/backend/bytecode/isa.rs`.

## Dataflow query commands

These commands query the compiled dataflow graph without running the program.
They all take `--term <name|id>` with the same resolution rules as `explain`.

All three stop at every `var` cell. See
[Cells and the frontier](#cells-and-the-frontier): none of them promises an
unqualified "minimal" answer on a program that uses `var`, and each one says so
in its output.

### `show-provenance` — Backward slice

```
petal show-provenance [--json] --term <name|id> <file.ptl>
```

Prints the terms that feed into the target, with the edges connecting them.
"What does this value depend on?"

```
$ petal show-provenance --term total -e 'let a = 2
let total = a * 3'
Provenance of t119 (total):
  op: Mul
  inputs: [117, 118]

Ancestors (3):
  t117: Copy
  t118: Constant(ConstantId(1))
  t116: Constant(ConstantId(0)) a

Edges (3):
  t117 -> t119
  t118 -> t119
  t116 -> t117
```

JSON shape:
`{root: Term, ancestors: Term[], edges: [{from, to, kind}, ...], frontier: [...], complete: bool}`.
`kind` is always `"dataflow"` here: a backward walk answers a *must* question,
so it only crosses value edges.

### `show-dependents` — Forward slice

```
petal show-dependents [--json] --term <name|id> <file.ptl>
```

The mirror of `show-provenance`: everything downstream of the target. "What
would a change to this value reach?"

JSON shape: `{root: Term, dependents: Term[], edges: [{from, to, kind}, ...]}`.

This direction answers a *may* question, so it also carries `"kind": "may"`
edges from a `var` declaration and from every `set` to every read of that cell
(`t96 ~> t97 (cell 'x', may)` in text mode). Which write actually supplied a
given read is a runtime fact; over-approximating it is correct, while
under-approximating it would report that a `set` affects nothing.

### `show-slice` — Subgraph for multiple targets

```
petal show-slice [--json] --term <a> [--term <b> ...] <file.ptl>
```

Prints the smallest subgraph that still produces every target. Useful for
focused visualizations and for extracting the interesting part of a larger
program.

JSON shape: `{targets, slice: Term[], frontier: [...], complete: bool, minimal: bool}`.

On a cell-free program this is the smallest such subgraph. On a program that
reads a `var` it is deliberately **not** minimal: it also pulls in the
declaration and every `set` site, transitively, and reports
`"minimal": false` with a `Not minimal — N cell reads crossed:` block. Too
small would silently compute a different value; too big only loses precision.

### Cells and the frontier

A `var` binds a mutable heap cell (see
[`var` and `set`](language-guide.md#var-and-set)). The cell operand of a
`CellRead`, a `CellWrite` or a closure capture is an *identity* edge — it names
which box, not which value — so a backward walk never crosses one. Every stop
is reported as a **frontier** entry naming the var, its declaration, and the
complete set of writes that could have supplied the value:

```
Frontier (1):
  t97: read of var 'x' (not traced)
    possible write: t96 [line 2, column 1]
```

The write set is complete because no expression evaluates to a cell, so the
only way to reach one is a name lexically bound to its declaration. The
exception is a `state var`, whose slot the host can also write through
`set_state`; that case is printed rather than glossed.

The `show-*` commands never run the program, so they always give this static
answer (`"resolution": "not_traced"`). `explain` does run it, and so resolves
the boundary to the exact write and continues the chain through it.

`--term x` on a `var` resolves to the *last* `CellWrite` on that name, since
name lookup is last-wins. `explain` from the write shows that write's own
chain, and `show-dependents` from it shows every read the write may reach.
Pass an explicit `--term tNN` to start somewhere else.

### `show-graph` — Graphviz DOT export

```
petal show-graph [--all] <file.ptl>
petal show-graph [--all] -e '<code>'
```

Prints the dataflow graph in DOT format, ready to pipe into `dot -Tpng`.
Phantom builtin terms are hidden unless `--all` is given. There is no `--json`
output; the format is DOT.

Nodes are colored by role (constants light blue, state pink, user bindings
white) so the output stays readable on mid-sized programs.

## Builtin Phantom Terms

Every **compiled** program starts with one phantom term per registered
built-in function (`t0`, `t1`, …) in the root block. These are `Copy` terms
with no inputs; their `name` field holds the builtin name. The IDs follow
the registration order in `rust/src/builtins/mod.rs`.

The phantoms exist for name resolution and for using a builtin as a
first-class *value*; a direct call like `print(x)` compiles to a
`BuiltinCall` naming the builtin via a string constant instead. The runtime
seeds native function values into phantom registers **by name** when the
root frame is pushed — so an imported IR document (schema 0.2) needs no
phantoms at all unless it wants a builtin as a value, and never depends on
registration order. See [docs/dev/ir-as-target.md](dev/ir-as-target.md).

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
