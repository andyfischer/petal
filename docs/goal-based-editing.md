# Goal-Based Source Editing

The goal-based editing system is a programmatic API for modifying Petal source
code in a declarative way. The API is designed so the client specifies the
**outcome** of the edit operation. This system is easier and more high-level
compared to text or AST based edit operations.

- **Module:** [`rust/src/goal_based_editing.rs`](../rust/src/goal_based_editing.rs)
- **Crate path:** `petal::goal_based_editing`
- **Built on:** the lossless CST rewrite primitives in
  [`rust/src/rewrite.rs`](../rust/src/rewrite.rs)

---

## Quick example

This short example modifies the init.ptl script to call `set_color_scheme("dracula")`:

```rust
use petal::goal_based_editing::{modify_source_with_goals, Goal};

let source = std::fs::read_to_string("~/.garden/init.ptl")?;

let goals = [Goal::should_call("set_color_scheme", ["dracula"])];
let updated = modify_source_with_goals(&source, &goals)?;

std::fs::write("~/.garden/init.ptl", updated)?;
```

- If `init.ptl` already has a top-level `set_color_scheme("light")`, then this
  process will modify the existing argument to `"dracula"`, and everything else
  in the file is untouched.
- If the file doesn't call `set_color_scheme` yet, then `set_color_scheme("dracula")`
  is appended as a new top-level statement.

---

## The API

### `modify_source_with_goals(source, goals) -> Result<String, GoalError>`

Modifies the Petal source text using the list of goals. On success returns the
modified source text (`Ok(String)`); on failure returns a [`GoalError`](#goalerror)
(the source didn't parse, or an edit was rejected). The distinct `Ok`/`Err`
types make the outcome unambiguous — the `Ok` string is the *only* output, and
an error is never mistaken for it.

### `GoalError`

A failure to apply the goals, wrapping a human-readable `message`. It implements
`Display` and `From<GoalError> for String`, so callers that thread a
`Result<_, String>` can keep using `?`.

### `Goal`

The declarative intent. There are two variants:

#### `Goal::should_call(function, params)`

> The source should contain a top-level call that looks like `function(params...)`.

- If a matching call exists, its **argument list is replaced** with `params`
  (the callee and the rest of the file are left alone).
- If no matching call exists, the call is **appended** as a new top-level
  statement.

`function` is any `Into<String>`. `params` is any iterable of values that convert
into [`StaticValue`](#staticvalue--structured-values) — bare `&str`, `String`,
`i32`, `i64`, `f64`, and `bool` all coerce automatically:

```rust
Goal::should_call("set_color_scheme", ["dracula"]);   // set_color_scheme("dracula")
Goal::should_call("resize", [800, 600]);              // resize(800, 600)
Goal::should_call("set_scale", [1.0]);                // set_scale(1.0)
Goal::should_call("clear", Vec::<petal::goal_based_editing::StaticValue>::new()); // clear()
```

#### `Goal::should_set_value(name, value)`

> Reading `name` out of the edited source should yield `value`.

The write half of [Petal as a configuration format](config-files.md).

- If `name` is bound at top level, the **right-hand side of its last binding** is
  replaced (the last binding is the one that decides the program's value). Both
  `let name = …` and a bare `name = …` rebinding count.
- If `name` isn't bound at top level, `let name = value` is **appended**.

```rust
Goal::should_set_value("color_scheme", "dracula");   // let color_scheme = "dracula"
Goal::should_set_value("font_size", 14);             // let font_size = 14
Goal::should_set_value("editor", StaticValue::record(vec![("tab_width", 4)]));
```

Everything around the value survives — the `let`, the name, comments,
indentation, and every other statement:

```text
before                             after
──────────────────────────────     ──────────────────────────────
// user config                     // user config
let font_size = 12 // points  →    let font_size = 14 // points
let other = 1                      let other = 1
```

Because the *whole* right-hand side is replaced, a binding that isn't a literal
today still becomes one: `let font_size = if wide then 16 else 12 end` collapses
to `let font_size = 14`. That is the blunt-but-correct static change; richer
resolutions (editing the branch actually taken rather than flattening) are the
next step for this goal, in `ensure_binding`.

Only **top-level** bindings are considered — one inside a function body belongs
to that body's scope, so the goal appends a new top-level binding instead of
editing it.

### `StaticValue` — structured values

Values are **structured**, not pre-rendered source. This module renders each one
into a valid Petal literal, so quoting and escaping are handled for you and
untrusted input can never break out of a string or inject interpolation.

| Variant | Constructor | Renders as | Example |
|---|---|---|---|
| `StaticValue::Str` | `StaticValue::str(s)` / `"s".into()` | quoted, escaped string literal | `"dracula"` |
| `StaticValue::Int` | `StaticValue::int(n)` / `5.into()` | integer literal | `5` |
| `StaticValue::Float` | `StaticValue::float(f)` / `1.0.into()` | float literal (always has a `.`) | `1.0` |
| `StaticValue::Bool` | `StaticValue::bool(b)` / `true.into()` | `true` / `false` | `true` |
| `StaticValue::Nil` | `StaticValue::nil()` | `nil` | `nil` |
| `StaticValue::List` | `StaticValue::list(items)` | list literal | `[1, 2, 3]` |
| `StaticValue::Record` | `StaticValue::record(fields)` | record literal (keys render bare, so they must be valid identifiers) | `{ line_numbers: true }` |
| `StaticValue::Call` | `StaticValue::call(name, args)` | call | `editor("a.rs")` |

Every variant renders to well-formed Petal, so a rewritten statement always
parses. There is deliberately **no verbatim/raw-source variant** — an escape
hatch that rendered caller-supplied text unquoted would defeat the point of
structured values (injection, unbalanced delimiters). Express identifiers or
field access by modeling them structurally, or add a new typed variant if a case
is genuinely missing.

The type lives in [`rust/src/static_value.rs`](../rust/src/static_value.rs) and is
re-exported here. It is the **same** type
[`get_static_value`](config-files.md#reading) returns when reading a value out of
source, so a value round-trips: read it, adjust it, write it back.

## See also

- [config-files.md](config-files.md) — using a `.ptl` file as a config format:
  the reading side, and the round-trip contract.
- [program-modification.md](program-modification.md) — the full catalogue of
  ways Petal programs can be programatically modified.
- [`rust/src/rewrite.rs`](../rust/src/rewrite.rs) — the CST splice primitives
  this module is built on.
