# Sharing Petal libraries

Notes from writing [bloom](../../petal-libs/bloom/), the first library in
`petal-libs/`: what a pure-Petal library can already do, what it has to work
around, and which of those are language- or host-level gaps worth closing.

Everything below was hit while building a real library — a ~2,300-line UI
component set used by Garden panels and by an app in `examples/` — rather than
imagined at a whiteboard.

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

**A facade module re-exports cleanly.** `export let button = bloom_button.button`
re-exports a function *and its whole overload set*, so a library can present one
import surface (`import bloom`) over many implementation modules.

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

### 1. A host's implicit imports do not reach imported modules

The biggest one. A host registers its prelude as an implicit import
(`env.set_implicit_imports(&["ui"])`), and every name in it is available bare —
**in the entry file only**. In an imported module the same call resolves to the
raw native, or to nothing:

```petal
// lib.ptl
export fn box(r, c)
  draw_rect(r, c)     // Error: Expected int at arg 1, got record
end                   // — the *native*, not the prelude's record overload
```

The workaround is for every library module to `import ui: …` explicitly, which
bloom does. It works, but it means:

- a library must hard-code the *name* the host registered its prelude under, and
- the host's careful "scripts get this for free" contract stops at file one, so
  the moment a script grows a second file its calls change meaning.

The fix is either to apply implicit imports to every module in the program, or
to make it explicit that they are entry-file sugar and give a library a way to
ask for the same set.

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

Still missing above that: a manifest (gap 5) that would let a library declare
its own name rather than inheriting whatever directory a user dropped it in.

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

### 4. A selective import list must fit on one line

```petal
import ui: mix, over, contrast_text, ui_theme, pad,   // ← parse error at the
           draw_rect_rounded, draw_text                //   line break
```

Repeating the statement (`import ui: …` twice) is allowed and is what bloom
does, but a wrapped list is the obvious thing to write.

### 5. There is no package or manifest concept

Distribution is "copy a directory, then make it reachable" (`-I`, `PETAL_PATH`,
beside the script, or `register_module` per file). Nothing records a library's
name, version, module list, or that its modules belong together. A
`bloom.toml`-shaped manifest that `import` and the CLI understood would replace
both the prefix convention and the per-host registration loop.

### 6. Overload sets do not merge across modules

A library cannot add an overload to a name another module owns — it can only
shadow the whole set. bloom wants `draw_text(s, pos, style, alpha)` and cannot
add it to `ui`'s set; it exports `ts_a(...)` instead, which builds the alpha
into the style record.

### 7. Small syntax edges

- `match` has no `else` arm; the catch-all is `when _ ->`.
- An `if` used as an expression still needs its `end`:
  `(if on then 1.0 else 0.0 end) * k`.

Neither is a gap so much as a thing every library author will hit once.

## Host-level gaps (three fixed here)

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

**Two things a host still cannot do conveniently:** register a whole directory
of modules in one call (bloom's Garden integration is nine `include_str!`s), and
tell a script which libraries are available.

## Testing notes

**The wall clock stands still in a headless run while `dt` keeps flowing.**
`Headless` publishes a fixed `time()` unless the driver advances it, and
`petal-ui-run` does not — so any animation written against `time()` (the `ui`
prelude's `spinner`, `elapsed`) holds its value for a whole trace and cannot be
tested. bloom integrates `dt()` everywhere except two deliberately phase-locked
helpers, so its motion is exercised by every headless run. Advancing the
harness clock by `dt` would be a small change with a golden re-baseline behind
it.

**A `state` key contains the import path that reached it.** The same component
reached through the facade and through its own module keys as
`switch#1/switch/spring#2/…` and `bloom_controls::switch/switch/spring#1/…`.
Tests and tools that key on state paths should match on the animator's own
variable, not the whole path.
