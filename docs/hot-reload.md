# Hot reload

A host that keeps a Petal program running while its source is edited calls one
function when a file changes:

```rust
let source = std::fs::read_to_string(&entry)?;
match env.reload_program(stack, &source, Some(&entry)) {
    Ok(report) => log!("reload: {}", report.outcome.label()),
    Err(e) => show_diagnostics(e.phase, &e.items), // the old program keeps running
}
```

`Env::reload_program` compares every source file the program was compiled from
with the text that file holds now, and does only the work the edit calls for:

| The edit | Outcome | What happens |
|---|---|---|
| nothing changed | `unchanged` | nothing |
| whitespace, comments, layout | `relocated` | the program's recorded source text and positions are updated |
| literal values (`0.35` to `0.4`, `"red"` to `"blue"`, `#ff2e88` to `#ff2e80`) | `patched` | the constants are written into the running program |
| anything else | `recompiled` | the program is recompiled and swapped in with `transfer_state` |

The first three compile nothing and lower nothing. On `games/neon` in the
Cheesecake engine (14 files, 10,300 lines) a value edit takes 0.8 ms where the
recompile it replaces took about 300 ms; the numbers are under
[What it costs](#what-it-costs).

The contract is the same on every path: **afterwards the program and the stack
are what a full recompile of the new source, followed by `transfer_state`,
would have left.** The same terms, the same constants by value, the same span
for every term, the same warnings, the same bytecode, the same `state`. When
that cannot be shown for an edit, the edit is recompiled.

The C bridge's `pb_vm_reload` goes through this, as do the SDL host's watcher
and Garden's script and panel hosts. See
[embedding-c.md](embedding-c.md#hot-reload) for the C side.

## What each outcome leaves alone

| | `unchanged` / `relocated` | `patched` | `recompiled` |
|---|---|---|---|
| `state` | kept | kept | kept where the declaration still exists |
| closures, the captured function table | kept | dropped, recaptured by the next run | dropped |
| memo records | kept | dropped | dropped |
| frame gate (`run_needed`) | unchanged: no frame is forced | the next frame runs | the next frame runs |
| bytecode | kept | kept (one operand retargeted, the first time a literal changes) | lowered again on the next run |

After any reload the stack is ready to run from the top: `run` may follow
without a `reset_stack`.

A `patched` reload drops what a `recompiled` one drops, by running the same
code (`transfer_stack_state`). A closure captured the old value; a memo record
of a function whose body loads the constant has an input the record does not
list; a `state` slot that was *initialized* from the old value keeps its value
under both, because an initializer runs once.

## What kind of edit is it

`petal::source_diff::diff_source(old, new, file)` parses both texts and
compares the syntax trees. The result is a `SourceChange`:

- **`None`**: the trees are the same. The texts are identical, or differ in
  whitespace, comments, blank lines, line wrapping, or a trailing comma.
- **`Values(list)`**: the trees differ only in the values of literals, each
  keeping its kind (int, float, string, bool). Every entry names the literal's
  old and new value and its span in both texts, and for a literal inside a
  top-level `let` or `config let` its binding path: `SPEED`,
  `POST.effects[2].amount`, `SUN.dir[1]` (a call's arguments count as
  elements, as in [config files](config-files.md)). A literal in a function
  body or a statement is a value change too, with no path.
- **`Constructs(list)`**: the structure changed. Each entry names a top-level
  construct (`fn draw`, `config let GEN`, `state score`, an import, a bare
  statement) and whether it was added, removed or changed.
- **`Full(reason)`**: the new text does not parse, or the edit is one the tree
  does not show (`export` respelled `pub`, redundant parentheses added).

`Env::diff_program(pid, new_entry_source)` runs this over every file of a
loaded program and returns a `ProgramChange`; `Env::apply_program_change`
applies one that needs no recompile, and refuses any other. `reload_program`
is the two together with the recompile as the fallback, and its `ReloadReport`
carries the classification, the files that changed, and (when an edit that
looked incremental was recompiled anyway) why.

These are value changes:

```petal
config let SPEED = 4.0          // 4.0 -> 5.5
config let POST = {
  bloom: 0.2,                   // any field, at any depth
  effects: [{amount: 0.3}],     // any element
  tint: rgb(255, 0, 80),        // any argument of a constructor call
  accent: #ff2e88,              // a color, component by component
  offset: -1.5,                 // -1.5 -> 1.5: see below
}
fn area(w)
  (w + 2) * 10                  // literals in code, too
end
```

These are construct changes, and recompile:

- a literal that changes type: `10` to `10.5`, `1` to `"one"`, `nil` to `0`;
- a list or record that changes shape: an element or field added or removed;
- a literal in a `match` pattern (patterns are compiled into the program's
  arm metadata, not into constants);
- the text of an interpolated string (`"n is {n}"`);
- a literal divisor reaching or leaving zero (`w / 2` to `w / 0`): the
  compiler hoists a constant `let` only when it divides by a non-zero literal.

### Negative numbers in config data

`-3` is the negation of `3`, so dragging a value across zero changes the
tree's shape. Inside the initializer of a `config let` (the value itself, and
through list elements, record fields and call arguments) the compiler folds a
negated number literal into one constant, and the diff reads it the same way,
so `-0.5` to `0.5` is a value change there. Elsewhere `-3` to `3` is a
construct change, and `-3` to `-4` a value change. An operator ends the data
position: in `config let W = 2 * -1` the `-1` stays a negation.

## Setting a value without writing the file

A drag should not write a file sixty times a second. `Env::set_config_value`
sets one value in the running program directly:

```rust
env.set_config_value(stack, None, "POST.effects[2].amount", &StaticValue::float(0.35))?;
```

The path is a binding path as in [config files](config-files.md). The second
argument names the source file holding the binding; `None` finds it, and
requires that exactly one file binds the name at its top level.

The call edits the text the running program holds for that file, with
`literal_edit::set_path` (the edit a host makes to the file itself, keeping
comments and layout), and applies the result through the same path as a
reload, if it needs no recompile. So:

- the program afterwards is exactly what reloading that edited file gives;
- `Env::program_source(pid, file)` returns the edited text. When the drag ends
  the host writes it to disk, and the reload that follows is `unchanged`;
- to abandon the drag, reload: the file's own values come back.

It returns `ConfigSetError::NeedsReload` when the value has a different type or
shape from what is written (an integer literal given a float, a list of a
different length), and changes nothing. Write the file and reload for those.
Any top-level `let` of data can be set this way; `config` is not required.

From C: `pb_vm_set_config(vm, file, path, value_json)` and
`pb_vm_source_text(vm, file)`.

## How a value is patched

A literal compiles to a `Constant` term, lowered to one `LoadConst`
instruction that reads the program's constant table each time it runs. The
optimizer passes look at instruction kinds and registers, never at constant
values, so nothing downstream holds the value.

Constants are deduplicated, so the first time a literal changes its term is
given a slot of its own (`ConstantTable::alloc_slot`), and the one instruction
lowered from that term is pointed at the slot. After that a change is one
write to the table. The constant table is the cell the value is read through.

The term is found from the diff: the `Constant` terms recorded at the
literal's old span, which must be as many as the tree has literals there (one,
except for a color literal's components), and must hold the old value. If
either check fails the reload recompiles.

Source positions move with a map built from the two token streams. The streams
agree token for token apart from the changed literals (line breaks and commas
are layout and are skipped), so each old token pairs with a new one, and a
span's start and end move with the tokens they sit on. A span that sits on
nothing both versions have makes the reload recompile.

## Why this is exact

The claim is that changing a literal's value changes one constant and nothing
else the compiler produced. It rests on where the front end reads literal
values:

1. the term the literal compiles to;
2. the constant-`let` hoisting rule, for a literal divisor (handled by the diff
   as above).

The type checker reads a literal's kind only.

Layout reaches the compiler's output in two more ways, both through warnings:

- A warning may name another place by line number (`... written on line 12
  ...`). Such a diagnostic is built with `Diagnostic::citing`, which keeps the
  cited span and the message template, and a reload re-renders it.
- One check compares layout itself: a statement starting with `-`, indented
  under an unfinished-looking line, is reported as a broken continuation. A
  check that reads layout records what it compared as a `LayoutDep` on the
  program, and a layout-only reload re-reads each pair at its new position. If
  a reading changed, the edit is recompiled.

A value change inside code the compiler warned about is recompiled as well,
since only the compiler can say whether the warning still reads the same.

**If you add a compile-time check, keep this true.** A check that reads a
literal's value, quotes a line number, or compares lines or columns must go
through one of the mechanisms above. `core/tests/hot_reload.rs` is where a
violation shows up.

## What is tested

`core/tests/hot_reload.rs` runs each case twice from the same starting point,
once through `reload_program` and once through recompile plus `transfer_state`,
and compares what a host could tell apart: the output, value or error of the
frames that follow (error text carries a line and column), the state after
each, the two programs' IR (`ir_equivalent`), source text, every term's span,
warnings, and the bytecode. Each case also states which path it expects.

The cases: a config scalar, nested fields and elements, a sign flip, colors,
strings and booleans, a config value used by closures, by a lambda held in
`state`, by a state initializer, by a default parameter, by other config
bindings, and across module imports; a literal in a function body and one a
memoized function reads; whitespace-only and comment-only edits; a runtime
error's position after a layout edit; warnings; a function body change; added
and removed bindings; type-changing edits; and edits the tree does not show.
Further tests assert that a value edit compiles nothing and lowers nothing
(`Env::work_counters`), and time the paths on a generated program.

Two sweeps cover real programs. The default run applies a layout edit, an
indentation-flattening edit and a number edit to every console example. The
ignored one does the same over any tree:

```bash
cd core
PETAL_RELOAD_CORPUS=/path/to/scripts cargo test --release --test hot_reload -- --ignored
```

Over the 427 `.ptl` files of this repository and the Cheesecake games it
reports 647 edits relocated, 343 patched and 10 recompiled, every one
identical to the recompile.

## What it costs

`core/examples/reload_timing.rs` times each path on a real program, lowering
included, without touching the disk:

```bash
cd core && cargo run --release --example reload_timing -- game.ptl config.ptl
```

On `games/neon` (14 files, 10,346 lines, 62,733 terms; release build, Apple
silicon, minimum of 9):

| Edit to `config.ptl` | Outcome | Time |
|---|---|---|
| a comment added | relocated | 0.75 ms |
| one number changed | patched | 0.79 ms |
| a statement added | recompiled | 110 ms |
| `set_config_value`, file named | patched | 1.3 ms |
| `set_config_value`, file searched for | patched | 2.9 ms |

`set_config_value` costs more than a reload of the same edit because it also
makes the edit: it parses the file's text to find the literal and rewrite it
in place. Name the file when the host knows it.

The recompile itself went from about 300 ms to 110 ms in the same work:

- Lowering dropped from 225 ms to 69 ms. Copy propagation was solving its
  dataflow per instruction over hash maps, which is quadratic in a function's
  length, and a program's top level is one function. It now solves per basic
  block over bit sets (`core/src/backend/bytecode/copyprop.rs`); the old
  solver is kept as the test oracle and produces identical code for every
  function of every `.ptl` file in the repository.
- The module registry keeps the last parse of each source file
  (`module::ParseCache`), so a reload re-parses only the files whose text
  changed. Parsing was about half of a compile.

A program that is not being reloaded runs the same instructions as before:
`petal bench` reports identical instruction counts, and the benchmark programs
time within noise (see [performance.md](dev/performance.md)).

## Limits

- **A structural edit recompiles the whole program.** The diff says which
  top-level constructs changed, but there is no per-function recompile: a
  program is one term graph with program-wide term, register and constant
  numbering, and an edit to one function shifts the ids of everything compiled
  after it. Making that local needs position-independent function bodies,
  which the compiler does not produce. The recompile is cheaper than it was;
  it is not incremental.
- A value edit inside code that carries a warning recompiles.
- `set_config_value` cannot change a value's type or shape.
- The reload looks at the files the program was compiled from. A new file that
  would now win an import's resolution is not noticed until something else
  forces a recompile (a watcher over the program's own files would not report
  it either).
- A program loaded from IR JSON has no source to diff and always recompiles.
