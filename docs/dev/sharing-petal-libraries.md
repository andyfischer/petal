# Sharing Petal libraries

Notes from writing [bloom](../../petal-libs/bloom/), the first library in
`petal-libs/`: what a pure-Petal library can already do, what it has to work
around, and which of those are language- or host-level gaps worth closing.

Everything below was hit while building a real library — a ~2,300-line UI
component set used by Garden panels and by an app in `examples/` — rather than
imagined at a whiteboard.

All seven gaps in the first draft of this document have since been closed, and
bloom has been migrated onto the result. That migration is the honest test of
the fixes, so each section below says both what the new form is and what it
actually did to the library. Two rough edges turned up on the way; they are in
gap 6, because that is where they live.

## What already works, and works well

**Call-path-keyed `state` is the feature that makes a component library
possible.** A `state` slot inside a library function is keyed by the whole call
path, so this is a complete, reusable, animated component:

```petal
export fn ease_flag(on, rate)
  state v = 0.0            // one animator per *callsite*, not per function
  …
end
```

Two callsites of `button` never share their hover fade; a `button` inside a
`for` loop gets one animator per iteration. No ids, no registration, no state
record threaded through the app. Every other UI toolkit spends its API budget
on this problem. (See [state-call-paths.md](state-call-paths.md).)

**A facade module re-exports cleanly.** It used to be 130 hand-written lines of
`export let button = bloom_button.button`; it is now eight lines of
`export import bloom/button: *` (gap 3). Either way a library can present one
import surface (`import bloom`) over many implementation modules, and — the
property that matters — a re-export carries a whole overload set, so
`bloom.button` has both of `bloom/button`'s arities.

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

## Language-level gaps

### 1. A host's implicit imports do not reach imported modules — fixed

The biggest one. A host registers its prelude as an implicit import
(`env.set_implicit_imports(&["ui"])`), and every name in it *was* available
bare — in the entry file only. In an imported module the same call resolved to
the raw native, or to nothing:

```petal ignore
// lib.ptl
export fn box(r, c)
  draw_rect(r, c)     // Error: Expected int at arg 1, got record
end                   // — the *native*, not the prelude's record overload
```

Implicit imports now bind inside **every** module of a program. A library
module writes `draw_rect(r, c)`, `screen_width()`, `ui_theme()` bare, and the
host's "scripts get this for free" contract no longer stops at file one.
Precedence inside a module, weakest first: the gated core prelude (`std`), the
host's implicit imports, the module's own explicit imports, the module's own
declarations. Implicit binding is weak and silent — it never raises a
collision.

The one deliberate exception: a prelude module itself, and anything it imports,
does not receive the host implicit imports (it is emitted before everything
else, so that would be a self-import). A prelude that spans two modules still
imports its sibling explicitly.

Reaching every module also armed a trap in hoisting, since fixed: a top-level
`fn` used to be left where it stands whenever its name was already in scope, so
that `let _draw_line = draw_line` above `fn draw_line` still read the native.
With a ~261-name prelude bound in every module, that rule fired on any library
function whose name merely *collided* with a prelude export — a call written
above the declaration silently reached the prelude's function, at its arity.
The rule now needs the read, not just the collision (see the language guide,
"Declaration order").

What it did to bloom: every implementation module lost its `import ui: …`
header — twelve statements across seven files, a list that had to name every
primitive the file used and be kept in sync by hand. The library no longer
hard-codes the name the host registered its prelude under.

### 2. Module names are flat and global — fixed

Module names *were* one identifier in one global namespace, so a library had to
prefix its files by hand — `bloom_menu.ptl`, `bloom_motion.ptl` — and two
libraries that both shipped a `menu.ptl` could not be used together at all.

A module name is now a path of identifier segments joined by `/`:

```petal ignore
import bloom/menu                // binds `menu`
import petal/menu as pmenu       // the two coexist; `as` names the second
import bloom/menu: open, close   // selective, and it may wrap across lines
```

