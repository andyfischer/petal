# Settings: `let X = tunable(…)`

A **setting** is a top-level binding that tools and hosts are meant to edit:
a speed, a size, a colour, a table of options. It is a `let` whose whole
initializer is a call of the builtin
[`tunable`](../Builtins.md#direct-manipulation-hints):

```petal
let SPEED = tunable(4.0)
let RADIUS = tunable(12)

print(SPEED * 2, RADIUS)   // 8.0 12
```

`tunable` returns its first argument, so the program runs exactly as if the
call were not there. What changes is how everything around the program
treats the binding. An editor that lets you drag a shape rewrites the setting
behind it and leaves the rest of the code alone, a host can list a file's
settings and draw a slider for each, and a number dragged from positive to
negative is still a cheap value change.

`tunable` is an [annotation](../language-guide.md#annotations), like
`const(…)` and `rigid(…)`: a reserved builtin that returns its argument and
changes only what tools do with the value. Where it is written decides what
it means:

- **The whole initializer of a top-level `let`** (`let X = tunable(v, …)`):
  a *setting*, a binding that hosts list, set live and override from
  outside, holding data of any type (a number, a string, a bool, a colour, a
  record, a list).
- **Anywhere else** (a literal in a function body, a call argument, part of
  an expression): a *hint* that direct manipulation should prefer the number
  it marks, with an optional range.

This page is the reference for settings as a script author sees them. The
mechanics on the tool side are owned by other pages, and each section links
to its owner.

- [Syntax](#syntax)
- [Where it is a setting](#where-it-is-a-setting)
- [What a setting can hold](#what-a-setting-can-hold)
- [Ranges and options](#ranges-and-options)
- [What it changes for the program](#what-it-changes-for-the-program)
- [What it changes for tools](#what-it-changes-for-tools)
- [Editing one while the program runs](#editing-one-while-the-program-runs)
- [Worked example: a slider](#worked-example-a-slider)
- [`let`, `tunable`, `const(…)` and `state`](#let-tunable-const-and-state)
- [The deprecated `config let`](#the-deprecated-config-let)

## Syntax

```petal
let offset = tunable(10)
let scale: float = tunable(1.5)             // an optional type annotation, as on `let`
pub let THEME = tunable({                   // exported from a module
  bg: #101418,
  accent: #ff2e88,
})
let SPEED = tunable(4.0, 0, 10)             // a number with a range
let START = tunable("title", options: ["title", "select", "play"])
print(offset, scale, THEME.bg, SPEED, START)
```

Everything outside the call is an ordinary
[`let`](../language-guide.md#variables). The call's arguments are the value,
then `min`, `max` and `step` by position or by name, and `options:` by name.

### The `@tunable` spelling

The same setting can be written with the annotation before the declaration
([Writing an annotation with `@`](../language-guide.md#writing-an-annotation-with-)).
The declared value is the call's first argument, so `@tunable(a, …)` passes
everything after the value:

```petal
@tunable let offset = 10                          // let offset = tunable(10)
@tunable(0, 10) let SPEED = 4.0                   // let SPEED = tunable(4.0, 0, 10)
@tunable(min: 3.0, max: 12.0, step: 0.5)
pub let RUN = 7.0                                 // pub let RUN = tunable(7.0, min: 3.0, max: 12.0, step: 0.5)
@tunable(options: ["title", "select", "play"])
let START = "title"
print(offset, SPEED, RUN, START)
```

The two spellings are the same program to the compiler and to every tool and
host: the parser turns the `@` form into the call. Neither is preferred.
Named bounds read well in the `@` form, where the bounds come before the
value: `@tunable(3.0, 12.0)` is "min 3, max 12", not "value 3".
The annotation goes on the declaration's line or on lines of its own above
it, and before `pub`.

## Where it is a setting

| Written | Result |
|---|---|
| `let X = tunable(v)` at the top level | a setting |
| `@tunable let X = v` | the same setting |
| `pub let X = tunable(v)` | an exported setting |
| `let X = tunable(4) * 2` | not a setting: the hint marks the `4` for direct manipulation, and hosts do not list `X` (lint `tunable-not-whole-initializer`) |
| `var X = tunable(v)` | not a setting: a `var` is a cell, not a knob (lint `tunable-on-var`) |
| `state x = tunable(v)` | not a setting, and the hint cannot work: a running program keeps its state across a reload (lint `dm-hint-state-init`). Declare the start value as a setting and initialise the state from it |
| `let X = const(tunable(v))` | error: `a setting of literals is already static; drop const(…)` |
| `let x = tunable(v)` inside a function | a hint on the literal, not a setting |

`tunable` is a reserved name: declaring a function, binding, parameter or
loop variable called `tunable` (or `rigid`, or `const`) is a compile error,
so every reader can recognise the call by its name.

## What a setting can hold

Static data: numbers, strings, booleans, `nil`, colour literals, lists and
records of those, enum variants, and calls with literal arguments such as
`rgb(255, 0, 80)`, which a host reads as a constructor to interpret. That is
the set a host can read without running the program
([What counts as static](../usage/config-files.md#what-counts-as-static)) and
the set whose literals can be edited in place.

A computed value still runs, but hosts see no value for it and a slider has
no literal to hold on to, so `petal lint` reports it (`config-not-data`):

```petal
let base = 20
let margin = tunable(base * 2)     // config-not-data: needs evaluating
print(margin)   // 40
```

Every number literal in a setting's data is a knob, not only a scalar
initializer: the `140` in `let BOX = tunable({x: 140, y: 90})` and the `30`
in `let XS = tunable([20, 30])` are preferred by direct manipulation and
match its `knob` selector exactly as `let X = tunable(140)` does.

## Ranges and options

```petal
let SPEED = tunable(4.0, 0, 10)                       // min 0, max 10
let GRAV = tunable(-9.8, min: -20, max: 0, step: 0.1)
let MODE = tunable("day", options: ["day", "night"])
@tunable(min: 0, max: 1) let MIX = 0.25               // bounds by name, `@` form
print(SPEED, GRAV, MODE, MIX)   // 4.0 -9.8 day 0.25
```

The bounds are `min`, `max` and `step`, in that order by position, or by
name in any order (`tunable(4.0, max: 10, min: 0)`); a positional bound may be
followed by named ones (`tunable(4.0, 0, max: 10)`).

What hosts see is the setting, not the call:

- `static_bindings` reports `SPEED` with the value `4.0`, the text `4.0` and
  a `range` of `{min: 0, max: 10}`. From C, `pb_source_bindings_json` gives a
  `"tunable"` object with the range and the options; `"config": true` and
  `"range"` are deprecated mirrors. `petal show-static --json` shows the same
  `range`.
- The path `SPEED` names the `4.0`, so `set_config_value("SPEED", …)` and
  `pb_vm_set_tunable` set it live without a reload, across zero too, and the
  path edits (`literal_edit`, `pb_source_set_value`, `pb_source_set_setting`)
  change the number and leave the bounds as written.
- Editing the number in the file (`tunable(4.0, …)` to `tunable(7.0, …)`) is a
  value patch on hot reload. So is editing a bound.
- Direct manipulation keeps a drag within the range.

The range is **advice, not a clamp**. Nothing in Petal enforces it:
`set_config_value` and `pb_vm_set_tunable` accept a value outside it, as
direct manipulation treats it as a box for its search and nothing more. A
host that wants a hard limit clamps before it sets. `options:` is advice too:
`petal lint` reports `tunable-value-not-in-options` when the value is not one
of them, and a host can offer them as a dropdown.

The bounds are numbers written as literals (`dm-hint-bounds` reports one that
is not, a `min` above the `max`, and bounds on a value that is not a number).
The rules are in [Builtins, `tunable`](../Builtins.md#direct-manipulation-hints).

Keep a setting to one declaration and do not assign it again. A top-level
`margin = 50` after the declaration is legal, but the value a host reads is
then the assignment's, and the binding is no longer a
[static value](../features/static-values.md).

## What it changes for the program

Nothing you can observe from inside the script. A setting evaluates, scopes,
shadows, exports and hoists as a `let` does, and it gets no compile-time
check: for a guarantee that a value depends on nothing dynamic, that is
[`const(…)`](../language-guide.md#const--a-value-that-must-be-constant).
`tunable` is still a native call at run time (one per setting per run), where
`const(…)` is erased by the compiler.

## What it changes for tools

### Direct manipulation edits the setting

When a user drags something a script drew, the editor asks Petal which source
edits would produce the new value. A value computed from several bindings has
several answers:

```petal ignore
import io
let offset = 10
let x = 20
push_output(symbol("shapes"), x + offset)
```

```
$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 sketch.ptl
2 proposals:
  1. set `x` to 32.5 (line 3)
  2. set `offset` to 22.5 (line 2)
```

Write `let offset = tunable(10)` and the knob is ranked first. Settings and
`tunable` literals are one knob tier; with `--knob-strict` (v1's rule) only
knobs move once the program has any. A literal written directly in the call
has no variable and stays editable. The mechanics are in
[direct-manipulation.md](../features/direct-manipulation.md).

### Hosts can list a file's settings

A host reads a file's top-level bindings without running it, with
`static_value::static_bindings`. Each binding carries `is_config` ("is a
setting"), next to its value, its text as written, the comment block above
it, its line, and a `range` and `options` when it declares them. That is
enough to build a settings panel: one control per setting, labelled with the
author's comment. From C the same list is `pb_source_bindings_json`. See
[Reading the whole file](../usage/config-files.md#reading-the-whole-file-including-what-it-cannot-read)
and [embedding-c.md](../usage/embedding-c.md#editing-source-a-script-as-a-config-file).

### Where else the mark shows up

| Place | How |
|---|---|
| `petal show-static` | the declaration's kind reads `config let` (`"kind": "config let"` with `--json`, plus `"range"`), a name kept until the keyword is removed. It is classified by the same rules as a `let`. See [show-static](../CLI.md#show-static--static-declarations). |
| `petal show-ir --json` | `"is_config": true` on the term that carries the binding's value, and on every literal in its data. This is what direct manipulation reads. |
| Hot reload's source diff | a declaration that changed structurally has the kind `ConstructKind::TunableLet` (`"tunable let"` in the reload report). |
| `petal propose-edit` | the `knob` selector (alias `config`) and `--knob-strict`. |

## Editing one while the program runs

A [hot reload](../features/hot-reload.md) treats a setting as it treats any
code. Editing a literal so that it keeps its type (`4.0` to `5.5`, a field of
a record, an element of a list, an argument of a constructor call) is a value
change: the new constant is written into the running program, nothing is
recompiled, and `state` is untouched. Changing a value's type or shape (`12`
to `12.5`, a list that grows) recompiles. Both are true of a plain `let` too,
and so is crossing zero: `-0.5` is one literal, so `-0.5` to `0.5` is a value
change (see [Negative numbers](../features/hot-reload.md#negative-numbers)).

Adding or removing the `tunable(…)` wrapper recompiles: it changes the
construct, not a value.

Values built from a setting (a table computed from `SPEED`, say) are rebuilt
after the edit, and a `state` variable whose initializer read the old value
keeps what it holds; the reload report names both. See
[Static values across a reload](../features/hot-reload.md#static-values-across-a-reload).

A host can also set a value without the file changing at all, which is what a
slider needs while it is being dragged: `Env::set_config_value` in Rust,
`pb_vm_set_tunable` in C. The value is addressed by a binding path such as
`SPEED` or `POST.effects[2].amount`, the result is exactly what reloading the
edited file would give, and the host writes the file once when the drag ends.
A value that would need a recompile is refused (`NeedsReload`) and nothing
changes. The owner pages are
[Setting a value without writing the file](../features/hot-reload.md#setting-a-value-without-writing-the-file)
for the rules and
[embedding-c.md](../usage/embedding-c.md#setting-a-value-without-writing-the-file)
for the C calls and the JSON form of a value.

## Worked example: a slider

A script with three settings and one piece of state:

```petal
// How far the dot moves each frame.
let SPEED = tunable(4.0, -10, 10)
// Dot radius in pixels.
let RADIUS = tunable(12)
let TINT = tunable({r: 255, g: 80, b: 40})

state x = 0.0
x += SPEED
print(x, RADIUS * 2, TINT.g)   // 4.0 24 80
```

A live-editing host does this with it:

1. **Builds the panel.** It calls `static_bindings` on the file and keeps the
   settings: `SPEED` (a float from -10 to 10, with the comment "How far the
   dot moves each frame."), `RADIUS` (an int) and `TINT` (a record). It draws
   a slider for each number. `x` is `state` and is not offered.
2. **Drags.** As the user moves the `SPEED` slider to 5.5, each tick calls
   `set_config_value(stack, None, "SPEED", &StaticValue::float(5.5))`. The
   constant in the running program becomes `5.5`, nothing is recompiled, and
   the next frame adds 5.5 to the `x` the program already had.
3. **Crosses zero.** The user drags on to `-2.0`. This is still a value
   change, and the dot starts moving left.
4. **Hits a type change.** Dragging `RADIUS` to `12.5` is refused with
   `NeedsReload`: the file says `12`, an integer. The host either rounds to an
   integer, or writes the file and reloads. Writing `tunable(12.0)` in the
   first place avoids the question.
5. **Releases.** The host writes `Env::program_source(pid, None)` to disk: the
   original file with `tunable(-2.0, -10, 10)` and every comment and blank
   line where it was. The reload that the file watcher then triggers finds
   nothing to do.

## `let`, `tunable`, `const(…)` and `state`

| | `let X = v` | `let X = tunable(v)` | `let X = const(v)` | `state x = v` |
|---|---|---|---|---|
| Says | a binding | a binding meant to be tuned from outside | a binding that must be constant | a value that persists between runs |
| Checked by the compiler | no | no | yes: an error if the value depends on anything dynamic | no |
| Where | anywhere | top level (elsewhere it is a hint) | top level only | anywhere |
| Can be assigned again | yes | yes, though it should not be | no | yes |
| Preferred by direct manipulation | no | yes | no | no |
| A setting in `static_bindings` | no | yes, with its range and options | no | no (and it has no static value) |
| A literal edited by hot reload | patched | patched, including a sign change | patched | patched in the initializer; the slot keeps its value |

`tunable` and `const(…)` answer different questions and do not combine:
`const(…)` is about what a value depends on, `tunable` about who edits it. A
setting of literals is already a [static value](../features/static-values.md),
so it is built once and kept between runs just as a `const(…)` is, and
`const(tunable(…))` is an error.

A setting and `state` are opposites in one respect: a setting lives in the
source text and changes when the text does, while a `state` value lives in
the running program and survives changes to the text. A value the user tunes
is a setting; a value the program accumulates is
[`state`](../features/inline-state.md).

## The deprecated `config let`

Before `tunable` could declare a setting, the `config` modifier did:

```petal
config let SPEED = 4.0                       // means let SPEED = tunable(4.0)
config let GRAV = tunable(-9.8, -20, 0)      // means let GRAV = tunable(-9.8, -20, 0)
print(SPEED, GRAV)
```

It still works and means exactly the same, and it is deprecated
([plan](../tasks/lifecycle-annotation-functions-refactor.md)): the compile
check warns about each top-level one (`[config-let-deprecated]`), and
`petal migrate-old-code` rewrites it to `let X = tunable(v)` (or drops `config`
when the value is already a `tunable(…)` call). A
`// petal-lint-ignore config-let-deprecated` silences both. `config` goes directly before `let` and after `pub`; it
modifies `let` only, and `config var`, `config const`, `config state` and
`config fn` are errors. It is a contextual word: everywhere else `config` is
an ordinary identifier (`let config = {width: 80}`).

## See also

- [direct-manipulation.md](../features/direct-manipulation.md): tracing a
  drawn value back to source and proposing edits.
- [hot-reload.md](../features/hot-reload.md): what each kind of edit costs and
  what it keeps.
- [config-files.md](../usage/config-files.md): reading and writing a `.ptl`
  file as configuration.
- [goal-based-editing.md](../features/goal-based-editing.md): writing a new
  value into a file while keeping its comments and layout.
