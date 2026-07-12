# Goal-Based Source Editing

A small Rust API for **programmatically editing Petal source** by describing the
*outcome* you want rather than the text operation to perform. You state a list of
**goals** — properties the source should satisfy — and the system inserts or
updates code to make them true, preserving the surrounding comments and layout.

- **Module:** [`rust/src/goal_based_editing.rs`](../rust/src/goal_based_editing.rs)
- **Crate path:** `petal::goal_based_editing`
- **Built on:** the lossless CST rewrite primitives in
  [`rust/src/rewrite.rs`](../rust/src/rewrite.rs)

For where this fits among Petal's other program-modification capabilities, see
[program-modification.md](program-modification.md).

---

## Why goals instead of "replace this text"

A config file — say Garden's `~/.garden/init.ptl` — is *also* a document the user
edits by hand. A menu action that wants to change the color scheme shouldn't care
whether `set_color_scheme(...)` is already present, how it's indented, or what
comments sit around it. It just wants the end state: "there is a call to
`set_color_scheme` with this argument."

That's a **goal**. You declare it; the module figures out whether to update an
existing call in place or append a new one, and does so without disturbing the
rest of the file.

---

## Quick start

```rust
use petal::goal_based_editing::{modify_source_with_goals, Goal};

let source = std::fs::read_to_string("~/.garden/init.ptl")?;

let goals = [Goal::should_call("set_color_scheme", ["dracula"])];
let updated = modify_source_with_goals(&source, &goals)?;

std::fs::write("~/.garden/init.ptl", updated)?;
```

- If `init.ptl` already has a top-level `set_color_scheme("light")`, its argument
  becomes `"dracula"` and everything else in the file is untouched.
- If it has no such call, `set_color_scheme("dracula")` is appended as a new
  top-level statement.

The `&str` `"dracula"` is automatically wrapped as a **string argument** and
rendered as the quoted, escaped literal `"dracula"` — you never build Petal
source text by hand.

---

## The API

### `modify_source_with_goals(source, goals) -> Result<String, String>`

Applies `goals` to `source` **in order**, returning the rewritten source. Each
goal sees the output of the previous one, so a later goal can update a call an
earlier goal inserted. Any goal returning an error aborts the whole batch (the
result is only returned on full success).

### `Goal`

The declarative intent. Today there is one variant:

#### `Goal::should_call(function, params)`

> The source should contain a top-level call `function(params...)`.

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

Arguments of **different types** can't share one array literal (Rust arrays are
homogeneous), so use a `Vec<Arg>` with the `Arg` constructors for mixed calls:

```rust
use petal::goal_based_editing::Arg;
Goal::should_call("configure", vec![Arg::str("dark"), Arg::bool(true)]);
// configure("dark", true)
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

An `f32` also coerces via `From` (round-tripped through its shortest display
form, so `0.7f32` renders as `0.7`, not `0.7000000298023224`).

The composite variants (`List`/`Record`/`Call`) nest arbitrarily, so a whole
declarative tree can be expressed as one goal — this is how Garden persists a
runtime layout change back to `init.ptl`:

```rust
Goal::should_call("layout", [Arg::call("row", vec![
    Arg::list([
        Arg::call("editor", vec![Arg::str("a.rs"),
                                 Arg::record(vec![("line_numbers", Arg::bool(true))])]),
        Arg::call("editor", vec![Arg::str("b.md")]),
    ]),
    Arg::list([0.6f32, 0.4f32]),
])]);
```

renders (and splices in) as:

```petal
layout(row([
    editor("a.rs", { line_numbers: true }),
    editor("b.md"),
  ], [0.6, 0.4]))
