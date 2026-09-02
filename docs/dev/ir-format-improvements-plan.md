# Intermediate format improvements

Record of an audit of Petal's four intermediate-format dumps — tokens, AST, IR
term graph, bytecode — each in text and `--json` form, and of the changes it
led to. Everything the audit ranked has shipped; the current formats are
documented in [CLI.md](../CLI.md) (see "Dump format conventions" and the
`show-*` sections), and the IR JSON contract in
[ir-as-target.md](ir-as-target.md).

## What the audit found

The four dumps had four conventions: tokens used raw serde enums with no
spans, the text AST was Rust `{:#?}` debug output (about 100 lines for
`let x = 1 + 2.5`), the IR listing cross-referenced constants by id and hid
the phi and match-arm mechanisms, and the bytecode JSON was prerendered
strings. The IR JSON also leaked in-memory details (a hand-maintained
`block_next`/`block_prev` linked list, mandatory registers, builtin phantom
terms in registration order) into the emit-target contract, and `show-ir`
flooded every dump with the `std` prelude: the MCP `ShowIR` tool returned
about 2,000 lines for a six-line program.

## What shipped

1. **Prelude and phantoms hidden by default.** `show-ir` shows the user's
   program; `--all` restores everything, `--user-only` filters the JSON form
   too. The MCP `ShowIR` tool returns the user-only view unless `all: true`.
2. **Compact text AST.** One node per line, key facts inline, spans as
   `@1:9-1:16`.
3. **Self-contained text IR.** Constants resolve inline
   (`BuiltinCall("map")`), block headers list params/captures/self with their
   term ids and registers, `phi_outs` and match arms print under their owner,
   and every term carries a source position.
4. **IR JSON schema 0.2.** Defaults omitted, an ordered `terms` array per
   block instead of the linked list, registers optional (the loader
   recomputes them), builtin phantoms matched by name rather than position,
   compact array spans, and an explicit `"schema": "0.2"` field. Legacy
   documents still load.
5. **Token dumps with spans**, as uniform `{kind, value?, span}` rows.
6. **AST JSON** skips default fields and uses the same compact spans.
7. **Structured bytecode JSON**: each instruction carries the `inst` operand
   object alongside its rendered `text`.
8. **One conventions section in CLI.md** covering span encoding, id prefixes,
   the omit-defaults rule, and which dumps are contracts versus debug views.

## Observations that are IR design, not format

Still open, and out of scope for the dumps themselves:

- **Copy-per-reference**: every variable use emits a `Copy` term, roughly
  doubling term count. It is load-bearing (each reference site is a provenance
  node with its own span), and bytecode copy-propagation erases the runtime
  cost, but a display-level fold ("inline single-use Copies") could be a
  future `show-ir` nicety.
- String interpolation compiles to a `Concat` chain ending in a concat with
  the constant `""`. Harmless, mildly confusing in dumps.
- `match_arms` embeds `ast::Pattern`, an AST type, inside the IR: the one
  place the "IR is independent of surface syntax" claim breaks. An IR-level
  pattern encoding would fix it.
- Builtin identity is a string in the constant table, resolved by name at
  lower time. Unifying builtin identity further is a separate design
  question.
