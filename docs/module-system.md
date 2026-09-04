# Module system

A Petal program can be split across files. Each file is a *module*; `import`
pulls another module's exported names into the current file. All the
modules of a program are compiled together into one program — there is no
separate compilation and no runtime linker.

## Importing

A module name is a *path* of one or more identifier segments joined by `/`,
and it names the file `<path>.ptl` — each leading segment a directory.
There are four forms:

```petal ignore
import ui                       // qualified: ui.button(...), ui.palette
import ui: button, clicked      // selective: button(...), clicked
import ui as u                  // alias: u.button(...)
import ui: *                    // every export, bound weakly (see Re-exporting)
```

Import statements must come before every other statement in the file.
Putting one later is an error:

```
Error: import statements must appear before any other statement
```

A module name is only usable through `.`: writing `ui` on its own
(`let x = ui`, `print(ui)`) is an error suggesting `ui.<name>` or a
selective import.

### Namespaced paths

A path groups a library's modules under a directory, so two libraries that
both ship a `menu.ptl` can be used in one program:

```petal ignore
import bloom/menu                // binds `menu`: menu.open(...)
import petal/menu as pmenu       // binds `pmenu`: pmenu.open(...)
import bloom/menu: open, close   // selective, as usual
```

The **local name is the last segment** — qualified access always goes
through it (`menu.open`), never through the path. `as` overrides it, and
two paths ending in the same segment need one: importing both `bloom/menu`
and `petal/menu` bare is the ordinary alias collision.

```
Error: 'menu' is already an alias for module 'bloom/menu' and cannot also alias 'petal/menu'
```

The **full path is the module's identity** — for deduplication, cycle
detection, `state` keys, qualified export names (`bloom/menu::open`), and
`module_manifest`. `bloom/menu` and `petal/menu` are two modules with two
sets of top-level state, and each loads once. A nested module's own imports
resolve against *its* directory, so `bloom/menu.ptl` reaches its sibling
with a flat `import motion`.

A segment is an identifier, which leaves no way to write a path that climbs
out of the directory it resolves against:

```
Error: a module path segment must be an identifier ('.' and '..' are not allowed in an import path)
```

Errors from a nested module name it by path (`bloom/menu.ptl line 2`),
since two namespaces may well ship the same file name.

## Exporting

A module's top-level declarations are private unless marked `export`:

```petal ignore
// ui.ptl
export fn button(label)      // importable: ui.button, `import ui: button`
  "[" ++ label ++ "]"
end

fn helper(x)                 // private to ui.ptl
  x + 1
end
```

`export` goes directly before the declaration keyword: `export fn`,
`export let`, `export var`, `export state`, `export enum`, `export class`.
An `export enum` exports its variants; an `export class` exports its
constructor and its type name. `export` in the entry file is allowed but
does nothing, since nothing imports the entry file.

A leading underscore carries no meaning: `fn _helper` is private because it
lacks `export`, not because of the name.

Naming something that is not exported is an error. A selective import
fails at compile time and lists what the module does export:

```
Error: module 'ui' has no export 'helper' (exports: button, palette, twice)
```

A qualified access (`ui.helper()`) fails when it runs:

```
Error: module 'ui' has no export 'helper' (declarations are private unless marked `export`)
```

### Re-exporting

A **facade** module presents one import surface over a library's several
implementation modules. `export import` builds it declaratively — the
facade names the modules, not each of their exports, so a name added to an
implementation module needs no edit here:

```petal ignore
// bloom.ptl — the whole facade
export import bloom/button: *        // every export of bloom/button
export import bloom/theme: accent    // just these
export import bloom/menu             // the module binding itself
```

An importer of `bloom` sees those names as if `bloom` had declared them:

```petal ignore
import bloom: button, accent, menu
button("ok")        // bloom/button's, whole overload set and all
menu.open()         // a module alias, passed on by the bare form
```

The three forms:

- **`export import m: *`** re-exports every export of `m` under its own
  name, *and* binds it locally, so the facade can use what it passes on.
  Whole overload sets travel: a `button` with two arities arrives with both.