```

**Pretty-printing rule:** a list whose elements are all scalars renders inline
(`[0.6, 0.4]`); a list containing any composite element (call, list, record)
renders one element per line, indented two spaces per nesting level — so
generated layout trees read like hand-written config. Records always render
inline.

`Arg::Expr` is the escape hatch for arguments the structured variants can't
express — identifiers, field access, operators:

```rust
Goal::should_call("theme", [Arg::expr("palette.dark")]);   // theme(palette.dark)
```

Because `Arg::Expr` is rendered verbatim, **you** are responsible for its
validity — an unparseable expr falls back to a raw string splice rather than an
error (see [Fallback behavior](#fallback-behavior)).

#### String escaping

`Arg::Str` produces a double-quoted literal with `\`, `"`, and the interpolation
opener `{` escaped (plus `\n`/`\t`), so no character of the content can change how
the literal parses. For example, the value `a"b\c{d}` renders as:

```petal
"a\"b\\c\{d}"
```

which compiles and evaluates back to exactly `a"b\c{d}`. This is why passing a
user-supplied color-scheme name (or any external string) through `Arg::Str` is
safe.

---

## Composing goals

Pass several goals in one call; they apply left to right:

```rust
let goals = [
    Goal::should_call("set_color_scheme", ["dracula"]),
    Goal::should_call("set_font_size", [14]),
];
let updated = modify_source_with_goals(&source, &goals)?;
```

Starting from `set_color_scheme("light")\n`, this yields:

```petal
set_color_scheme("dracula")

set_font_size(14)
```

---

## What is (and isn't) matched

`ShouldCall` matches a call only when it is:

1. **top-level** — a statement in the file's top scope (not inside a function,
   loop, or `if`), and
2. **statement-position** — the whole statement is the call, not a call nested in
   a larger expression (`x = set_color_scheme(...)` is not matched), and
3. **a bare-identifier callee** — `set_color_scheme(...)`, not
   `theme.set_color_scheme(...)` or a module-qualified call.

The **first** matching call in source order is the one updated. These rules fit
declarative config files, where each setter is its own top-level line. If a call
doesn't match under these rules, the goal treats it as absent and **appends a new
call** — so a nested or namespaced call with the same name will result in a
duplicate top-level statement, not an edit of the nested one.

---

## Fallback behavior

Updating an existing call prefers a **lossless tree splice** — the replacement is
parsed and spliced into the syntax tree, so comments and layout around the call
survive. If the rendered call doesn't parse as a single expression (only
reachable via a malformed `Arg::expr`), the module falls back to a **string-level
splice** that replaces the call's character span verbatim. It never fails just
because an `Arg::expr` was malformed; the malformed text lands in the output for
you to notice.

Structured `Arg` variants (scalars and `List`/`Record`/`Call` trees built from
them) always render to valid source, so they always take the clean tree-splice
path. The one caveat is `Arg::Record` keys, which render bare and so must be
valid Petal identifiers.

---

## Verifying an edit

The module performs a mechanical rewrite; it does **not** semantically validate
the result (unlike `petal lint --fix`, which is gated on IR-equivalence). If you
want a safety net before writing the file back, run the result through the
compiler's front end:

```rust
// pseudocode: reject the edit if it no longer compiles
if petal_check(&updated).is_err() {
    // keep the original, surface an error to the user
}
```

From the CLI that's `petal check <file>` (lex + parse + compile, no execution).

---

## Extending the system

`Goal` is the extension point. New intents are added as enum variants plus a
branch in `apply_goal`:

- `EnsureImport { module, .. }` — add an `import` if absent.
- `RemoveCall { function }` — delete a top-level call.
- `SetField { record, key, value }` — set a field on a record literal.

The longer-term direction is goals that reason about the **dataflow graph**, not
just source calls (see the roadmap notes in
[program-modification.md](program-modification.md)) — e.g. "this output should
equal *X*, adjust the source constants that feed it." The `Goal` vocabulary is
where those richer, eventually graph-derived intents will live.

---

## See also

- [program-modification.md](program-modification.md) — the full catalogue of
  ways Petal programs can be modified (source, IR, and live/running).
- [rebind-operator.md](rebind-operator.md) — the `@` source desugar, a related
  syntactic rewrite.
- [`rust/src/rewrite.rs`](../rust/src/rewrite.rs) — the CST splice primitives
  this module is built on.
