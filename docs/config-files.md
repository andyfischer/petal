# Petal as a Configuration Format

A `.ptl` file can be used as an application's **configuration file**. The file is
mostly `let` bindings; the host reads the values out of it, and writes changes
back when the user flips a setting in the UI.

```petal
// ~/.garden/config.ptl

let color_scheme = "dracula"
let font_size    = 14
let editor       = { line_numbers: true, tab_width: 4 }
let accent       = rgb(255, 128, 0)
let recent       = ["a.rs", "b.rs"]
```

Nothing about this is a separate dialect — it is ordinary Petal source. The point
of using Petal rather than TOML or JSON is that the *same file* can grow into
real code (a function, an `if`, an imported module) without the user having to
migrate to a different format.

Two APIs cover the round trip:

| Direction | API | Module |
|---|---|---|
| **Read** | `get_static_value(source, name)` / `static_values(source)` | [`rust/src/static_value.rs`](../rust/src/static_value.rs) |
| **Write** | `Goal::should_set_value(name, value)` | [`rust/src/goal_based_editing.rs`](../rust/src/goal_based_editing.rs) |

Both speak the same type, [`StaticValue`](#staticvalue), so a value read out of a
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

**Nothing runs.** There is no `Env`, no heap, no stack — the source is parsed and
the binding's right-hand side is evaluated statically. A config file can't print,
allocate, or loop forever just by being read, and reading is cheap enough to do
on every access if you like.

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

| Static ✅ | Not static ❌ |
|---|---|
| `"a"`, `14`, `1.5`, `true`, `nil` | `12 + 2` (arithmetic) |
| `-3`, `not flag` (folded) | `"size {n}"` (interpolation) |
| `[1, 2]`, `{ k: "v" }` | `if wide then 16 else 12 end` |
| `rgb(255, 0, 0)` — see below | `sizes[0]`, `config.size` |
| a reference to a name bound statically **above** it | `fn` and `state` declarations |

**A call is static.** `rgb(255, 0, 0)` reads back as
`StaticValue::Call { function: "rgb", args: [...] }`, held **unevaluated**. In a
config file a call names a constructor the *host* interprets — a color, a layout,
a pane — not a computation to run. Interpret it yourself, or write it back
unchanged.

### Errors

`get_static_value` distinguishes three failures, because a caller reacts to each
differently:

| Variant | Meaning | Typical response |
|---|---|---|
| `StaticValueError::Parse` | the file doesn't parse | surface it; the config is broken |
| `StaticValueError::NotFound` | no top-level binding of that name | fall back to the default |
| `StaticValueError::NotStatic` | bound, but needs running to know | fall back, and maybe warn |

`NotFound` and `NotStatic` are deliberately separate: `fn font_size() … end` or
`state font_size = 14` report as `NotStatic`, not `NotFound`, because the name
*is* plainly there in the file and telling the user it's missing would misdirect
them.

`static_values` instead **omits** non-static bindings rather than failing, so a
config file that also declares functions still yields its readable settings.

---

## Writing

Use the goal-based editing system — see
[goal-based-editing.md](goal-based-editing.md) for the full API.

```rust
use petal::goal_based_editing::{modify_source_with_goals, Goal};

let updated = modify_source_with_goals(&source, &[
    Goal::should_set_value("color_scheme", "nord"),
    Goal::should_set_value("font_size", 16),
])?;
std::fs::write("config.ptl", updated)?;
```

`should_set_value` states an outcome — *reading `name` out of the result yields
`value`* — and the module decides how to get there:

- **The name is bound** → the right-hand side of its **last** top-level binding is
  replaced. Everything else is untouched: the `let`, the name, comments, blank
  lines, indentation, and every other statement in the file.
- **The name isn't bound** → `let name = value` is appended as a new top-level
  statement.

Edits go through the lossless CST, so a file the user hand-wrote comes back
looking hand-written:

```text
before                             after
──────────────────────────────     ──────────────────────────────
// user config                     // user config
let font_size = 12 // points  →    let font_size = 16 // points
let other = 1                      let other = 1
```

### Non-trivial bindings collapse to a literal

The *whole* right-hand side is replaced, which is what makes the change static
even when the existing binding isn't:

```petal
let font_size = if wide_screen then 16 else 12 end
```

`should_set_value("font_size", 14)` rewrites this to `let font_size = 14`. The
conditional is gone, and the goal holds by construction.

That is the blunt-but-correct answer. Richer resolutions — editing the branch
that is actually taken, or the binding a conditional selects, rather than
flattening the expression — are the natural next step for this goal, and the
place to add them is `ensure_binding` in
[`goal_based_editing.rs`](../rust/src/goal_based_editing.rs).

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
Petal — strings are quoted and escaped (including the interpolation opener `{`),
so a value that came from user input can never break out of its literal. There is
deliberately **no verbatim/raw-source variant**.

Record keys and call names are rendered **bare**, so they must be valid Petal
identifiers; they are not validated against the grammar.

### Round-tripping

Everything the reader can read, the writer can write back to source that reads as
the same value. That property is what makes read-modify-write safe, and it is
pinned by tests in both modules:

```rust
let value = get_static_value(&source, "accent")?;
let source = modify_source_with_goals(&source, &[
    Goal::should_set_value("accent", value.clone()),
])?;
assert_eq!(get_static_value(&source, "accent")?, value);   // unchanged
```

---

## Limits

- **Reading is static, so a computed value is unreadable.** `let x = a + b` needs
  the program to run. If you need computed config, run the file as a program
  (`Env::run_source`) instead — this API is the no-execution path.
- **`state` is not configuration.** Its value only exists while the program runs
  and changes as it runs; it reads as `NotStatic`.
- **Writing flattens.** `should_set_value` replaces the whole right-hand side, so
  a conditional binding loses its conditional (see above).
- **Modules aren't followed.** `import`ed files are not read; `get_static_value`
  looks only at the source you hand it.

## See also

- [goal-based-editing.md](goal-based-editing.md) — the full editing API.
- [program-modification.md](program-modification.md) — every way a Petal program
  can be modified programmatically.
