# Goal-Based Source Editing

Goal-based editing lets a program rewrite a `.ptl` file by saying what the
result should look like, not which characters to change. You state a goal such
as "the file should call `set_color_scheme("dracula")`" and the library
finds the existing call and updates it, or appends one if there is none.
Comments, blank lines and everything else in the file stay as they were.

- **Module:** [`core/src/goal_based_editing.rs`](../../core/src/goal_based_editing.rs)
  (`petal::goal_based_editing`)
- **Built on:** the lossless CST splice primitives in
  [`core/src/rewrite.rs`](../../core/src/rewrite.rs), and the style-preserving
  literal edits in [`core/src/literal_edit.rs`](../../core/src/literal_edit.rs)
  ([source-preservation.md](source-preservation.md))
- **Reading values back out:** [../usage/config-files.md](../usage/config-files.md)
- **From C and C++:** the `pb_source_*` functions
  ([../usage/embedding-c.md](../usage/embedding-c.md#editing-source-a-script-as-a-config-file))

This page is the full description of the Goal API. How an edit keeps the
file's formatting is in [source-preservation.md](source-preservation.md).
Goals about a value a running program *emitted* ("this argument should be
55") are a different API, in
[direct-manipulation.md](direct-manipulation.md#the-goal-based-protocol).

---

## Quick example

Make `init.ptl` call `set_color_scheme("dracula")`:

```rust
use petal::goal_based_editing::{modify_source_with_goals, Goal};

let source = std::fs::read_to_string(path)?;
let goals = [Goal::should_call("set_color_scheme", ["dracula"])];
let updated = modify_source_with_goals(&source, &goals)?;
std::fs::write(path, updated)?;
```

- If the file already has a top-level `set_color_scheme("light")`, the argument
  becomes `"dracula"` and nothing else changes.
- If the file does not call `set_color_scheme`, the call is appended as a new
  top-level statement.

---

## The API

### `modify_source_with_goals(source, goals) -> Result<String, GoalError>`

Applies the goals in order and returns the rewritten source. Later goals see
the insertions made by earlier ones. On failure (the source does not parse, or
an edit was rejected) it returns a `GoalError`.

### `GoalError`

A failure to apply the goals. It carries a human-readable `message` and
implements `Display` and `std::error::Error`, so it works with `?` in any
`Box<dyn Error>` result. Convert with `.to_string()` for a `Result<_, String>`.

### `Goal`

The intent: a call (`should_call`), a binding's value (`should_set_value`), or
a place inside a binding's literal (the [path goals](#path-goals)). `Goal` is
an enum, and the place to add further intents (ensure an import, remove a
call) as they are needed.

#### `Goal::should_call(function, params)`

> The source should contain a top-level call `function(params...)`.

- If a matching call exists, its **argument list is replaced** with `params`.
- Otherwise the call is **appended** as a new top-level statement.

`function` is any `Into<String>`. `params` is any iterable of values that
convert into [`StaticValue`](#staticvalue--structured-values); `&str`,
`String`, `i32`, `i64`, `f64`, `f32` and `bool` all convert automatically.
Arguments of different types cannot share one array literal, so pass a
`Vec<StaticValue>` for those. The whole argument list is replaced: this goal
does not edit the arguments one by one.

```rust
Goal::should_call("set_color_scheme", ["dracula"]);   // set_color_scheme("dracula")
Goal::should_call("resize", [800, 600]);              // resize(800, 600)
Goal::should_call("set_scale", [1.0]);                // set_scale(1.0)
Goal::should_call("clear", Vec::<StaticValue>::new()); // clear()
```

#### `Goal::should_set_value(name, value)`

> Reading `name` out of the edited source should yield `value`.

This is the write half of [Petal as a configuration format](../usage/config-files.md).

- If `name` is bound at top level, the **right-hand side of its last binding**
  is edited to read as `value`. The last binding is the one that decides the
  program's value. Both `let name = …` and a bare `name = …` count.
- If `name` is not bound at top level, `let name = value` is **inserted**, at
  the end of the file or wherever a [placement](#placement) says.
- If reading `name` already yields `value`, **nothing is written** and the
  source comes back byte-identical.

```rust
Goal::should_set_value("color_scheme", "dracula");   // let color_scheme = "dracula"
Goal::should_set_value("font_size", 14);             // let font_size = 14
Goal::should_set_value("editor", StaticValue::record(vec![("tab_width", 4)]));
```

Everything around the value survives:

```text
before                             after
──────────────────────────────     ──────────────────────────────
// user config                     // user config
let font_size = 12 // points  →    let font_size = 14 // points
let other = 1                      let other = 1
```

Three things to know:

- **Only what differs is rewritten.** A record or list literal is edited in
  place, element by element
  ([source-preservation.md](source-preservation.md#setting-a-composite-value)).
- **A right-hand side that is not a literal of the new value's shape is
  replaced whole.** A binding that is not a literal today becomes one:
  `let font_size = if wide then 16 else 12 end` collapses to
  `let font_size = 14`. The edit is blunt, but the goal holds by construction.
- **Only top-level bindings count.** A binding inside a function body belongs
  to that body, so the goal appends a new top-level binding instead of editing
  it.

#### A goal that already holds writes nothing

If the outcome already holds, the source is returned unchanged, to the byte.
This matters for a host that writes back every field on every save: only the
lines that actually changed get touched, and a value's spelling is preserved.

```text
let drag_axial = 0.020000    →    let drag_axial = 0.020000   (unchanged: same f64)
let drag_axial = 0.020001    →    let drag_axial = 0.02       (changed: different f64)
```

The comparison is exact and on the value: an `Int` never equals a `Float` of
the same magnitude, and a binding that cannot be read statically never compares
equal, so it is collapsed to the literal.

### Path goals

A path names a place inside a binding's literal: a top-level name, then
`.field` steps into record literals (and element attributes), `[index]` steps
into list literals or the arguments of a call (`vec3(0.3, -1.0, 0.5)`), and
XPath-style `/tag[n]` steps into an element's children
(`layers[2]/group[0]/image[1].x`, 0-based). The grammar and its decisions are
in [edit-paths.md](edit-paths.md).

```rust
Goal::should_set_path("POST.effects[2].amount", 0.7);        // one number
Goal::should_set_path("POST.tint", StaticValue::color(0x29, 0xd9, 0xff));
Goal::should_insert("POST.effects[0]", effect);              // new first element
Goal::should_append("POST.effects", effect);                 // new last element
Goal::should_insert("POST.bloom", 0.4).after("exposure");    // new field, placed
Goal::should_remove("POST.effects[1]");
Goal::should_remove("POST.bloom");
Goal::should_set_path("layers[2]/image[1].x", 600);           // inside a <group>
Goal::should_move("layers[3]", "layers[1]/*[0]");            // a layer into a group
```

| Goal | Outcome |
|---|---|
| `should_set_path(path, value)` | The value at `path` reads as `value`. Only what differs is rewritten; a value that already holds writes nothing. A record field that does not exist yet is added at the end of its record. |
| `should_insert(path, value)` | `value` becomes the element or field `path` names: `xs[1]` inserts before the current element 1 (an index equal to the length appends), `rec.key` adds a field at the end, or next to a sibling with `.after(key)` / `.before(key)`. A field that already exists is set instead. |
| `should_append(path, value)` | `value` becomes the last element of the list (or call arguments) at `path`. |
| `should_remove(path)` | The element, field, attribute or child element at `path` is gone. A field that is already absent satisfies the goal. |
| `should_move(from, to)` | The list element or child element at `from` is at `to` instead, its text (comments inside included) carried over. `to` is read after `from` is taken out and may be in another list or element. A path moved onto itself changes nothing. |

These edit the literal where it stands, so comments and formatting inside a
list whose shape changes survive:

```text
before                                   after should_remove("fx[0]"), should_append("fx", …)
──────────────────────────────────       ──────────────────────────────────
let fx = [                               let fx = [
  {effect: "lens_dirt", amount: 0.5},      // grain goes last
  // grain goes last                 →     {effect: "grain", amount: 0.30},
  {effect: "grain", amount: 0.30},         {effect: "crt", amount: 0.2},
]                                        ]
```

New text is formatted like its siblings, and a changed number or color keeps
the spelling of the token it replaces. The rules, and what the edits
guarantee, are in [source-preservation.md](source-preservation.md).

A path that names nothing (`POST.effects[7].amount`, a field of a number, a
step through a computed value) is an error, as is an edit that would leave
source that does not parse (a record key that is not an identifier). The
source is returned only when every goal succeeded.

### Expression goals

Some edits mean an expression, not a value. Binding a design node to a token
is writing `fill={tok.color.ink}`, not the color the token has today; a helper
call reads best with its named arguments (`shadow(c, radius: 4, y: 3)`, not
`shadow(c, 4, nil, 0, 3)`). Every goal that takes a value has an `_expr`
twin that takes a [`StaticExpr`](#staticexpr--expressions):

```rust
use petal::goal_based_editing::{Goal, StaticExpr};

Goal::should_set_path_expr("card.fill", StaticExpr::path("tok.color.ink")?);
Goal::should_set_value_expr("s", StaticExpr::parse("shadow(c, radius: 4, y: 3)")?);
Goal::should_insert_expr("xs[0]", StaticExpr::path("gutter")?);
Goal::should_append_expr("page", element_of_exprs);
Goal::should_call_expr("theme", [StaticExpr::path("tok.dark")?], [("scale", 2.into())]);
```

| Goal | Outcome |
|---|---|
| `should_set_path_expr(path, expr)` | The value at `path` is written as `expr`. |
| `should_insert_expr(path, expr)` | `expr` becomes the element or field `path` names (placement like `should_insert`). |
| `should_append_expr(path, expr)` | `expr` becomes the last element of the list or element at `path`. |
| `should_set_value_expr(name, expr)` | The last top-level binding of `name` is written as `expr`, or `let name = expr` is inserted. |
| `should_call_expr(function, args, named)` | A top-level call with positional, then named, arguments. |

An expression that is a value (no names, no named arguments) is written by
the value goal, with all its care. Otherwise the parts that are values are
laid out and spelled the same way, and each part that is not (a name, a field
read, a named-argument call) is written as the renderer spells it. A place
already written as `expr` (compared as expressions, so `m["k"]` equals
`m.k`) is left byte-identical. Whether the names are in scope where the
expression lands is not checked here: the goal edits text and does not run
it (the programs-as-documents crate checks it, by running the edited
document).

### Record keys that are not identifiers

A record key that is not an identifier (`label.color`, `bad key`, a keyword)
is written as a quoted computed key, `{["label.color"]: :ff0000}`, by every
goal and by `StaticValue::to_source`, and read back as that key
(`get_static_value`). A path step names it the same way:
`o["label.color"]`, with `\"` and `\\` escaped inside the quotes, and a
`PathSeg::Field` whose key is not an identifier prints back in that form.

### Goals from a diff: `petal::drop_edit`

A caller that has the *new value* of a literal rather than the edit — a
`ui/drag` drop on a model that is a top-level `let` — gets the goals from
`drop_edit::diff_goals(path, old, new)`. Lists are matched element by
element on an identity (a record's `id`, else its `key`, else the element)
with a longest common subsequence, so a move is one `should_remove` and one
`should_insert` and every element that stayed keeps its text and comments;
records are diffed field by field (`should_insert` / `should_remove` for
added and dropped fields); anything else is `should_set_path`. The goals are
ordered to apply in sequence: removals back to front, insertions front to
back at their final index, then edits inside the elements that stayed.

```rust
use petal::drop_edit::{diff_goals, propose_drop};
// STEPS = ["fetch", "parse", "check"]  →  ["parse", "check", "fetch"]
// goals: [should_remove("STEPS[0]"), should_insert("STEPS[2]", "fetch")]
let edit = propose_drop(&source, &model_json, &drop_json)?;   // DropEdit {binding, goals, source}
```

`propose_drop` finds the one top-level `let` whose literal *is* the model
(`literal_bindings`; a colour literal equals its runtime `{r, g, b}`
record), diffs and applies. `state` initializers are never targets. Garden's
Petal IDE uses it to turn a drag-mode drop into an edit
(`garden/docs/petal-ide-mode.md`); the design is
[declarative-dragging.md](../proposals/declarative-dragging.md) §P4.

### Placement

Where a goal's statement goes **when one has to be inserted**. A goal that
edits an existing binding or call ignores placement; the statement stays where
its author put it.

```rust
Goal::should_set_value("tether_slack_m", 0.5).after("tether_max_m")
Goal::should_set_value("tether_slack_m", 0.5).before("tether_max_m")
Goal::should_set_value("tether_slack_m", 0.5).with_placement(Placement::End)
```

`should_call`, `should_set_value` and `should_insert` take a placement. For
`should_insert` the anchor is a sibling field of the record
([path goals](#path-goals)).

| `Placement` | |
|---|---|
| `End` (default) | append to the end of the file |
| `After(anchor)` | insert directly below the anchor's statement |
| `Before(anchor)` | insert directly above the anchor, and above its doc comment |

The anchor is a top-level binding name or a statement-position call name. An
anchor that is not in the file falls back to appending, so a placement can
misplace a statement but never lose one.

Insertion copies the spacing already in the file: a blank line between the
anchor and its neighbour means the new statement gets one too. `Before` inserts
above the anchor's comment block, so a doc comment stays with its binding.

This lets a host that generates a config file from a table keep the file's
ordering when a new field appears (anchor each field on the one before it)
without regenerating the file and losing the user's comments and layout.

### `StaticValue` — structured values

Values are **structured**, not pre-rendered source. The library renders each
one into a valid Petal literal, so quoting and escaping are handled for you and
untrusted input can never break out of a string or inject interpolation.

The type lives in [`core/src/static_value.rs`](../../core/src/static_value.rs) and
is re-exported from `goal_based_editing`. It is the same type
[`get_static_value`](../usage/config-files.md#reading) returns when reading, so a value
round-trips: read it, adjust it, write it back. The variant table and rendering
rules are in [../usage/config-files.md](../usage/config-files.md#staticvalue).

There is deliberately **no verbatim/raw-source variant**. An escape hatch that
rendered caller-supplied text unquoted would defeat the point of structured
values. Model identifiers or field access structurally with
[`StaticExpr`](#staticexpr--expressions), or add a typed variant if a case is
genuinely missing.

### `StaticExpr` — expressions

[`core/src/static_expr.rs`](../../core/src/static_expr.rs) (re-exported from
`goal_based_editing`). An expression an edit writes, as a typed tree:

| Variant | Renders |
|---|---|
| `Value(StaticValue)` | the literal |
| `Name(n)` | `n` |
| `Field { of, name }` | `of.name`, or `of["name"]` when `name` is not an identifier |
| `Index { of, index }` | `of[index]` |
| `Call { callee, args, named }` | `callee(a, b, name: c)` |
| `List`, `Record`, `Element` | literals whose parts are expressions |

Build one structurally (`StaticExpr::name`, `path("tok.color.ink")`,
`call("shadow").arg(c).named("radius", 4)`, `.field`, `.index`, the `From`
impls for scalars and `StaticValue`) or parse one with
`StaticExpr::parse(text)`. Parsing accepts exactly one expression of those
kinds: a statement, two expressions, unbalanced text, arithmetic, an
interpolated string, a lambda, `if`, `match` or a spread argument is an
`ExprError` saying what was found. Every name, field, argument name, tag and
attribute is checked when it renders, and every string is quoted by the
renderer, so text a caller passes becomes a string literal or (after the
identifier check) a name, never code.

`free_names()` lists the names it reads from the scope it lands in (the root
of each path, each callee). `as_value()` is the `StaticValue` when it is one.
`get_static_expr(source, name)` reads a top-level binding back as an
expression, which is how a call with named arguments is read (a
`StaticValue::Call` holds positional arguments only).

From C, `pb_source_set(path, text)` already writes the text of one expression
at a path (checked to parse as exactly one expression); the typed
`StaticExpr` is not exposed through the bridge.

## See also

- [../usage/config-files.md](../usage/config-files.md) — reading values out of a `.ptl` file, and
  the round-trip contract.
- [source-preservation.md](source-preservation.md) — how an edit keeps the
  file's formatting, and the guarantees it makes.
- [program-modification.md](program-modification.md) — every way a Petal
  program can be modified programmatically.
- [direct-manipulation.md](direct-manipulation.md) — goals about *emitted*
  values ("this argument should be 55"), answered with edit proposals.
