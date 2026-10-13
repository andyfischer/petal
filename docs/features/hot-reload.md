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
Cheesecake engine (35 files, 15,200 lines) a value edit takes 1.0 ms where the
recompile it replaces takes 187 ms; the numbers are under
[What it costs](#what-it-costs).

The contract is the same on every path: **afterwards the program and the stack
are what a full recompile of the new source, followed by `transfer_state`,
would have left.** The same terms, the same constants by value, the same span
for every term, the same warnings, the same bytecode, the same `state`. When
that cannot be shown for an edit, the edit is recompiled.

The C bridge's `pb_vm_reload` goes through this, as do the SDL host's watcher
and Garden's script and panel hosts. See
[../usage/embedding-c.md](../usage/embedding-c.md#hot-reload) for the C side.

## What each outcome leaves alone

| | `unchanged` / `relocated` | `patched` | `recompiled` |
|---|---|---|---|
| `state` | kept | kept, except a slot holding a function | kept where the declaration still exists, except a slot holding a function ([State across a reload](#state-across-a-reload)) |
| closures, the captured function table | kept | dropped, recaptured by the next run | dropped |
| memo records | kept | dropped | dropped |
| static values | kept | kept, except those the edit reaches | kept, except those the edit reaches |
| frame gate (`run_needed`) | unchanged: no frame is forced | the next frame runs | the next frame runs |
| bytecode | kept | kept (one operand retargeted, the first time a literal changes) | lowered again: by the reload when it follows static values, else on the next run |

A `state` whose value is a function, or holds one anywhere inside it, is a
closure of the program as it was. The two outcomes that drop closures drop
that slot with them (it counts in `state_dropped`), and the next run takes the
function from the declaration's initializer again. Before this rule the slot
was kept and calling the function failed.

After any reload the stack is ready to run from the top: `run` may follow
without a `reset_stack`.

A `patched` reload drops what a `recompiled` one drops, by running the same
code (`transfer_stack_state`). A closure captured the old value; a memo record
of a function whose body loads the constant has an input the record does not
list; a `state` slot that was *initialized* from the old value keeps its value
under both, because an initializer runs once. Static values are the
exception: both kinds of reload put back the ones the edit cannot have
changed, which is the next section.

## State across a reload

What `state` is and how a slot is identified is in
[inline-state.md](inline-state.md); which edits keep a slot, as the author of
a script sees it, is under
[State and hot reload](inline-state.md#state-and-hot-reload) there. This
section is the mechanism.

The primitive is `Env::transfer_state`
([`core/src/transfer_state.rs`](../../core/src/transfer_state.rs)): reshape a
running stack onto a freshly compiled `Program`. A `recompiled` reload ends
in it, and it is the definition of what the other outcomes must be
equivalent to. A host that compiles for itself calls it directly:

```rust
let new_program = env.compile_program_at(pid, &source, &path)?;
let result = env.transfer_state(stack, new_program)?;  // TransferStateResult { state_preserved, state_dropped }
```

It swaps the program in under the same `ProgramId` (invalidating cached
bytecode), clears closures and the stack's cached function, method and enum
tables (they point into old code and are recaptured on the next run), clears
memo records and static values, drops the state the new program no longer
declares, and resets execution so the next `run` starts from the new root.
`reload_program` then puts back the static values the edit cannot have
changed ([below](#static-values-across-a-reload)); a bare `transfer_state`
leaves every one to be rebuilt.

State is matched on the declaration only. A slot's key is its declaration
plus the call path that reached it, and the new program's declarations say
nothing about paths, so every slot under a surviving declaration is kept:

- a declaration the edit **added** is initialized on the next run;
- a declaration the edit **removed** loses its slots, counted in
  `state_dropped`;
- a declaration that is still there keeps its slots, whatever else changed.

Slots whose call path no longer occurs are not found at transfer time. The
sweep that ends every run (`Stack::sweep_untouched_state`) drops any slot the
run did not touch, and that is what removes them. So the two edits that lose
state are the same event, an identity the edit changed, and both read as a
removal plus an addition:

- Renaming a `state` variable, or the function or module it sits in, changes
  the declaration and drops the value at the transfer.
- Editing the call structure around a callsite (renaming the callee,
  inserting an earlier call to the same callee, extracting or inlining a
  helper) moves every slot below it to a new path. The reader sees a freshly
  initialized slot and the orphan is swept after the next run.

An existing slot keeps its value when its initializer is edited, because an
initializer runs once. The reload report names such a slot as a
[stale seed](#static-values-across-a-reload).

### Class instances across a reload

A class instance is an ordinary record carrying an interned class *name*, not
a pointer into the program that built it. An instance kept in `state` keeps
the fields it was constructed with, and `type()` keeps reporting its label
even if the class is gone. Petal does not migrate it: a field the edit adds
is not invented on an older instance.

Method calls on such a value resolve against the new code:

- **Pinning.** Where the compiler can tell the receiver's class (a constructor
  call, a `let` bound to one, or a class annotation), the call is bound to
  `fn Class.method` at compile time.
- **Stale-label fallback.** Otherwise the call dispatches on the label, but
  also carries the class its declaration named, which is consulted only when
  the label names no class in the running program.

See [Classes & Methods](../language-guide.md#when-the-call-is-resolved-at-compile-time)
and `core/tests/class_live_edit.rs`. Enum variants kept in `state` are under
[Editing an enum](#editing-an-enum).

## Static values across a reload

A static value is a value that depends on no dynamic data (a `const`, a table
of literals, a table built from one by functions that read nothing else),
built once and kept between runs. A reload keeps the ones the edit cannot
have changed and the next run rebuilds the rest: a value is rebuilt when the
edit touched its own statement, anything it depends on, or what one of the
names it uses means. The rule, its limits, and what is rebuilt when the diff
cannot say what changed are in
[static-values.md](static-values.md#following-edits-on-hot-reload); how the
decision is made is in
[internals/static-values.md](../internals/static-values.md#following-the-diff-on-reload).

`ReloadReport::statics` says what happened, and
`ReloadReport::static_lines()` is what the SDL host and Garden print:

```text
[hot-reload] constants: 2 rebuilt (TILES, WIDTHS), 41 kept
[hot-reload] stale seed: state grid was initialized from TILES, which changed; it keeps its value
[hot-reload] stale cache: state DEFS is filled from GENERIC, which changed, behind a test; it keeps what it built
```

A **stale seed** is a top-level `state` that kept its value although its
initializer reads something the edit changed, or was itself edited
([The reload report](static-values.md#the-reload-report)). A **stale
cache** is the same thing one step removed: a top-level `state` that code
fills behind a test, which keeps what it built from the old constant.
`Env::set_static_follow(false)` goes back to rebuilding every static value on
every reload. It is what the followed reload is tested against.

## What kind of edit is it

`petal::source_diff::diff_source(old, new, file)` parses both texts and
compares the syntax trees. It returns a `FileDiff`, whose `change` is a
`SourceChange`:

- **`None`**: the trees are the same. The texts are identical, or differ in
  whitespace, comments, blank lines, line wrapping, or a trailing comma.
- **`Values(list)`**: the trees differ only in the values of literals, each
  keeping its kind (int, float, string, bool). Every entry names the literal's
  old and new value and its span in both texts, and for a literal inside a
  top-level `let`, `const` or `config let` its binding path: `SPEED`,
  `POST.effects[2].amount`, `SUN.dir[1]` (a call's arguments count as
  elements, as in [config files](../usage/config-files.md)). A literal in a function
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

The path is a binding path as in [config files](../usage/config-files.md). The second
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
Any top-level `let` of data can be set this way; `config` is not required
(what [`config let`](../syntax/config-let.md) adds is the sign rule above).

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
drag. Each value waits on its own time: a cheap one written beside a slow
table (`let SCALED = [K * 3]` above a flood fill) is rebuilt on every tick,
whether or not the dragged literal stands alone in its declaration. A slow
table that only one other value reads is timed, and waits, with that value.

`Env::set_static_debounce(Some(StaticDebounce { threshold, quiet }))` sets
the two durations, `None` rebuilds on every tick, and
`Env::flush_static_rebuilds(stack)` rebuilds at once (for a host that knows
the mouse button went up). From C: `pb_vm_set_static_debounce(vm,
threshold_ms, quiet_ms)` (a negative threshold turns it off),
`pb_vm_flush_static_rebuilds(vm)` and `pb_vm_set_static_follow(vm, on)`.

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
holds the rule itself to 45 more cases against `set_static_follow(false)`: a
changed import, a shadowing declaration, an added overload, an added method,
a literal inside a function a constant calls, one literal followed through
17 routes to a kept table (a default parameter, a returned function,
recursion, a match guard, a method, enum data, a top-level loop), enums,
modules, values with no declaration, chains of edits, frames that raise,
collections, the drag, and which objects two values share (tables that
hold one another, frozen through arbitrary names after an edit to one of
them). `core-libs/petal-ui/tests/static_reload.rs` drives
the 55 example panels through five edits each under three policies: 275
followed reloads, 12,768 values kept and 2,250 rebuilt, every frame and
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

On `games/neon` (35 files with the packages it imports, 15,211 lines, 77,853
terms; release build, Apple silicon, the minimum of 9 rounds in each of 5
runs, 13 October 2026), and on Moses (65 files, 33,848 lines, 192,534
terms; 3 runs):

| Edit to `config.ptl` | Outcome | neon | Moses |
|---|---|---|---|
| a comment added | relocated | 0.85 ms | 1.88 ms |
| one number changed | patched | 0.99 ms | 2.75 ms |
| a statement added | recompiled | 187 ms | 488 ms |
| `set_config_value`, file named | patched | 1.04 ms | 4.08 ms |
| `set_config_value`, file searched for | patched | 2.71 ms | 4.61 ms |

The static-value cache is 31 ms of the 187 (156 ms under
`PETAL_POLICY=fast-static`) and 102 of the 488 (386 ms), and nothing that
can be measured of a relocated or patched reload. The largest part is the
classification the lowering reads: 14 ms and 47 ms, which the example prints
on its own. The values kept inside function bodies are 12 ms and 33 ms of it
(175 ms and 455 ms under `fast-fnstatic`, the cache without them). A program is classified once per recompile, whoever asks (the
`const` check, the lowering, the audit, `Env::static_report`), and not at
all by a reload that does not recompile. The example compiles without
linking, since it runs nothing, so a game whose package requires a host
interface loads in it.

A reload parses only text it has not seen. The diff needs the syntax tree of
the running text and of the new one, and the `Env` keeps the parse of the
text each reload ends on for the next: found again by the text itself, so
there is nothing to invalidate, held only for files that have been edited,
two versions of a file at most (so a reload that does not compile costs the
next one nothing), twelve texts and 2 MiB in all. `Env::clear_reload_parses`
drops them; the next reload then parses the running text of each changed
file once more.

`set_config_value` costs more than a reload of the same edit because it also
makes the edit: it parses the file's text to find the literal and rewrite it
in place. Name the file when the host knows it.

Two things keep the recompile itself down:

- Copy propagation solves its dataflow per basic block over bit sets
  (`core/src/backend/bytecode/copyprop.rs`). A program's top level is one
  function, and a solver that works per instruction over hash maps is
  quadratic in a function's length; that solver is kept as the test oracle
  and produces identical code for every function of every `.ptl` file in the
  repository. When the change was made it took lowering from 225 ms to 69 ms
  on the `games/neon` of that day.
- The module registry keeps the last parse of each source file
  (`module::ParseCache`), so a recompile re-parses only the files whose text
  changed. Parsing was about half of a compile.

`core/examples/reload_follow.rs` times a reload and the frame after it with
static values followed and not, on the static part of a program whose host
is not here (`cargo run --release --example reload_follow -- game.ptl`). On
Moses (27,000 lines, 451 kept values; release, minimum of 7 cycles, 13
October 2026), with every static value rebuilt and with the diff followed:

| Edit | Outcome | Reload, lowering and first frame, rebuilt | Followed | First frame's instructions, rebuilt | Followed |
|---|---|---|---|---|---|
| a function added to the entry file | recompiled | 151.5 + 303.3 + 0.57 ms | 452.8 + 0 + 0.36 ms | 9,218 | 2,642 (451 kept) |
| one table of a module, by shape | recompiled | 157.2 + 304.9 + 0.56 ms | 480.6 + 0 + 0.39 ms | 9,219 | 2,681 (450 kept, 1 rebuilt) |
| the same table, one number | patched | 3.09 + 0 + 0.40 ms | 3.66 + 0 + 0.41 ms | 9,217 | 2,679 |

and with its 74 constant `state` tables rewritten as `const` (548 kept
values):

| Edit | Outcome | Rebuilt | Followed | First frame's instructions, rebuilt | Followed |
|---|---|---|---|---|---|
| a function added | recompiled | 211.6 + 275.7 + 1.62 ms | 493.2 + 0 + 0.47 ms | 36,331 | 2,558 |
| one table, by shape | recompiled | 216.7 + 276.1 + 1.58 ms | 494.4 + 0 + 0.47 ms | 36,332 | 2,600 |
| the same table, one number | patched | 3.35 + 0 + 1.31 ms | 3.24 + 0 + 0.30 ms | 36,330 | 2,598 |

Read it plainly: following the diff removes 71% to 93% of the instructions
of the frame after a reload and about a millisecond of it at most, and a
recompile of this program is half a second either way. Where it shows is a
value edit, which is 3.5 to 4.7 ms end to end here and loses up to a
quarter of that, and a program whose tables are slow to build, which this
one is not. Deciding what to keep cost 0.02 ms on 426 values when it was
last timed alone (12 October).

Each value is stored on its own, so an edit to one table costs that table:
the frame after it runs 39 instructions more than a frame with everything
kept (2,681 against 2,642; 42 with the 74 tables), where it ran 214 more
while a rebuilt value took the stretch of statements around it along. The
price is on the frame that builds everything, which runs two instructions
more per stored value: 9,218 where it was 8,353.

A program that is not being reloaded runs the same instructions as before:
`petal bench` reports identical instruction counts, and the benchmark programs
time within noise (see [performance.md](../dev/performance.md)).

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

## Hosts that reload

- **`petal-sdl`** (every SDL host).
  [`watcher.rs`](../../integrations/petal-desktop-sdl/src/watcher.rs) watches
  the directory of every source file the program was compiled from
  (`Env::program_source_paths`), so editing an imported `palette.ptl` reloads
  its importer. A change calls `reload_program` and prints the outcome and
  the report's lines. A compile error is printed and the old program keeps
  running. On by default; `--no-hot-reload` turns it off.
- **The C bridge.** `pb_vm_reload`, with `pb_vm_last_reload_kind` and
  `pb_vm_last_reload_json` for what it did
  ([../usage/embedding-c.md](../usage/embedding-c.md#hot-reload)).
- **Garden.** Its script host and its panel host both call `reload_program`
  (`garden/garden-script/src/lib.rs`, `panel.rs`).
- **The browser editor of `diagram-canvas`** does not hot reload. Its
  CodeMirror widget
  ([`editor.ts`](../../examples/custom-apps/diagram-canvas/src/editor.ts))
  recompiles on a debounce with `petal.load(source)`, which makes a new
  stack: state resets and the frame count returns to 0. That editor is
  browser UI only; it is not exposed over the debug protocol or MCP.

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