The last segment is the local name (`menu.open`), `as` overrides it, and the
whole path is the module's identity — its dedupe key, its `state` key prefix,
its qualified export names. A library is a directory now, not a naming
convention; a flat `import palette` is unchanged. See
[module-system.md](../module-system.md#namespaced-paths).

Above that sits a manifest (gap 5), which lets a library declare its own name
rather than inheriting whatever directory a user dropped it in. bloom took the
manifest route: its files are plain `motion.ptl`, `theme.ptl`, `menu.ptl`, and
the `bloom/` in `import bloom/menu` comes from `petal.toml`, not from a
directory the user happens to have arranged.

**Pick one spelling per library.** A module reached as `motion` from inside the
package and as `bloom/motion` from outside is *two* modules with two copies of
its module-level `var`s and `state`. bloom uses the full path everywhere,
internal imports included.

### 3. There is no re-export form — fixed

The facade *was* 130 lines of `export let x = mod.x`: correct, and it did carry
overload sets, but it was a list to keep in sync by hand that said nothing when
an export was forgotten. `export import` makes it declarative:

```petal ignore
export import bloom/button: *        // every export, whole overload sets
export import bloom/theme: accent    // a selection — a missing name is an error
export import bloom/menu             // the module binding itself
```

A star also binds the names locally (the facade can use them), is the weakest
explicit binding in the file (a local declaration wins silently, and it merges
with an overload set rather than replacing it), and errors when two stars offer
the same non-function name. Chains work, cycles are the ordinary cycle error.
See [module-system.md](../module-system.md#re-exporting).

What it did to bloom: `src/bloom.ptl` went from 147 lines to 50, of which eight
are the stars. The two names the facade deliberately spells differently — `on`
becomes `ink_on`, `NAMES` becomes `ICONS` — are ordinary declarations layered
over what the stars bound locally:

```petal ignore
export import bloom/theme: *
export let ink_on = on
```

The immediate payoff was the thing the hand-written list could not do: two
exports (`motion.step` and `interact.capture_owner`) had been added to modules
and never added to the facade. The stars published them without an edit, which
is what the `VERSION` bump to 2 records.

### 4. A selective import list must fit on one line — fixed

A line break is now allowed after the `:` of a selective import and after each
`,`, and a trailing comma is legal:

```petal ignore
import ui: mix, over, contrast_text, ui_theme, pad,
           draw_rect_rounded, draw_text

import bloom/theme:
  theme, ts, tone,
  hair, stroke,
```

The list continues onto a later line only when that line starts with an
identifier followed by `,` or the end of the line, so a trailing comma cannot
swallow the statement after it. `import m` and `import m as u` end at their
newline as before.

bloom used to repeat the statement (`import bloom_theme: …` twice) to fit; the
repeats are gone. One caveat: `petal lint` is token-driven and re-indents
continuation lines to the statement's own indent, so it will offer to flatten a
hanging indent. bloom keeps the hanging indent and does not run lint over
itself; the two forms parse and run identically.

### 5. There is no package or manifest concept — fixed

Distribution *was* "copy a directory, then make it reachable" (`-I`,
`PETAL_PATH`, beside the script, or `register_module` per file), with nothing
recording a library's name, version, module list, or that its modules belong
together. A `petal.toml` at a library's root now does:

```toml
[package]
name = "bloom"
version = "0.1.0"
modules = "src"      # optional; defaults to src/, else the manifest's dir
```

Its modules are then importable as `bloom/menu`, wherever the directory sits —
the *manifest* name is the package name, so a library keeps its identity
rather than inheriting whatever directory a user dropped it in. `-I` and
`PETAL_PATH` pick up packages (a directory with a manifest, or one directory
of them), `petal packages` lists what that made available, and a host
registers a whole library in one call — `env.add_package(root)` from disk,
`env.register_package(name, sources)` from `include_str!` — which is what
bloom's nine-`include_str!` Garden integration was waiting for. Manifest
errors name the file and the line.

Two details worth knowing, both of which bloom leans on:

- A module named like its package is the **facade slot**. `src/bloom.ptl` is
  what a bare `import bloom` finds, so the front door survives the move to
  namespaced modules and every existing `import bloom` kept working untouched.
- `-I` now points at the library root (or at a directory of libraries), not at
  its `src/`. Everything in the repo that drove bloom with
  `-I petal-libs/bloom/src` says `-I petal-libs` now.

Deliberately absent: registry, fetching, dependency resolution, lockfile,
version constraints. `version` is metadata. See
[module-system.md](../module-system.md#packages).

### 6. Overload sets do not merge across modules — fixed, with two edges

A library *could* not add an overload to a name another module owned — it could
only shadow the whole set. Now a binding that lands on a name another module
already put in scope joins its set by arity:

```petal ignore
// lib.ptl — no `import ui` anywhere
export fn draw_rect(r)               // adds arity 1 to the prelude's set
  ui.draw_rect(r, {r: 9, g: 9, b: 9})
end
```

Merging happens only between function sets the compiler built; a `let`, a
record or a builtin native still shadows wholesale. The higher-precedence side
wins a colliding arity, silently. See
[function-overloading.md](../function-overloading.md).

**Edge one: arity is the only dispatch key, so bloom still exports `ts_a`.**
The motivating case was `draw_text(s, pos, style, alpha)` — but that is arity
4, and `ui` already defines arity 4 as `draw_text(text, pos, size, c)`. Adding
bloom's would not extend the set, it would take that slot over and break every
call to the prelude's. Merging is a real fix for *adding* an arity; it does
nothing for wanting a second meaning of one you already have. `ts_a(t, size,
color, a)` stays, and it is arguably the better API anyway: the alpha rides on
the style record, which is where the host looks for it.

**Edge two, and this one bites: inside one variant's body, the name cannot see
its siblings.** A variant's own name is bound to that variant (the
self-recursion binding), so a call at another arity falls *outward* — and now
that implicit imports reach every module (gap 1), outward is the host prelude.
The classic two-line default-argument idiom silently changes meaning:

```petal ignore
export fn context_menu(area, items)
  context_menu(area, items, {})   // ← the *prelude's* arity 3, not this file's
end

export fn context_menu(area, items, opts)
  …
end
```

That is only a problem when the library and the prelude share a name, but a
component library shares a lot of them: bloom collides with `ui` on `button`,
`checkbox`, `radio_group`, `slider`, `text_field`, `context_menu` and
`tooltip`. All seven are the two-arity default-argument shape, so all seven
broke — silently, drawing the host's flat widget instead of bloom's, no error
except where the argument shapes happened to disagree.

The library-side fix is to route both arities through a private implementation
the prelude cannot reach:

```petal ignore
export fn context_menu(area, items)
  context_menu_impl(area, items, {})
end

export fn context_menu(area, items, opts)
  context_menu_impl(area, items, opts)
end

fn context_menu_impl(area, items, opts)
  …
end
```

bloom does that for its seven, and the Garden panel tests that had started
failing pass again. But it is a workaround for something a library author will
not see coming, and the real fix is in the compiler: a variant's body should
resolve its own name to its module's complete set, not to the one closure. Any
library that defines an overload of a prelude name is exposed until then.

### 7. Small syntax edges

- `match` has no `else` arm; the catch-all is `when _ ->`.
- An `if` used as an expression still needs its `end`:
  `(if on then 1.0 else 0.0 end) * k`.

Neither is a gap so much as a thing every library author will hit once.

## Host-level gaps (all fixed here)

**A panel host that compiles with `env.load_program(&source)` has no importing
directory, so *no* import of a sibling file can resolve.** Garden's panel host
did exactly that, which meant a Petal panel could not be split into two files.
Fixed: `PanelHost::load` now uses `load_program_at`, and `poll_reload` uses
`compile_program_at`. The shorter, path-less API being the obvious one to reach
for is worth a second look.

**Hot reload watched only the entry file.** A panel that imports a module never
noticed that module changing. Fixed with `env.module_manifest(program_id)`,
which lists every file a program compiled from — the host now stats all of them.
That loop is small enough that `Env` could offer it directly.

**The headless driver had no `-I`.** `petal-ui-run` could only import from the
app's own directory, so a UI app that used a shared library could not be run
headlessly without copying the library next to it. Fixed:
`petal-ui-run … -I <dir>` (and `Headless::from_file_with_paths`), plus an
`include` list in a verify plan so the refactor verifier can drive such apps.

**Registering a whole directory of modules in one call** — bloom's Garden
integration was nine `include_str!`s and a loop — and **telling a script which
libraries are available** are both covered by packages now:
`env.add_package(root)` / `env.register_package(name, sources)` register a
library in one call, and `env.packages()` (or `petal packages`) lists what is
there. Garden's `garden-script/src/bloom.rs` is one `register_package` call
over the same `include_str!` table, and it is what makes `import bloom/menu`
resolve in a drawer pushed over a socket, which has no directory of its own.

## Testing notes

**The headless wall clock moves now.** `Headless::frame` advances `time()` by
1/60 s after every frame from a deterministic t0 = 0, so frame *N* sees
`time() == N/60` and `time()` tracks the `dt` the harness was already
publishing. It never reads the system clock, so traces stay byte-identical run
to run, and an animation written against `time()` (the `ui` prelude's
`spinner`, `elapsed`) is finally testable. Assigning `ui.time` still wins for
the next frame, and the advance resumes from there.

**A `state` key contains the import path that reached it,** and both halves of
that path moved in this migration. bloom's animators used to key as
`switch#1/switch/spring#2/…` (through the facade) and
`bloom_controls::switch/switch/spring#1/…` (through the module); they now key as
`bloom/controls::switch/…` and `bloom/motion::v`, because a namespaced module's
identity is its path and a star re-export routes through the defining module
rather than through a per-callsite `let`. Tests and tools that key on state
paths should match on the animator's own variable, not the whole path — the
Garden panel tests do, and needed a one-word change rather than a rewrite.

**Renaming a library's modules changes every trace that uses it, but only in
the `state` keys.** Migrating bloom left all four bloom-driven apps
byte-identical in `commands`, `prints`, `result` and `error` on all 60 frames,
and identical in state *values*; only key spellings moved. That made it easy to
see the one place where something really did change — a control in the gallery
that had been drawn by `ui`'s flat overload and is drawn by bloom's rounded one
again (gap 6, edge two).
