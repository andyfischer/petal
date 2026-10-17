# Provenance

Petal compiles a program to a dataflow graph: every expression is a term, and
every term names the terms it was computed from. The provenance tools read
that graph to answer three questions about a value:

| Question | Tool | Runs the program? |
|---|---|---|
| Why does `x` have this value? | `petal explain --term x` | yes, with tracing on |
| What does `x` depend on? What would a change to `x` reach? | `petal graph --term x [--direction forward]` | no |
| Which call drew this, and which literal moves it? | emit tracing, `petal run --trace-emits` | yes |

The first two are this page. The third is
[direct manipulation](direct-manipulation.md). Every flag and output field is
in the [CLI reference](../CLI.md#dataflow-query-commands).

## Why does a value look like that: `explain`

`explain` runs the program with the execution trace on, then walks backward
from the term and prints the value every ancestor had:

```petal
let base = 10
let scale = 3
let offset = base * scale
let width = offset + 4
print(width)
```

```
$ petal explain --term width layout.ptl
34
Explain t160 (width):
  Provenance chain:
    => t160 width [line 4, column 13] = 34
     . t158 - [line 4, column 13] = 30
     . t159 - [line 4, column 22] = 4
     . t157 offset [line 3, column 14] = 30
     . t155 - [line 3, column 14] = 10
     . t156 - [line 3, column 21] = 3
     . t153 base [line 1, column 12] = 10
     . t154 scale [line 2, column 13] = 3
```

The first line is the program's own output. Each later line is a term, its
name if it has one, where it is, and what it evaluated to. `--term` takes a
name (the last term bound to it), a term id (`160`) or the `t`-prefixed form;
`--json` gives the chain as data. The trace is a bounded ring buffer
(`core/src/trace.rs`, 200,000 events), so on a long run the oldest values can
have been dropped, and the JSON says so (`truncated`).

## What depends on what: `graph`

`graph` answers from the compiled graph alone, without running anything:

- **Backward** (the default): the terms a value is computed from.
- **Forward** (`--direction forward`): every term a change to the value
  would reach.
- **Slice** (several `--term`s, or `--slice`): the smallest part of the
  program that still computes all of them.

```
$ petal graph --term base --direction forward layout.ptl
Dependents of t153 (base):
  op: Constant(ConstantId(0))

Downstream (6):
  t155: Copy
  t157: Mul offset
  t158: Copy
  t160: Add width
  t161: Copy
  t162: BuiltinCall(ConstantId(3))
...
```

Every query returns the same JSON shape (`terms`, `edges`, `frontier`,
`complete`, `minimal`), so a tool can draw any of them the same way.
`petal show-graph` writes the whole graph as Graphviz DOT.

## Where the answer stops: `var` cells

A `let` is a dataflow edge, so a backward walk through `let`s is exact. A
[`var`](../syntax/var.md) is a mutable cell: which `set` supplied a given read
is a run-time fact. `graph` therefore stops at every read of a cell and lists
it in `frontier`, with every write that could have supplied it, and marks the
answer `complete: false`. A forward walk crosses the cell along "may" edges to
every read. `explain` runs the program, so it knows which write it was and
carries the chain on through it. The rules are in
[Cells and the frontier](../CLI.md#cells-and-the-frontier).

This is the price of `var`: a program written with `let` gets exact answers,
one written with cells gets the set of possible writers.

## Reading values without a query: observation

A host, or `petal run --observe`, can record the last value bound to every
named term of a run, without tracing the whole execution. That answers "what
was everything set to?" for one frame, and is what Garden and the debug
protocol show beside the code
([embedding guide](../usage/embedding-guide.md#reading-arbitrary-named-values-observation),
[debugging and visibility](../dev/debugging-visibility.md)).

## From code

The same queries are methods on `Program` (`core/src/program_analysis.rs`):

| Method | Answers |
|---|---|
| `find_term(name_or_id)` | the term `--term` would pick |
| `trace_provenance(term)` | the backward walk, with its frontier |
| `trace_dependents(term)` | the forward walk |
| `slice(&targets)` | the smallest subgraph that computes every target |
| `cell_index()` | every cell's declaration, writes and reads |

The run-time half is the trace buffer: `Env::trace_mut().enabled = true`
before a run, `Env::trace()` after it. The MCP server exposes `explain` as the
`ExplainTerm` tool ([mcp-server.md](../dev/mcp-server.md)).

## See also

- [Direct manipulation](direct-manipulation.md): from a drawn shape back to the
  call and the literal that produced it.
- [`var`: provenance](../syntax/var.md#provenance): what a cell costs these
  tools.
- [Architecture](../dev/Architecture.md): the term graph itself.
- [aspirational-ideas/provenance.ptl](../aspirational-ideas/provenance.ptl):
  where this is meant to go.
