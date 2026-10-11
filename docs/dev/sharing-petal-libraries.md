# Sharing Petal libraries

Notes from writing [bloom](../../core-runtime/bloom/), the first library in
`core-runtime/`: what a pure-Petal library can already do, what it has to work
around, and which of those are language- or host-level gaps worth closing.

Everything below was hit while building a real library — a ~2,300-line UI
component set used by Garden panels and by an app in `examples/` — rather than
imagined at a whiteboard.

The gaps the first draft of this document listed are closed, and bloom is
written against the result. This page describes how a library is written and
shared now, and the edges an author still meets.

## What already works, and works well

**Call-path-keyed `state` is the feature that makes a component library
possible.** A `state` slot inside a library function is keyed by the whole call
path, so this is a complete, reusable, animated component:

```petal
pub fn ease_flag(on, rate)
  state v = 0.0            // one animator per *callsite*, not per function
  …
end
```

Two callsites of `button` never share their hover fade; a `button` inside a
`for` loop gets one animator per iteration. No ids, no registration, no state
record threaded through the app. Every other UI toolkit spends its API budget
on this problem. (See [state-call-paths.md](state-call-paths.md).)

**A facade module re-exports cleanly.** bloom's is eight lines of
`pub import ./button`. A library presents one import surface
(`import bloom`) over many implementation modules, and — the property that
matters — a re-export carries a whole overload set, so `bloom.button` has
both of `bloom/button`'s arities.

**Closures make a z-order possible.** Immediate mode has no layering, so bloom
keeps a queue of paint closures (`defer_paint(fn() … end)`) that an app flushes
with one call at the end of its frame. Storing closures in a module-level
`var`, and the fact that a closure over a `var` must read it with `get` —
which the compiler insists on, naming the reason — made this a ten-line
mechanism rather than a redesign.

**Module `state` is per-module and survives hot reload,** which gives a library
somewhere to keep genuinely global facts — bloom's "which menu is open" and
"who has focus" cells — without asking the app to hold them.

**Hot reload across files works** once the host resolves and watches the
imported files (see the host section below).

## How a library is put together

### Every module imports what it uses

