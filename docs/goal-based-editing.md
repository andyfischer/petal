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

### `modify_source_with_goals(source, goals) -> Result<String, String>`

Modifies the Petal source text using the list of goals. Returns the modified
source text.

### `Goal`

The declarative intent. Today there is one variant:

#### `Goal::should_call(function, params)`

> The source should contain a top-level call that looks like `function(params...)`.

- If a matching call exists, its **argument list is replaced** with `params`
  (the callee and the rest of the file are left alone).
- If no matching call exists, the call is **appended** as a new top-level
  statement.

`function` is any `Into<String>`. `params` is any iterable of values that convert
into [`Arg`](#arg--structured-arguments) — bare `&str`, `String`, `i32`, `i64`,
`f64`, and `bool` all coerce automatically:

```rust
Goal::should_call("set_color_scheme", ["dracula"]);   // set_color_scheme("dracula")
Goal::should_call("resize", [800, 600]);              // resize(800, 600)
Goal::should_call("set_scale", [1.0]);                // set_scale(1.0)
Goal::should_call("clear", Vec::<petal::goal_based_editing::Arg>::new()); // clear()
```

### `Arg` — structured arguments

Call arguments are **structured values**, not pre-rendered source. This module
renders each one into a valid Petal literal, so quoting and escaping are handled
for you and untrusted input can never break out of a string or inject
interpolation.

| Variant | Constructor | Renders as | Example |
|---|---|---|---|
| `Arg::Str` | `Arg::str(s)` / `"s".into()` | quoted, escaped string literal | `"dracula"` |
| `Arg::Int` | `Arg::int(n)` / `5.into()` | integer literal | `5` |
| `Arg::Float` | `Arg::float(f)` / `1.0.into()` | float literal (always has a `.`) | `1.0` |
| `Arg::Bool` | `Arg::bool(b)` / `true.into()` | `true` / `false` | `true` |
| `Arg::Nil` | `Arg::nil()` | `nil` | `nil` |
| `Arg::List` | `Arg::list(items)` | list literal | `[1, 2, 3]` |
| `Arg::Record` | `Arg::record(fields)` | record literal (keys render bare, so they must be valid identifiers) | `{ line_numbers: true }` |
| `Arg::Call` | `Arg::call(name, args)` | nested call | `editor("a.rs")` |
| `Arg::Expr` | `Arg::expr(src)` | **verbatim source** | anything |


`Arg::Expr` is the escape hatch for arguments the structured variants can't
express — identifiers, field access, operators:

```rust
Goal::should_call("theme", [Arg::expr("palette.dark")]);   // theme(palette.dark)
```

Because `Arg::Expr` is rendered verbatim, **you** are responsible for its
validity — an unparseable expr falls back to a raw string splice rather than an
error (see [Fallback behavior](#fallback-behavior)).

## See also

- [program-modification.md](program-modification.md) — the full catalogue of
  ways Petal programs can be programatically modified.
- [`rust/src/rewrite.rs`](../rust/src/rewrite.rs) — the CST splice primitives
  this module is built on.