- **`export import m: a, b`** re-exports exactly those names. Naming
  something `m` does not export is the usual compile error —
  `module 'bloom/menu' has no export 'nope' (exports: close, open)` — which
  is the check a hand-written facade could not give you.
- **`export import m`** re-exports the module *binding*. A module name is
  not a value, so what travels is the alias: an importer that names it
  (`import bloom: menu`) gets a module alias of its own and writes
  `menu.open()`. `bloom.menu.open()` does **not** work — there is no
  value to reach through.

`*` and `export` are independent. A plain `import m: *` binds the whole
surface locally without re-exporting it, and a nested path works in every
form (`export import bloom/button: *`).

**A star is the weakest explicit binding in the file.** It never fights:

- a top-level declaration in the facade wins over a star, silently, and so
  does a name the file imports by hand — whichever order the statements
  appear in;
- a star composes with [overload
  merging](function-overloading.md#sets-merge-across-modules): a star over a
  name a host prelude already provides merges by arity rather than replacing
  the set;
- but **two stars offering the same name** is a genuine ambiguity. They
  merge when both are function sets (the later star wins each arity it
  defines); otherwise it is an error naming both modules:

```
Error: bloom.ptl: 'shared' is re-exported by both 'a' and 'b' — name one of them explicitly, or drop it from one side
```

Chains work — a facade over a facade re-exports what it received — and a
re-export cycle is caught by the ordinary cycle check
(`import cycle: a -> b -> a`) rather than hanging. Re-exporting does not
widen privacy: a star only ever carries names the target marked `export`.

### Classes and methods

Methods are program-wide. A module that declares `fn Rect.area(r: Rect)`
gives every `Rect` in the program that method, and an importer can add its
own methods to a class it imported.

The class *name* follows `export`, and it is one name for both uses: the
constructor `Circle(...)` and the type in an annotation `c: Circle`. An
unexported class is private in both positions; using it as a type elsewhere
is the usual `unknown type name` warning.

```petal ignore
// shapes.ptl
class Hidden          // no `export`
  a: int
end
export class Circle
  radius: int
end

// app.ptl
import shapes
fn f(c: Circle) c.radius end   // fine — `Circle` is exported
fn g(h: Hidden) h.a end        // warning: unknown type name `Hidden`
```

Class names are unique across the whole program, exported or not. Two files
may not both declare `class Dup`:

```
Error: dup_b.ptl: class `Dup` is already declared in `dup_a.ptl`, so `dup_b.ptl` may not declare it too
```

### An exported `var` is read-only to importers

Importers can read an exported `var` but not write it. The cell belongs to
the module that declared it:

```petal ignore
import tally: hits
set hits = 5     // Error: `hits` is a `var` exported by module `tally`;
                 //        only `tally` can write it — call a function it exports instead
```

Export a function that does the write instead. Reading an imported `var`
inside a function still needs [`get`](language-guide.md#get), because it is
still a cell read and the importer sees the owner's writes as they happen:

```petal ignore
import tally: hits, bump

bump()
bump()
print(hits)              // 2

fn describe()
  "hits: {get hits}"     // live: reflects whatever `tally` has written
end
```

The qualified form `tally.hits` needs no `get`; it reads the current value
where it is written.

### Overloaded functions

The variants of an [overloaded function](function-overloading.md) share one
name, so either all of them are `export`ed or none. A mixed group is an
error:

```
Error: overloaded function 'f' has mixed export markers: mark all overloads 'export' or none
```

Importing `f` from two modules by hand is a collision like any other. Sets
declared in different modules do merge by arity where one binding lands on
another — see [Sets merge across
modules](function-overloading.md#sets-merge-across-modules).

## Where modules are found

`import name` looks for the module in this order. The first hit wins.

1. **Modules registered by the host** in memory (`env.register_module`),
   keyed by the full path — `register_module("bloom/menu", …)` is what
   `import bloom/menu` finds. This is how an embedding app ships a Petal
   prelude, and how a browser host with no filesystem works.
2. **The importing file's directory** — `<dir>/<path>.ptl`, each leading
   segment a directory (`<dir>/bloom/menu.ptl`). Two scripts side by side
   can share a `palette.ptl` next to them.
3. **A registered package** — a library with a `petal.toml`, whose modules
   answer to `<package name>/<module>` wherever the library sits. See
   [Packages](#packages).
4. **Search directories** added with `petal run -I <dir>` (repeatable, and
   accepted by every command that compiles) or `env.add_module_path`.
5. **`PETAL_PATH`** — colon-separated directories from the environment.

Note that `petal run -e '...'` has no importing file, so step 2 finds
nothing; add `-I .` to import from the current directory.

A missing module is an error naming the importer and the places searched:

```
Error: cannot find module 'geom' (imported by g.ptl): not registered, and no geom.ptl in the importing file's directory, module paths, or PETAL_PATH
```

Each module is loaded at most once per program. If `a` and `b` both import
`base`, there is one `base`, and its top-level code runs once. Import cycles
are an error that shows the path:

```
Error: import cycle: cyc1 -> cyc2 -> cyc1
```

### The standard prelude

Every program implicitly imports `std`, the part of the standard library
written in Petal (`sum`, `first`, `count`, `find`, `take`, `mean`,
`clamp01`, ...). Its names are available bare, with no `import`. They are
weak bindings: a declaration of your own with the same name shadows them
silently. `std` is only merged into a program that uses one of its names,
so it costs nothing otherwise.

An embedding host can add its own implicit imports the same way; see
[Embedding](#embedding) below.

## How a multi-file program runs

- A module's top-level statements run **exactly once**, before the code of
  any file that imports it. Modules run in dependency order, then the entry
  file. So `let palette = {...}` at a module's top level is computed before
  any importer touches `palette`. Keep top-level side effects small; the
  language does not forbid them.
- Builtins are visible in every file.
- Each file has its own scope. A `let x` in one module never collides with
  a `let x` in another.

### Collisions are loud, shadowing is quiet

A selective import is an explicit request, so a conflict is a compile
error naming both sides:

- `import a: draw` and `import b: draw` —
  `'draw' is imported from both 'a' and 'b'`.
- `import a: draw` plus a top-level `fn draw` in the same file —
  `'draw' is imported from 'a' but is also declared in this file`.
- `import a as x` and `import b as x` —
  `'x' is already an alias for module 'a' and cannot also alias 'b'`.

Ordinary shadowing stays silent, as everywhere in Petal: `let m1 = 5` after
`import m1` just rebinds the name.

### `state` in modules

A [`state`](language-guide.md#state) slot is keyed by where it is declared,
and the module is part of that key. Two modules declaring `state scroll`
get two slots, as do two functions declaring `state row`. The entry file's
keys are unchanged by the module system, so a single-file program's
hot-reload state is unaffected.

Moving a `state` declaration to another file, or renaming a module, changes
its key and drops that state on the next reload — the same as renaming the
variable.

## Errors name the file

An error in the entry file looks as it always has:

```
Cannot add int and nil [line 4, column 3]
```

An error in an imported module names it:

```
Cannot add int and nil [bad.ptl line 2, column 3]
```

Source snippets, `Caused by:` provenance and stack traces all show the
right file.

## Not supported

- Dotted module names (`import lib.geom`) and path strings. Nesting is
  spelled with `/`; use `-I` or `PETAL_PATH` to reach other directories.
- Imports anywhere but the top of a file, including conditional imports.
- Reaching a re-exported module through the facade as a value
  (`bloom.menu.open()`); module names are not values.
- Registries, fetching, version constraints and lockfiles. A `petal.toml`
  names a *local* library and lists nothing but itself (see
  [Packages](#packages)); its `version` is metadata that nothing resolves on.
- Distributing a module as compiled IR. Programs always compile from
  source.

## Packages

A `petal.toml` at the root of a directory turns it from "some `.ptl` files"
into a library with a name:

```toml
[package]
name = "bloom"
version = "0.1.0"
modules = "src"      # optional: where the modules are.
                     # Defaults to src/ when it exists, else the manifest's
                     # own directory.
```

Every `.ptl` file under the module directory is then importable as
`<name>/<module>`, nested directories included:

```petal ignore
import bloom/menu
import bloom/widgets/button
import bloom/menu: open, close
```

The **manifest name is the package name**, not the directory name — a library
keeps its name wherever a user drops it, and two copies cannot be told apart
by where they were unpacked. A module whose name matches the package
(`src/bloom.ptl`) is reachable as a bare `import bloom`, which is where a
facade goes.

Inside the library, modules reach each other by their plain flat names —
`bloom/menu.ptl` says `import motion` — or by the package-qualified path,
`import bloom/motion`. Pick one spelling per library and keep to it: a module
imported both ways in one program is two modules, because a module's identity
is the path the importing file wrote (see [How a multi-file program
runs](#how-a-multi-file-program-runs)).

**Making a package reachable.** From the command line, `-I` finds packages:
a directory holding a `petal.toml`, and every directory directly under one,
is registered as a package. `PETAL_PATH` directories are searched the same
way.

```bash
petal run app.ptl -I ~/petal-libs      # ~/petal-libs/bloom/petal.toml → `bloom`
petal packages -I ~/petal-libs         # what that made available
```

`petal packages` lists each library with its version, its directory, and its
modules. A `petal.toml` that will not parse is an error naming the file and
the line — a library the user pointed at never goes quietly missing.

From a host, a package is one call (see [Embedding](#embedding)):

```rust
env.add_package("petal-libs/bloom")?;                 // from disk
env.register_package("bloom", [("menu", MENU_SRC)])?; // from memory
env.packages();                                       // what is available
```

There is no registry, no fetching, no dependency resolution and no version
solving. A manifest records a library's identity; making it reachable is
still a matter of `-I`, `PETAL_PATH`, or a host call.

## Shipping a library

A library written in Petal is a directory of modules plus, usually, a facade
module that re-exports them — `export import bloom/button: *` per
implementation module, which carries whole overload sets and errors on a name
that is not there (see [Re-exporting](#re-exporting)). Give it a
[`petal.toml`](#packages) and the directory becomes a named package: users
reach it through `-I`, `PETAL_PATH`, `env.add_package`, or (with no manifest)
a copy beside their script or a host's `register_module`.
Put the directory under a namespace and its modules import as
`bloom/menu`, `bloom/motion` — no filename prefixes, and no collision with
another library's `menu`. [`petal-libs/`](../petal-libs/README.md) is where
this repo's own live, and [Sharing Petal
libraries](dev/sharing-petal-libraries.md) covers what works and what a
library author still has to work around.

## Embedding

Hosts that embed Petal manage modules through `Env`:

```rust
env.register_module("ui", include_str!("ui.ptl")); // in-memory module; wins over files
env.add_module_path(dir);                          // filesystem search path (+ packages under it)
env.add_package("petal-libs/bloom")?;              // a whole petal.toml library, one call
env.register_package("bloom", modules)?;           // the same, from in-memory sources
env.packages();                                    // every registered package
env.set_implicit_imports(&["ui"]);                 // every program imports ui's exports bare
env.load_program_at(&source, &path)?;              // entry file, with importer-relative resolution
env.compile_program_at(pid, &source, &path)?;      // hot-reload recompile
env.module_manifest(pid);                          // every source file: name, origin, content hash
```

Implicit imports give user scripts a host's prelude with no ceremony:
they call `button(...)` directly, their own declarations shadow it
silently, and an explicit `import ui` on top is a no-op. `std` is kept in a
separate list, so `set_implicit_imports` cannot drop it; both the host
prelude and user code shadow `std`.

`module_manifest` lists every file a program was compiled from, so a host
can watch all of them and hot-reload when an imported file changes.
Reloading is a recompile plus `transfer_state`; state whose key survives is
preserved. Module functions are addressable by qualified name
(`env.call_function(stack, "ui::button", args)`), and module `state`
appears in state JSON under `ui::scroll`-style names.

Custom resolvers implement the `ModuleResolver` trait in
`rust/src/module.rs`. The wasm bindings expose `register_module` and
`set_implicit_imports`. See the [Embedding guide](embedding-guide.md) for
the rest of the host API.