Nothing reaches a module without an import except the core builtins and
`std`. A library module that draws says `import ui` at its top; one that
calls `lerp` says `import creative`. An import reaches only the file it is
written in, so each of bloom's implementation modules carries its own.
[Which name wins](../module-system.md#which-name-wins) has the precedence: a
module's own declarations, then selective imports, then plain imports, then
`core` and `std`.

Because bloom's modules import `ui`, bloom's exports shadow `ui`'s for a
script that plainly imports both: an importer's export beats the export of a
module it is built on, whatever order the imports are written in. A script
with `import ui` and `import bloom` gets bloom's `button`, and `ui.button`
is still there by its qualifier.

### Modules are paths; identity is canonical

A module name is a path of identifier segments joined by `/`:

```petal ignore
import bloom/menu                // binds `menu`
import petal/menu as pmenu       // the two coexist; `as` names the second
import bloom/menu: open, close   // selective, and it may wrap across lines
import ./palette                 // a file beside the importer
```

The last segment is the qualifier, `as` overrides it, and the module's
canonical name — `<package>/<path>` — is its identity: its dedupe key, its
`state` key prefix, its qualified export names. `import ./motion` inside
bloom and `import bloom/motion` from an app are the same module, with one
set of module-level `var`s and `state`. See
[module-system.md](../module-system.md#module-identity).

### The facade is `pub import`

```petal ignore
pub import ./button               // every export, whole overload sets
pub import ./theme: accent        // a selection — a missing name is an error
```

A plain `pub import` also binds the names locally (the facade can use them),
sits below the file's own declarations (a local declaration wins silently,
and it merges with an overload set rather than replacing it), and leaves a
name two of them offer ambiguous for whoever uses it. Chains work, cycles are
the ordinary cycle error. The two names bloom's facade spells differently are
ordinary declarations layered over what the re-exports bound locally:

```petal ignore
pub import ./theme
pub let ink_on = on
```

A name added to an implementation module is published without an edit to the
facade. See [module-system.md](../module-system.md#re-exporting).

### Selective import lists wrap

A line break is allowed after the `:` of a selective import and after each
`,`, and a trailing comma is legal:

```petal ignore
import bloom/theme:
  theme, ts, tone,
  hair, stroke,
```

The list continues onto a later line only when that line starts with an
identifier followed by `,` or the end of the line. One caveat: `petal lint`
re-indents continuation lines to the statement's own indent, so it offers to
flatten a hanging indent.

### The manifest names it and says what it needs

```toml
[package]
name = "bloom"
version = "0.1.0"
modules = "src"

[dependencies]
text_layout = { path = "../text-layout" }

[requires]
ui = ">=0.1"
```

The *manifest* name is the package name, so a library keeps its identity
wherever its directory lands. A module named like its package
(`src/bloom.ptl`) is the facade slot: what a bare `import bloom` finds.
`[dependencies]` names the packages its modules import (the key is the import
name) and `[requires]` the host interfaces it needs, which a host must
provide for the program to load. Users reach a library through a
`[dependencies]` entry of their own — a path, or a git repository pinned to
a tag and fetched, locked and optionally vendored by the package manager.
bloom, text_layout and glsl are also bundled in the runtime. See
[packages.md](../packages.md).

### Overload sets merge across modules, with two edges

A binding that lands on a name a lower tier already put in scope joins its
set by arity. Merging happens only between function sets the compiler built;
a `let`, a record or a builtin native still shadows wholesale. The
higher-precedence side wins a colliding arity, silently. See
[function-overloading.md](../function-overloading.md).

**Edge one: arity is the only dispatch key, so bloom exports `ts_a`.**
`draw_text(s, pos, style, alpha)` would be arity 4, and `ui` already defines
arity 4 as `draw_text(text, pos, size, c)`. Adding bloom's would not extend
the set, it would take that slot over. Merging adds an arity; it does nothing
for wanting a second meaning of one that exists. `ts_a(t, size, color, a)`
carries the alpha on the style record instead.

**Edge two, and this one bites: inside one variant's body, the name cannot see
its siblings.** A variant's own name is bound to that variant (the
self-recursion binding), so a call at another arity falls *outward* — and
since a library imports `ui` plainly, outward is `ui`'s function of that name.
The two-line default-argument idiom silently changes meaning:

```petal ignore
pub fn context_menu(area, items)
  context_menu(area, items, {})   // ← `ui`'s arity 3, not this file's
end

pub fn context_menu(area, items, opts)
  …
end
```

That is only a problem when the library and the package under it share a
name, but a component library shares a lot of them: bloom collides with `ui`
on `button`, `checkbox`, `radio_group`, `slider`, `text_field`,
`context_menu` and `tooltip`. The library-side fix is to route both arities
through a private implementation `ui` cannot reach:

```petal ignore
pub fn context_menu(area, items)
  context_menu_impl(area, items, {})
end

pub fn context_menu(area, items, opts)
  context_menu_impl(area, items, opts)
end

fn context_menu_impl(area, items, opts)
  …
end
```

bloom does that for its seven. The real fix belongs in the compiler: a
variant's body should resolve its own name to its module's complete set.

### Small syntax edges

- `match` has no `else` arm; the catch-all is `when _ ->`.
- An `if` used as an expression still needs its `end`:
  `(if on then 1.0 else 0.0 end) * k`.

Neither is a gap so much as a thing every library author will hit once.

## What a host does

- **Load with a path.** `env.load_program_at(source, path)` gives relative
  imports a file to resolve against and finds the project's `petal.toml`.
  A program loaded from a string resolves `./x` against in-memory modules
  only, unless the host sets `env.set_inline_import_dir`.
- **Watch every file.** `env.module_manifest(program_id)` and
  `env.program_source_paths` list every file a program compiled from, so a
  host hot-reloads when an imported module changes.
- **Register a library in one call**, when it ships one that is not bundled:
  `env.add_package(root)` from disk, `env.register_package(name, sources)`
  from `include_str!`. `env.packages()` (or `petal packages`) lists what is
  there. A registration wins over the bundled copy of the same name.
- **Provide the interfaces libraries require.** `petal_ui::register_all`
  provides `ui`; a host with natives of its own ships an interface package
  and calls `env.provides_package(name)`. See
  [Host interface packages](../module-system.md#host-interface-packages).
- **The headless driver** `petal-ui-run` resolves packages the way the CLI
  does and takes `-I <dir>` for anything else
  (`Headless::from_file_with_paths` in tests).

## Testing notes

**The headless wall clock moves now.** `Headless::frame` advances `time()` by
1/60 s after every frame from a deterministic t0 = 0, so frame *N* sees
`time() == N/60` and `time()` tracks the `dt` the harness was already
publishing. It never reads the system clock, so traces stay byte-identical run
to run, and an animation written against `time()` (`ui`'s
`spinner`, `elapsed`) is testable. Assigning `ui.time` still wins for
the next frame, and the advance resumes from there.

**A `state` key contains the module that declares it,** by canonical name:
bloom's animators key as `bloom/controls::switch/…` and `bloom/motion::v`,
whichever way a script imported them, because a re-export routes through the
defining module. Tests and tools that key on state paths should still match
on the animator's own variable rather than the whole path: renaming a module,
or giving a directory of loose modules a `petal.toml`, changes the prefix and
nothing else.
