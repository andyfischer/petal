# Goal-Based Source Editing

Goal-based editing lets a program rewrite a `.ptl` file by saying what the
result should look like, not which characters to change. You state a goal such
as "the file should call `set_color_scheme("dracula")`" and the library
finds the existing call and updates it, or appends one if there is none.
Comments, blank lines and everything else in the file stay as they were.

- **Module:** [`core/src/goal_based_editing.rs`](../core/src/goal_based_editing.rs)
  (`petal::goal_based_editing`)
- **Built on:** the lossless CST splice primitives in
  [`core/src/rewrite.rs`](../core/src/rewrite.rs), and the style-preserving
  literal edits in [`core/src/literal_edit.rs`](../core/src/literal_edit.rs)
  ([source-preservation.md](source-preservation.md))
- **Reading values back out:** [config-files.md](config-files.md)

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
a place inside a binding's literal (the [path goals](#path-goals)).

#### `Goal::should_call(function, params)`

> The source should contain a top-level call `function(params...)`.

- If a matching call exists, its **argument list is replaced** with `params`.
- Otherwise the call is **appended** as a new top-level statement.

`function` is any `Into<String>`. `params` is any iterable of values that
convert into [`StaticValue`](#staticvalue--structured-values); `&str`,
`String`, `i32`, `i64`, `f64`, `f32` and `bool` all convert automatically:

```rust
Goal::should_call("set_color_scheme", ["dracula"]);   // set_color_scheme("dracula")
Goal::should_call("resize", [800, 600]);              // resize(800, 600)
Goal::should_call("set_scale", [1.0]);                // set_scale(1.0)
Goal::should_call("clear", Vec::<StaticValue>::new()); // clear()
```

#### `Goal::should_set_value(name, value)`

> Reading `name` out of the edited source should yield `value`.

This is the write half of [Petal as a configuration format](config-files.md).

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
  place: unchanged fields and elements keep their text and comments, a changed
  scalar is one token, and added or removed elements are inserted and removed
  individually, formatted like their siblings. See
  [source-preservation.md](source-preservation.md).
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
`.field` steps into record literals and `[index]` steps into list literals or
the arguments of a call (`vec3(0.3, -1.0, 0.5)`).

```rust
Goal::should_set_path("POST.effects[2].amount", 0.7);        // one number
Goal::should_set_path("POST.tint", StaticValue::color(0x29, 0xd9, 0xff));
Goal::should_insert("POST.effects[0]", effect);              // new first element
Goal::should_append("POST.effects", effect);                 // new last element
Goal::should_insert("POST.bloom", 0.4).after("exposure");    // new field, placed
Goal::should_remove("POST.effects[1]");
Goal::should_remove("POST.bloom");
```

| Goal | Outcome |
|---|---|
| `should_set_path(path, value)` | The value at `path` reads as `value`. Only what differs is rewritten; a value that already holds writes nothing. A record field that does not exist yet is added at the end of its record. |
| `should_insert(path, value)` | `value` becomes the element or field `path` names: `xs[1]` inserts before the current element 1 (an index equal to the length appends), `rec.key` adds a field at the end, or next to a sibling with `.after(key)` / `.before(key)`. A field that already exists is set instead. |
| `should_append(path, value)` | `value` becomes the last element of the list (or call arguments) at `path`. |
| `should_remove(path)` | The element or field at `path` is gone. A field that is already absent satisfies the goal. |

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

New text copies the formatting of its siblings (one line or several, padding,
separators, trailing comma, indentation); a changed number or color keeps the
spelling habits of the token it replaces. The rules, and what the edits
guarantee, are in [source-preservation.md](source-preservation.md).

A path that names nothing (`POST.effects[7].amount`, a field of a number, a
step through a computed value) is an error, as is an edit that would leave
source that does not parse (a record key that is not an identifier). The
source is returned only when every goal succeeded.

### Placement

Where a goal's statement goes **when one has to be inserted**. A goal that
edits an existing binding or call ignores placement; the statement stays where
its author put it.

```rust
Goal::should_set_value("tether_slack_m", 0.5).after("tether_max_m")
Goal::should_set_value("tether_slack_m", 0.5).before("tether_max_m")
```

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

The type lives in [`core/src/static_value.rs`](../core/src/static_value.rs) and
is re-exported from `goal_based_editing`. It is the same type
[`get_static_value`](config-files.md#reading) returns when reading, so a value
round-trips: read it, adjust it, write it back. The variant table and rendering
rules are in [config-files.md](config-files.md#staticvalue).

There is deliberately **no verbatim/raw-source variant**. An escape hatch that
rendered caller-supplied text unquoted would defeat the point of structured
values. Model identifiers or field access structurally, or add a typed variant
if a case is genuinely missing.

## See also

- [config-files.md](config-files.md) — reading values out of a `.ptl` file, and
  the round-trip contract.
- [source-preservation.md](source-preservation.md) — how an edit keeps the
  file's formatting, and the guarantees it makes.
- [program-modification.md](program-modification.md) — every way a Petal
  program can be modified programmatically.
- [direct-manipulation.md](direct-manipulation.md) — goals about *emitted*
  values ("this argument should be 55"), answered with edit proposals.
