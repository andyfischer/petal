# Stable per-loop keys for `state`

Status, 2026-10-22: **done.** `for x in xs key k do` shipped; the user docs
are [inline-state.md, Keyed loops](../features/inline-state.md#keyed-loops)
and the design is [state-call-paths.md §2.2a](../dev/state-call-paths.md#22a-keyed-loops).
Opened when `state(key)` was deprecated ([inline-state.md, Deprecated:
`state(key)`](../features/inline-state.md#deprecated-statekey)). The
"Outcome" section at the end records how each question below was settled.

## The problem

A `for` or `while` loop keys each iteration's `state` by position: the
VM pushes an `Index(i)` part onto the call path
([state-call-paths.md §3.3](../dev/state-call-paths.md#33-vm)). When the
list reorders, or an item is inserted ahead of another, each item's state
stays with the position and not with the item. A hover flag or a drag ends
up on the wrong row.

Until 2026-10-22 the way around this was absolute keyed state,
`state(item.id) x = …`. It is deprecated because it is a global hidden in a
function: two calls that pass the same key share one slot. That breaks the
rule that state is nested per call path, and so stateful code is no longer
safe to duplicate.

The sites that needed identity were moved to a top-level `state var` holding
a [`Map`](../language-guide.md#maps) keyed by the id. That works, but it costs
four things:

- **Hand-written pruning.** A map entry is not dropped when a run stops
  reaching it the way an untouched slot is. Every site now cleans up by hand:
  - `core-libs/petal-ui/prelude/ui.ptl` swaps two maps each run
    (`_slider_now` / `_slider_last`);
  - `examples/productivity/settings/widgets.ptl` does the same for `anim`;
  - `examples/productivity/node-editor/app.ptl` runs `prune_scopes` each
    frame;
  - `garden/examples/panels/plant.ptl` rebuilds `next_segments` /
    `next_leaves` every frame;
  - the NES prelude compares a `seen` frame number.
- **Memo scopes are lost.** An in-place write to a shared cell makes the
  enclosing scope effectful
  ([memo-scopes.md](../dev/memo-scopes.md)). The keyed slider, `anim` and the
  node-editor scopes, and every scope around them, now run every frame
  instead of replaying.
- **The state is no longer nested.** The data moved out of the widget and
  into a module-level table, which is the shape this design is meant to make
  unnecessary for per-row state.
- **Less useful debugging output.** The debugger shows one big map, where it
  used to show slots under the row's path.

## Proposed shape

A key that **replaces the loop's `Index(i)` path part** with `Key(hash(k))`,
and leaves the rest of the path alone. The key is relative: it identifies an
item among the iterations of *this* loop at *this* call path. Two callers of
the same widget list still get separate slots, so stateful code stays safe
to duplicate.

```petal ignore
for t in tasks key t.id do
  task_row(t)        // task_row's state follows t.id, not the position
end
```

`for … key` is the spelling already sketched for P3 in
[reactive-rendering-plan.md](../dev/reactive-rendering-plan.md#p3--keyed-incremental-collections-and-a-hit-test-index).
Choose this syntax together with P3 so that the incremental collection work
can build on it later. This task needs only the path part, not the
incremental diffing.

## Questions to settle

1. **Duplicate keys in one loop run.** Two iterations with an equal key would
   share slots without any sign of it. Options: a runtime error, a warning
   plus falling back to `Index(i)` for the duplicate, or React's "warn and
   behave badly". A loud error is probably right.
2. **Key equality.** Use the content hash that `Map` and `memo` keys use
   (`value::hash_value`, consistent with `==`). Reject the same values Map
   rejects: functions, cells, pending values and NaN.
3. **`while` loops.** Only `for` has an item to key by. Either leave `while`
   positional, or allow `while … key expr`.
4. **Higher-order builtins.** `map`/`forEach` callbacks share the builtin's
   callsite today, one slot for every element
   ([inline-state.md](../features/inline-state.md#callbacks-run-at-the-builtins-callsite)).
   Should a keyed form exist (`map(xs, f, key: …)`), or should the docs keep
   pointing at `for`?
5. **`path_pop`.** A top-level accumulator written inside a keyed loop has to
   drop the `Key` part the same way it drops `Index`. Check
   `Term::path_pop` and the compiler's loop-depth count.
6. **Debug and host key strings.** Render the key value in the path and not
   only its hash (`[id=42]/row/hovered` rather than `k1234…`), so the
   debugger shows which item a slot belongs to
   ([debug-protocol.md](../dev/debug-protocol.md)).
7. **Hot reload.** The key is a value, not a source position, so a slot under
   a keyed loop survives edits the same way positional slots do. Add a test
   anyway.
8. **Lifecycle.** The sweep needs no change: an item removed from the list is
   not reached, so its subtree of slots is dropped. This is the main thing
   the hand-written pruning above is replacing.

## Work

- Parser and CST (`parse.rs` and `cst_project.rs`), AST JSON (`docs/CLI.md`),
  `petal fmt`, tree-sitter grammar, vim syntax.
- IR: the loop term carries the key block. Lowering and the VM push
  `Key(h)` in place of `Index(i)`. The `explain` trace and the IR tables in
  `docs/dev/ir-as-target.md`.
- Memo scopes: the touch capture and the record paths work on whole keys
  already. Confirm that a keyed row's scope replays after a reorder, because
  that is the payoff.
- Migrate the sites listed above back to nested state, and delete their
  pruning code:
  - the keyed slider in `ui.ptl` (the `style.key` API may then go away);
  - `anim` in `settings/widgets.ptl`;
  - node-editor `SCOPES`;
  - `reactive_ui.ptl` click counts.

  Keep `plant.ptl` on a `Map`. Its identity survives a change of recursion
  depth, which no loop key covers.
- Docs: inline-state.md ("Interaction with control flow", and the
  deprecation note's list of replacements), state-call-paths.md §2,
  writing-petal-guide.md, game-dev-guide.md, the CHANGELOG.
- Tests: reorder, insert ahead, remove, duplicate key, nested keyed loops,
  keyed loop inside a function called from two sites (stays separate),
  memo-scope replay after a reorder, `--state-storage` round-trip of a keyed
  path.

## Outcome

How the questions were settled:

1. **Duplicate keys** are a runtime error, `Duplicate loop key <value>: …`.
   The loop's cursor keeps the run's key hashes.
2. **Key equality** is `dict::key_hash`, so a key that Map rejects is
   rejected the same way ("Cannot use a function as a loop key").
3. **`while`** stays positional. `key` parses only in a `for` header.
4. **Higher-order builtins** get no keyed form. The docs point at `for`.
5. **`path_pop`** needed no change. The key overwrites the `Index` part in
   place, so the count of loop parts stays the same.
6. **Display**: `LoopKey` records the key's debug string per hash in
   `Stack::loop_key_labels`, and slots render as `[key="a"]/row/seen`.
7. **Hot reload**: a test shows that a keyed slot survives an unrelated edit.
8. **Lifecycle**: no change. A test shows that a removed item's slots are
   swept.

The IR does not use a key *block* on the loop term. A `LoopKey` term with
`inputs=[key]` goes at the head of the body block, after the key
expression's own terms, and it lowers to `Inst::LoopKey`. This way no pass
that walks `child_blocks` had to learn about a new block.

Migrations:

- `ui.ptl`: the keyed slider and its two maps are gone, and so is
  `style.key`. A caller keys its loop instead. `~/tools/petal-movies` still
  passes `{key: …}`, which is now ignored. Its sliders are not in a loop, so
  the key only reset a drag when the selected item changed.
- settings: `anim(target, rate)` is plain `state`, and the two row loops are
  keyed by setting id.
- node-editor: `scope_push` is plain `state` under `for n in nodes key n.id`,
  and `prune_scopes` is gone.
- `reactive_ui.ptl` **stays on its Map**. Its frames are iterations of one
  loop in a single run, so the memory has to be shared across iterations,
  and a key names one iteration. Its comment now says so.
