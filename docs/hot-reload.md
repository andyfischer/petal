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
| static values | kept | kept, except those the edit reaches | kept, except those the edit reaches |
| frame gate (`run_needed`) | unchanged: no frame is forced | the next frame runs | the next frame runs |
| bytecode | kept | kept (one operand retargeted, the first time a literal changes) | lowered again: by the reload when it follows static values, else on the next run |

After any reload the stack is ready to run from the top: `run` may follow
without a `reset_stack`.

A `patched` reload drops what a `recompiled` one drops, by running the same
code (`transfer_stack_state`). A closure captured the old value; a memo record
of a function whose body loads the constant has an input the record does not
list; a `state` slot that was *initialized* from the old value keeps its value
under both, because an initializer runs once. Static values are the
exception: both kinds of reload put back the ones the edit cannot have
changed, which is the next section.

## Static values across a reload

A static value is a top-level value that depends on no dynamic data, built
once and kept between runs ([performance.md](dev/performance.md#static-values)).
A reload keeps the ones the edit cannot have changed and rebuilds the rest
on the next run. The rule is asked in terms of where things are written
(`core/src/env/static_follow.rs`):

1. The diff says which top-level statements of each file changed, appeared
   or went away, and for a value edit which statement each literal is in.
2. A declaration is **changed** when it is written inside one of those
   statements. A literal inside a function body changes the function; a
   method inside a class changes the class's declarations.
3. A static value is **kept** when it is not written inside a changed
   statement, none of the declarations it depends on is changed, and (after
   a recompile) the new program has a value for a declaration of the same
   module-qualified name that depends on the same names, none of them
   changed there either. The dependency set is the classifier's, closed
   transitively (`petal show-static --json`, `depends_on`): the `let`s,
   functions and enums a value's expression reaches, every method of a name
   it calls, every enum whose data a field read may reach.

The comparison of names is what catches an edit that changes what a name
means without touching the value's own text. A new import, a declaration
that shadows a builtin or a module's export, an added overload and an added
method each add a name to the set or change a declaration in it.

A value with no declaration of its own (the `[10, 20, 30]` written inside a
call) is matched across a recompile by the text of its statement and the
shape the statement compiled to, and follows the same rule.

Everything is rebuilt, as before this existed, when the diff cannot say what
changed: the new text was not classifiable (`change: full`), top-level
statements changed order, a source file could not be read, or the program's
set of source files changed (an import added or removed).

**A rebuilt value that is the value it was keeps the old heap value.** The
store compares the two (`static_cache::identical`: the same kinds, numbers
bit for bit so `1` is not `1.0`, records with the same keys in the same
order, the same answer to `frozen`), and on a match the slot and the
register take the old one. An edit to a helper function that leaves a table
as it was costs the rebuild and changes nothing else.

**The report.** `ReloadReport::statics` says what happened, and
`ReloadReport::static_lines()` is what the SDL host and Garden print:

```text
[hot-reload] constants: 2 rebuilt (TILES, WIDTHS), 41 kept
[hot-reload] stale seed: state grid was initialized from TILES, which changed; it keeps its value
```

Nothing is printed for a reload that reached no constant and left no stale
seed. A **stale seed** is a top-level `state` cell that kept its value
although its initializer reads something the edit changed, or was itself
edited. That has always been how `state` behaves; the line makes it
visible when a table reloads and a cell built from it does not. From C the
same is under `statics` in `pb_vm_last_reload_json`. The report is absent
(`None`) when the program does not run with the static-value cache (the
`baseline` and `explain` policies, a host that records observations such as a
Garden panel) or has not run yet.

`Env::set_static_follow(false)` goes back to rebuilding every static value
on every reload. It is what the followed reload is tested against.

Two things to know. A reload that follows static values lowers the new
program itself, so that it can say what was kept; that work was the next
frame's before, and is the same amount. And the unit of rebuilding is a run
of static statements, not one declaration: when one value of a run is
rebuilt the others of that run are evaluated again and discarded.

## What kind of edit is it

`petal::source_diff::diff_source(old, new, file)` parses both texts and
compares the syntax trees. The result is a `SourceChange`:

- **`None`**: the trees are the same. The texts are identical, or differ in
  whitespace, comments, blank lines, line wrapping, or a trailing comma.
- **`Values(list)`**: the trees differ only in the values of literals, each
  keeping its kind (int, float, string, bool). Every entry names the literal's
  old and new value and its span in both texts, and for a literal inside a
  top-level `let`, `const` or `config let` its binding path: `SPEED`,
  `POST.effects[2].amount`, `SUN.dir[1]` (a call's arguments count as
  elements, as in [config files](config-files.md)). A literal in a function
  body or a statement is a value change too, with no path.
- **`Constructs(list)`**: the structure changed. Each entry names a top-level
  construct (`fn draw`, `config let GEN`, `const TILES`, `state score`, an
  import, a bare statement) and whether it was added, removed or changed.
  Changing a `let` to a `const` or back is one of these: the keyword adds or
  removes a compile-time check, so the program is recompiled.
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

### A slow table under a drag

Each tick of a drag rebuilds the static values built from the dragged
literal. When one of them is slow to build (a table derived by a flood fill,
say), that is a slow frame per tick. So while `set_config_value` is called
continuously, a static value whose last build took 4 ms or more keeps the
value it had and is rebuilt once the drag pauses: by the first frame that
starts 150 ms after the last tick (`run_needed` asks for that frame), or by
the next reload. The first tick of a drag always rebuilds. The dragged
value itself never waits: its slot is given the new literal directly.

While a rebuild waits, the program runs with the new literal everywhere
except in the waiting tables, which is the point and also an inconsistency;
`ReloadReport::statics.deferred` names them, and the report line reads
`1 waiting for the drag to pause (GRID)`. Cheap values built from a waiting
one are rebuilt each tick from its old value and once more with it when it
is finally rebuilt, so nothing built from a replaced value outlives the
drag. The unit is again the run: a cheap value that shares a run of
statements with a slow one waits with it.

`Env::set_static_debounce(Some(StaticDebounce { threshold, quiet }))` sets
the two durations, `None` rebuilds on every tick, and
`Env::flush_static_rebuilds(stack)` rebuilds at once (for a host that knows
the mouse button went up).

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

Every case also compares the static values the two sides hold after each
frame: `reload_program` keeps the ones the edit cannot have changed,
`transfer_state` empties every one, and a slot the full side has filled must
hold the same value on the incremental side. `core/tests/static_reload.rs`
holds the rule itself to 23 more cases against `set_static_follow(false)`: a
changed import, a shadowing declaration, an added overload, an added method,
a literal inside a function a constant calls, one literal followed through
17 routes to a kept table (a default parameter, a returned function,
recursion, a match guard, a method, enum data, a top-level loop), enums,
modules, values with no declaration, chains of edits, frames that raise,
collections, and the drag. `core-libs/petal-ui/tests/static_reload.rs` drives
the 55 example panels through five edits each under three policies: 275
followed reloads, 11,352 values kept and 1,752 rebuilt, every frame and
every table equal to the reload that rebuilds everything.

Two sweeps cover real programs. The default run applies a layout edit, an
indentation-flattening edit, a number edit and an added declaration to every
console example, runs two frames before and after, and compares the static
values. The ignored one does the same over any tree (set
`PETAL_RELOAD_NO_RUN=1` for a tree whose scripts do not end):

```bash
cd core
PETAL_RELOAD_CORPUS=/path/to/scripts cargo test --release --test hot_reload -- --ignored
```

Over the 403 `.ptl` files of this repository it reports 264 edits relocated,
128 patched and 218 recompiled (the added declaration always recompiles);
over 343 files of the Cheesecake games, worlds-fair, `~/.garden`, the
website and `~/tools`, 160, 9 and 154. Every one is identical to the
recompile and leaves the same static values (12 October 2026).

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

`core/examples/reload_follow.rs` times a reload and the frame after it with
static values followed and not, on the static part of a program whose host
is not here (`cargo run --release --example reload_follow -- game.ptl`). On
Moses (27,000 lines, 426 kept values; release, minimum of 7 cycles, 12
October 2026), with every static value rebuilt and with the diff followed:

| Edit | Outcome | Reload, lowering and first frame, rebuilt | Followed | First frame's instructions, rebuilt | Followed |
|---|---|---|---|---|---|
| a function added to the entry file | recompiled | 153.3 + 388.5 + 0.62 ms | 533.4 + 0 + 0.35 ms | 8,353 | 2,675 (426 kept) |
| one table of a module, by shape | recompiled | 156.6 + 383.9 + 0.59 ms | 544.1 + 0 + 0.38 ms | 8,354 | 2,889 (425 kept, 1 rebuilt) |
| the same table, one number | patched | 5.97 + 0 + 0.45 ms | 5.99 + 0 + 0.29 ms | 8,352 | 2,887 |

and with its 74 constant `state` tables rewritten as `const` (529 kept
values):

| Edit | Outcome | Rebuilt | Followed | First frame's instructions, rebuilt | Followed |
|---|---|---|---|---|---|
| a function added | recompiled | 219.0 + 261.7 + 1.39 ms | 485.0 + 0 + 0.42 ms | 35,292 | 2,567 |
| one table, by shape | recompiled | 223.5 + 262.1 + 1.42 ms | 486.8 + 0 + 0.42 ms | 35,293 | 2,781 |
| the same table, one number | patched | 5.91 + 0 + 1.31 ms | 5.84 + 0 + 0.32 ms | 35,291 | 2,779 |

Read it plainly: following the diff removes 65% to 93% of the instructions
of the frame after a reload and about a millisecond of it at most, and a
recompile of this program is half a second either way. Where it shows is a
value edit, which is 6 to 7 ms end to end here and loses a sixth of that,
and a program whose tables are slow to build, which this one is not.
Deciding what to keep costs under 0.1 ms on 426 values.

A program that is not being reloaded runs the same instructions as before:
`petal bench` reports identical instruction counts, and the benchmark programs
time within noise (see [performance.md](dev/performance.md)).

## Editing an enum

A variant value is its tag (`Kind.park`) and its payload, and nothing else.
What its enum declares — the constant data behind `k.shape`, the names of the
payload's fields, which variants there are — is read from the program that is
running now. So a variant kept in `state` across an edit is the old value read
through the new declaration:

| Edit | Outcome | A variant already in `state` |
|---|---|---|
| A literal in constant data (`shape: "area"` to `"green"`) | patched | answers with the new data from the next run |
| A variant added, the variants reordered, a data key added | recompiled | unchanged; it answers the new key, and `Kind.all` follows the declaration |
| Its variant removed | recompiled | still prints as `Kind.park`, still answers `name` and `type()`, equals no variant the enum has now; a data read is the runtime error `No field 'shape' on enum variant Kind.park`, and a `match` with no `when _ ->` is `No matching pattern for value: Kind.park` |
| Its payload fields renamed, added or removed | recompiled | the payload is positional: it answers the new names as far as it reaches, a field it never had is absent, and a pattern for the new shape does not match it |
| Its enum renamed or removed | recompiled | keeps the old tag, so it is a variant of an enum that no longer exists |

Static constant data is kept between runs like any other static value
(`core/src/static_cache.rs`), and across a reload under the same rule: an
edit that reaches the enum (its data, its variants, a function its data
calls) rebuilds its records on the next run, and any other edit leaves them,
published again by the reload itself. Every static value that holds or reads
a variant depends on the enum, so it is rebuilt with it. The table above
holds with the cache on and off.

None of these is a crash. The stale value is not rewritten or dropped either:
`state` is the program's memory, and putting the variant back makes the same
value whole again. A program that must survive such an edit gives its `match`
a `when _ ->` arm and reads data with `?.` or `??`; otherwise the error names
the value and the line, and the fix is to reset that state.

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
