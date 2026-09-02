# Petal as a Configuration Format

A `.ptl` file can serve as an application's **configuration file**. The file is
mostly `let` bindings. The host reads the values out of it, and writes changes
back when the user flips a setting in the UI.

```petal
// ~/.garden/config.ptl

let color_scheme = "dracula"
let font_size    = 14
let editor       = { line_numbers: true, tab_width: 4 }
let accent       = rgb(255, 128, 0)
let recent       = ["a.rs", "b.rs"]
```

This is ordinary Petal source, not a separate dialect. The point of using Petal
rather than TOML or JSON is that the same file can grow into real code (a
function, an `if`, an imported module) without the user migrating to another
format.

Two APIs cover the round trip:

| Direction | API | Module |
|---|---|---|
| **Read** | `get_static_value(source, name)` / `static_values(source)` / `static_bindings(source)` | [`rust/src/static_value.rs`](../rust/src/static_value.rs) |
| **Write** | `Goal::should_set_value(name, value)` | [`rust/src/goal_based_editing.rs`](../rust/src/goal_based_editing.rs) |

Both use the same type, [`StaticValue`](#staticvalue), so a value read out of a
file can be adjusted and written straight back.

---

## Reading

```rust
use petal::static_value::{get_static_value, static_values, StaticValue};

let source = std::fs::read_to_string("config.ptl")?;

let scheme = get_static_value(&source, "color_scheme")?;   // StaticValue::Str("dracula")
let size   = get_static_value(&source, "font_size")?;      // StaticValue::Int(14)

// Or read the whole file at once:
let all = static_values(&source)?;                         // BTreeMap<String, StaticValue>
```

**Nothing runs.** There is no `Env`, no heap, no stack. The source is parsed
and the binding's right-hand side is evaluated statically. A config file cannot
print, allocate, or loop forever just by being read, and reading is cheap
enough to do on every access.

### Which binding wins

The **last** top-level binding of the name, since that is the value the program
would end up with. Both binding forms count:

```petal
let font_size = 12
font_size = 14        // ← this is what get_static_value returns
```

Only **top-level** bindings are configuration. A binding inside a function body
or a loop belongs to that scope and is invisible to the reader.

### What counts as static

| Static | Not static |
|---|---|
| `"a"`, `14`, `1.5`, `true`, `nil` | `12 + 2` (arithmetic) |
| `-3`, `not flag` (folded) | `"size {n}"` (interpolation) |
| `[1, 2]`, `{ k: "v" }` | `if wide then 16 else 12 end` |
| `rgb(255, 0, 0)` — see below | `sizes[0]`, `config.size` |
| a reference to a name bound statically **above** it | `fn` and `state` declarations |

**A call is static.** `rgb(255, 0, 0)` reads back as
`StaticValue::Call { function: "rgb", args: [...] }`, held **unevaluated**. In
a config file a call names a constructor the *host* interprets (a color, a
layout, a pane), not a computation to run. Interpret it yourself, or write it
back unchanged.

### Errors

`get_static_value` distinguishes three failures:

| Variant | Meaning | Typical response |
|---|---|---|
| `StaticValueError::Parse` | the file does not parse | surface it; the config is broken |
| `StaticValueError::NotFound` | no top-level binding of that name | fall back to the default |
| `StaticValueError::NotStatic` | bound, but needs running to know | fall back, and maybe warn |

`NotFound` and `NotStatic` are separate on purpose. `fn font_size() … end` or
`state font_size = 14` report as `NotStatic`, not `NotFound`, because the name
*is* in the file; telling the user it is missing would misdirect them.

`static_values` **omits** non-static bindings rather than failing, so a config
file that also declares functions still yields its readable settings.

### Reading the whole file, including what it cannot read

When `static_values` omits a binding, a host cannot tell "you wrote
`walk_speed` in a form I can't read" from "the file never mentions
`walk_speed`". `static_bindings` returns everything:

```rust
use petal::static_value::static_bindings;

for binding in static_bindings(&source)? {
    match binding.value {
        Ok(value) => apply(&binding.name, value),
        Err(reason) => warn!("`{}` is not static: {reason}", binding.name),
    }
}
```

Each `StaticBinding` carries:

| Field | |
|---|---|
| `name` | the bound name |
| `value` | `Ok(StaticValue)`, or `Err(reason)` — the same phrase `NotStatic` carries |
| `text` | the right-hand side **exactly as written** (`None` for a `fn`/`state` declaration) |
| `comment` | the comment block directly above the binding, `//` markers stripped (`None` if there is none) |

Bindings come back in source order, one entry per name, carrying the last
binding's value, so a host regenerating the file keeps its ordering.

`text` exists because a number's spelling is not recoverable from its `f64`:
`0.020000` and `0.02` are the same value, and only the source text says which
one the author typed. `comment` lets a host show the user's own note next to a
value in its UI. A blank line, or any code, between a comment and a binding
ends the block, so a file header does not become the first binding's
documentation.

---

## Writing

Use goal-based editing. The full API is in
[goal-based-editing.md](goal-based-editing.md).

```rust
use petal::goal_based_editing::{modify_source_with_goals, Goal};

let updated = modify_source_with_goals(&source, &[
    Goal::should_set_value("color_scheme", "nord"),
    Goal::should_set_value("font_size", 16),
])?;
std::fs::write("config.ptl", updated)?;
```

`should_set_value` states an outcome (reading `name` out of the result yields
`value`) and the library decides how to get there:

- **The name is bound:** the right-hand side of its last top-level binding is
  replaced. The `let`, the name, comments, blank lines and every other
  statement are untouched.
- **The name is not bound:** `let name = value` is inserted, at the end of the
  file or wherever a [placement](goal-based-editing.md#placement) says.
- **The goal already holds:** nothing is written. The source comes back
  byte-identical, so a save that rewrites every field only touches the lines
  that moved, and `let drag = 0.020000` does not become `let drag = 0.02` on a
  save that changed nothing about it.

Because the *whole* right-hand side is replaced, a non-literal binding
collapses to a literal: `let font_size = if wide_screen then 16 else 12 end`
becomes `let font_size = 14`. See
[goal-based-editing.md](goal-based-editing.md#goalshould_set_valuename-value).

---

## `StaticValue`

The value type shared by both directions.

| Variant | Constructor | Petal source |
|---|---|---|
| `Str` | `StaticValue::str(s)` / `"s".into()` | `"dracula"` (quoted, escaped) |
| `Int` | `StaticValue::int(n)` / `5.into()` | `5` |
| `Float` | `StaticValue::float(f)` / `1.0.into()` | `1.0` (always has a `.`) |
| `Bool` | `StaticValue::bool(b)` / `true.into()` | `true` |
| `Nil` | `StaticValue::nil()` | `nil` |
| `List` | `StaticValue::list(items)` | `[1, 2, 3]` |
| `Record` | `StaticValue::record(fields)` | `{ tab_width: 4 }` |
| `Call` | `StaticValue::call(name, args)` | `rgb(255, 0, 0)` |

`to_source()` renders one as Petal source. Every variant renders to well-formed
Petal. Strings are quoted and escaped (including the interpolation opener `{`),
so a value that came from user input can never break out of its literal. There
is deliberately no verbatim/raw-source variant.

Record keys and call names are rendered **bare**, so they must be valid Petal
identifiers; they are not validated against the grammar.

### Round-tripping

Everything the reader can read, the writer can write back to source that reads
as the same value. That is what makes read-modify-write safe, and tests in both
modules pin it:

```rust
let value = get_static_value(&source, "accent")?;
let source = modify_source_with_goals(&source, &[
    Goal::should_set_value("accent", value.clone()),
])?;
assert_eq!(get_static_value(&source, "accent")?, value);   // unchanged
```

---

## Limits

- **Reading is static, so a computed value is unreadable.** `let x = a + b`
  needs the program to run. If you need computed config, run the file as a
  program (`Env::run_source`) instead.
- **`state` is not configuration.** Its value only exists while the program
  runs, and one declaration inside a function can be many live slots at once
  (one per [call path](language-guide.md#one-slot-per-call-path)). It reads as
  `NotStatic`.
- **Writing flattens.** `should_set_value` replaces the whole right-hand side,
  so a conditional binding loses its conditional, unless the goal already
  holds, in which case nothing is written.
- **Exact representability is the host's problem.** A consumer with a
  fixed-point grid or a bounded range rejects and rounds against its own
  arithmetic; Petal reads and writes the number it was given.
- **Modules are not followed.** `import`ed files are not read;
  `get_static_value` looks only at the source you hand it.

## See also

- [goal-based-editing.md](goal-based-editing.md) — the full editing API.
- [program-modification.md](program-modification.md) — every way a Petal
  program can be modified programmatically.
