# `config let`: declaring a tunable

`config let` declares a binding that tools and hosts are meant to edit: a
speed, a size, a colour, a table of settings.

```petal
config let SPEED = 4.0
config let RADIUS = 12

print(SPEED * 2, RADIUS)   // 8.0 12
```

The program runs exactly as if the word `config` were not there. What changes
is how everything around the program treats the binding. An editor that lets
you drag a shape rewrites the `config let` behind it and leaves the rest of
the code alone, a host can list a file's config bindings and draw a slider for
each, and a number dragged from positive to negative is still a cheap value
change.

This page is the reference for the modifier as a script author sees it. The
mechanics on the tool side are owned by other pages, and each section links to
its owner.

- [Syntax](#syntax)
- [Where it is allowed](#where-it-is-allowed)
- [`config` is not a reserved word](#config-is-not-a-reserved-word)
- [What a config binding can hold](#what-a-config-binding-can-hold)
- [What it changes for the program](#what-it-changes-for-the-program)
- [What it changes for tools](#what-it-changes-for-tools)
- [Editing one while the program runs](#editing-one-while-the-program-runs)
- [Worked example: a slider](#worked-example-a-slider)
- [`config let`, `let`, `const` and `state`](#config-let-let-const-and-state)

## Syntax

```petal
config let offset = 10
config let scale: float = 1.5      // an optional type annotation, as on `let`
pub config let THEME = {            // exported from a module; `pub` comes first
  bg: #101418,
  accent: #ff2e88,
}
```

`config` goes directly before `let`, and `pub` goes before `config`.
Everything after the keyword is an ordinary
[`let`](../language-guide.md#variables).

## Where it is allowed

`config` modifies `let` and nothing else.

| Written | Result |
|---|---|
| `config let x = 1` | a config binding |
| `pub config let x = 1` | an exported config binding |
| `config var x = 1` | error: `` `config` marks a `let` binding; a mutable `var` cell cannot be config `` |
| `config const X = 1` | error: `` `config` marks a `let` binding; write `config let` (a config binding of literals is already constant) or a plain `const` `` |
| `config state x = 1`, `config fn f() … end`, `config pub let x = 1` | not a declaration. `config` is read as a name here (see below), so the line fails as an expression, usually with ``undefined variable `config` `` |

`pub config var` and `pub config const` give the same two errors.

A `config let` inside a function body is accepted and marks the binding the
same way. The tools that work on a traced run see it (direct manipulation
prefers it, and [`fn_ast`](../features/function-introspection.md) reports it),
but the tools that read a file's top-level bindings do not: it is not in
`static_bindings`, not in `petal show-static`, and it cannot be set live by
path. Declare tunables at the top level of a file unless you only need the
first.

## `config` is not a reserved word

`config` is a contextual modifier. It is special only when the very next token
is `let` (or `var` or `const`, to give the errors above). Everywhere else it
is an ordinary identifier:

```petal
let config = {width: 80}
print(config.width)   // 80
```

`as`, `class` and `native` work the same way; see
[Identifiers and keywords](overview.md#identifiers-and-keywords).

## What a config binding can hold

Anything a `let` can: the initializer is an ordinary expression, and nothing
checks it.

```petal
let base = 20
config let margin = base * 2                 // allowed
config let shout = fn(s) -> upper(s)         // allowed
print(margin, shout("ok"))   // 40 OK
```

The tools are only useful, though, when the value is written as data:
numbers, strings, booleans, `nil`, colour literals, lists and records of
those, enum variants, and calls with literal arguments such as
`rgb(255, 0, 80)`, which a host reads as a constructor to interpret. That is the set a host can read
without running the program
([What counts as static](../usage/config-files.md#what-counts-as-static)) and
the set whose literals can be edited in place. `config let margin = base * 2`
has no literal of its own for a slider to hold on to.

Keep a config binding to one declaration and do not assign it again. A
top-level `margin = 50` after the declaration is legal, but the value a host
reads is then the assignment's, without the config mark, and the binding is no
longer a [static value](../features/static-values.md).

## What it changes for the program

Nothing you can observe from inside the script. A `config let` evaluates,
scopes, shadows, exports and hoists as a `let` does. It can be rebound with
`=`, and it gets no compile-time check: for a guarantee that the value depends
on nothing dynamic, that is [`const`](../language-guide.md#const--a-value-that-must-be-constant).

One thing differs in the compiled program, and it has no effect on values. A
negated number literal in a config binding's data is compiled as one constant
instead of a negation applied to a constant:

```
$ petal show-ir -e 'config let A = -4
let B = -4'
  t138 r138 = Constant(-4) [] ; A @1:16
  t139 r139 = Constant(4) [] @2:10
  t140 r140 = Neg [t139] ; B @2:9
```

This reaches through list elements, record fields and call arguments
(`config let P = {x: -1.5, dir: [0, -1]}`) and stops at any other
operator: in `config let W = 2 * -1` the `-1` stays a negation. It exists so
that changing a sign is a value edit; see
[Editing one while the program runs](#editing-one-while-the-program-runs).

## What it changes for tools

### Direct manipulation edits the config binding

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

Write `config let offset = 10` and the same command returns one proposal,
`` set `offset` to 22.5 ``. The rule: when a program declares any config
binding, config bindings are the ones to edit and every other named binding is
left alone. A literal written directly in the call has no variable and stays
editable. A program with no config bindings is unaffected, and a host's
explicit policy for a name overrides the source.

The mechanics (`VarPolicy`, `propose_edits`, the `config` flag on each
proposal) are in
[direct-manipulation.md](../features/direct-manipulation.md#config-let-in-source-policy).

### Hosts can list a file's config bindings

A host reads a file's top-level bindings without running it, with
`static_value::static_bindings`. Each binding carries `is_config`, next to its
value, its text as written, the comment block above it and its line. That is
enough to build a settings panel: one control per config binding, labelled
with the author's comment. From C the same list is `pb_source_bindings_json`,
where the field is `"config": true`. See
[Reading the whole file](../usage/config-files.md#reading-the-whole-file-including-what-it-cannot-read)
and [embedding-c.md](../usage/embedding-c.md#editing-source-a-script-as-a-config-file).

### Where else the mark shows up

| Place | How |
|---|---|
| `petal show-static` | the declaration's kind reads `config let` (`"kind": "config let"` with `--json`). It is classified by the same rules as a `let`: static when it depends on nothing dynamic and is not assigned again. See [show-static](../CLI.md#show-static--static-declarations). |
| `petal show-ast` | `Let config SPEED`, or `Let pub config SPEED`; with `--json`, `"is_config": true` on the `Let` (the field is omitted when false). |
| [`fn_ast`](../features/function-introspection.md) | a `Let` node's `is_config` field. |
| `petal show-ir --json` | `"is_config": true` on the term that carries the binding's value. This is what direct manipulation reads. |
| Hot reload's source diff | a declaration that changed structurally has the kind `ConstructKind::ConfigLet`. |
| [`petal apply-change convert-to-var`](../CLI.md#convert-to-var) | refuses a `config let`, since a `var` cannot be config. |

## Editing one while the program runs

A [hot reload](../features/hot-reload.md) treats a config binding as it treats
any code. Editing a literal so that it keeps its type (`4.0` to `5.5`, a field
of a record, an element of a list, an argument of a constructor call) is a
value change: the new constant is written into the running program, nothing
is recompiled, and `state` is untouched. Changing a value's type or shape
(`12` to `12.5`, a list that grows) recompiles. Both are true of a plain `let`
too. Two things are particular to `config`:

- **A sign change is a value change.** `-0.5` to `0.5` in a config binding's
  data is patched in place, because of the folding described
  [above](#what-it-changes-for-the-program). In a plain `let` the same edit
  adds or removes a negation and recompiles. See
  [Negative numbers in config data](../features/hot-reload.md#negative-numbers-in-config-data).
- **Adding or removing the word `config` recompiles.** The mark is part of the
  declaration, so `let` to `config let` is a change of construct, not of
  value.

Values built from a config binding (a table computed from `SPEED`, say) are
rebuilt after the edit, and a `state` variable whose initializer read the old
value keeps what it holds; the reload report names both. See
[Static values across a reload](../features/hot-reload.md#static-values-across-a-reload).

A host can also set a value without the file changing at all, which is what a
slider needs while it is being dragged: `Env::set_config_value` in Rust,
`pb_vm_set_config` in C. The value is addressed by a binding path such as
`SPEED` or `POST.effects[2].amount`, the result is exactly what reloading the
edited file would give, and the host writes the file once when the drag ends.
The same type and shape rule applies: a value that would need a recompile is
refused (`NeedsReload`) and nothing changes. The call works on any top-level
`let` of data, config or not. What `config` adds is the sign rule: only a
config binding can be dragged across zero this way. The owner pages are
[Setting a value without writing the file](../features/hot-reload.md#setting-a-value-without-writing-the-file)
for the rules and
[embedding-c.md](../usage/embedding-c.md#setting-a-value-without-writing-the-file)
for the C calls and the JSON form of a value.

## Worked example: a slider

A script with three tunables and one piece of state:

```petal
// How far the dot moves each frame.
config let SPEED = 4.0
// Dot radius in pixels.
config let RADIUS = 12
config let TINT = {r: 255, g: 80, b: 40}

state x = 0.0
x += SPEED
print(x, RADIUS * 2, TINT.g)   // 4.0 24 80
```

A live-editing host does this with it:

1. **Builds the panel.** It calls `static_bindings` on the file and keeps the
   entries with `is_config`: `SPEED` (a float, with the comment "How far the
   dot moves each frame."), `RADIUS` (an int) and `TINT` (a record). It draws
   a slider for each number. `x` is `state` and is not offered.
2. **Drags.** As the user moves the `SPEED` slider to 5.5, each tick calls
   `set_config_value(stack, None, "SPEED", &StaticValue::float(5.5))`. The
   constant in the running program becomes `5.5`, nothing is recompiled, and
   the next frame adds 5.5 to the `x` the program already had.
3. **Crosses zero.** The user drags on to `-2.0`. Because `SPEED` is a
   `config let`, this is still a value change, and the dot starts moving left.
   Had `SPEED` been a plain `let`, the call would have returned `NeedsReload`.
4. **Hits a type change.** Dragging `RADIUS` to `12.5` is refused with
   `NeedsReload`: the file says `12`, an integer. The host either rounds to an
   integer, or writes the file and reloads. Writing `config let RADIUS = 12.0`
   in the first place avoids the question.
5. **Releases.** The host writes `Env::program_source(pid, None)` to disk: the
   original file with `SPEED = -2.0` and every comment and blank line where it
   was. The reload that the file watcher then triggers finds nothing to do.

The same slider can be driven the other way round, from a drag on the dot
itself: the editor asks for the edit that moves the dot to where the pointer
is, and because the script declares config bindings, the answer is an edit to
one of them ([direct manipulation](../features/direct-manipulation.md#the-goal-based-protocol)).

## `config let`, `let`, `const` and `state`

| | `let` | `config let` | `const` | `state` |
|---|---|---|---|---|
| Says | a binding | a binding meant to be tuned from outside | a binding that must be constant | a value that persists between runs |
| Checked by the compiler | no | no | yes: an error if the value depends on anything dynamic | no |
| Where | anywhere | anywhere; useful at top level | top level only | anywhere |
| Can be assigned again | yes | yes, though it should not be | no | yes |
| Preferred by direct manipulation | no; left alone once the file has any config binding | yes | no | no |
| `is_config` in `static_bindings` | false | true | false | false (and it has no static value) |
| A literal edited by hot reload | patched | patched, including a sign change | patched | patched in the initializer; the slot keeps its value |

`config` and `const` answer different questions and do not combine. `const` is
about what a value depends on; `config` is about who edits it. A `config let`
of literals is already a [static value](../features/static-values.md), so it is built once and kept between
runs just as a `const` is.

`config` and `state` are opposites in one respect: a config value lives in the
source text and changes when the text does, while a `state` value lives in the
running program and survives changes to the text. A setting the user tunes is
a `config let`; a value the program accumulates is
[`state`](../features/inline-state.md).

## See also

- [direct-manipulation.md](../features/direct-manipulation.md): tracing a
  drawn value back to source and proposing edits.
- [hot-reload.md](../features/hot-reload.md): what each kind of edit costs and
  what it keeps.
- [config-files.md](../usage/config-files.md): reading and writing a `.ptl`
  file as configuration.
- [goal-based-editing.md](../features/goal-based-editing.md): writing a new
  value into a file while keeping its comments and layout.
