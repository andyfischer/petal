# Module system

A Petal program can be split across files. Each file is a *module*; `import`
pulls another module's exported names into the current file. All the
modules of a program are compiled together into one program — there is no
separate compilation and no runtime linker.

This page covers the `import` forms, which name wins, how a module is
identified and found, and what a host does to offer its own modules.
[Packages](packages.md) covers the `petal.toml` manifest, dependencies, the
lockfile and the package commands.

Nothing arrives without an import except the core builtins and
[`std`](#the-standard-prelude). A UI script says `import ui`; a script that
calls `lerp` says `import creative`; a host's own functions come from the
host's package.

## Importing

There are three forms:

```petal ignore
import hud                      // every export bare, plus the qualifier: draw_score(), hud.draw_score()
import hud as h                 // the same, with the qualifier renamed: draw_score(), h.draw_score()
import hud: draw_score, margin  // only these names, and no qualifier
```

A **plain import** is the default. It binds every name the module exports,
bare, and it binds the module's *qualifier* — the last segment of the path —
so both `draw_score()` and `hud.draw_score()` work. `as` renames the
qualifier. A **selective import** names exactly what it wants and binds
nothing else, the qualifier included.

Import statements must come before every other statement in the file.
Putting one later is an error:

```
Error: import statements must appear before any other statement
```

A qualifier is only usable through `.`: writing `hud` on its own
(`let x = hud`, `print(hud)`) is an error suggesting `hud.<name>` or a
selective import — unless the name is also a value: the module exports a
name spelled like itself (`import noise` where `noise` exports `fn noise`),
or a builtin is called that. Then the bare name is the value, and
`noise.fbm(…)` still reaches the module's exports. A declaration of the
file's own (`let noise = …`, a parameter called `noise`) takes the name
back entirely, as shadowing always does.

### Which name wins

A bare name is looked up in four tiers, strongest first:

1. a declaration in the file,
2. a name imported selectively,
3. a name from a plain import,
4. the implicit prelude — the core builtins and [`std`](#the-standard-prelude).

A higher tier shadows a lower one silently, like any other shadowing in
Petal: a file that declares `fn draw` and imports a module exporting `draw`
gets its own.

**Two plain imports that offer the same name are not an error at the
import.** Nothing is wrong until the file uses the name bare; then it is an
error at each such use, naming both modules:

```petal ignore
import ./hud       // exports draw
import ./level     // exports draw

hud.draw()         // fine — qualified access always works
level.draw()
draw()             // Error: `draw` is ambiguous: 'hud' and 'level' export it — write
                   //        `hud.draw` or `level.draw`, or pin one with `import ./hud: draw`
```

So a game whose modules all export `draw`, `step` and `reset` imports them
all plainly and calls each through its qualifier, and a
module growing a new export can never break a file that does not use that
name bare. Fix an ambiguity by qualifying the use, or by pinning the name
with a selective import beside the plain ones (`import ./level: draw`).

Three refinements:

- **An importer's export shadows the export of a module it imports.** If
  `kit` (directly or through other modules) imports `base`, and both export
  `button`, a file that plainly imports both gets `kit`'s — `kit` is built on
  `base` and redeclared the name on purpose. Import order never matters.
- **The same declaration reached by two routes is one name.** Importing a
  module and a facade that re-exports it is not an ambiguity.
- **Function sets with different parameter counts merge.** If `hud` exports
  `score(points)` and `level` exports `score(points, bonus)`, both are
  callable bare — with a `check` warning on each call through the merged
  name, since which module answers depends on how many arguments were
  written. The same parameter count from two unrelated modules is the
  ambiguity error above, at the call.

A plain import that takes a name away from a builtin or a `std` function is
legal, and worth knowing about: `check` warns once per name per file, at the
first bare use.

```
warning: `clamp` here is the one module 'tween' exports, which hides the builtin `clamp` —
write `tween.clamp` to say so, or `import ./tween: clamp` to take it on purpose
```

Qualifying the use or importing the name selectively says it was meant, and
the warning goes. First-party packages that redefine builtin names by design
(the packages bundled in the binary: `ui`, `bloom`, …) do not raise it.

The warning is about the names that are in scope with no import (`core` and
`std`). A domain builtin such as `lerp` arrives through a plain import of its
own (`import creative`), so a second module exporting `lerp` is two plain
imports offering one name: the ambiguity error above, at each bare use.

### Import paths

What follows `import` says where the module comes from, and there are
exactly two kinds of path:

```petal ignore
import ./level            // a file beside this one: level.ptl
import ./ui/hud           // a file below it: ui/hud.ptl
import ../shared/rng      // a file above it
import bloom              // a package
import bloom/menu         // a module of a package
```

- **A relative path** starts with `./` or one or more `../`. It names a
  `.ptl` file relative to the *importing file*, each further segment a
  directory. Inside a [package](#packages) it may not leave the package root
  (the directory holding `petal.toml`); a loose script has no such limit.
- **Anything else starts with a package name.** A bare first segment is
  *never* a sibling file: `import level` does not find `level.ptl`, it looks
  for a package called `level`. The error says what to write instead:

  ```
  Error: cannot find module 'level' (imported by main.ptl): there is no package named
  'level' — a bare name is always a package. The file beside this one is `import ./level`
  (`petal fix-imports` rewrites these)
  ```

  [Where packages are found](#where-packages-are-found) lists the places a
  package name is looked up.

The **qualifier is the last segment** of either kind of path
(`import ./ui/hud` binds `hud`, `import bloom/menu` binds `menu`). Two plain
imports ending in the same segment need an `as`:

```
Error: 'menu' is already an alias for module 'bloom/menu' and cannot also alias 'petal/menu'
```

After a relative prefix every segment is an identifier, so a path cannot
wander mid-way:

```
Error: a module path segment must be an identifier ('.' and '..' are only allowed at the
start of an import path: `./name`, `../name`)
```

### Module identity

A module is identified by *what it is*, not by how an importer spelled it:

- a file under a package's module directory is `<package>/<path>` —
  `bloom/menu`, and plain `bloom` for the module named after its package;
- a module a host registered in memory is the name it was registered under;
- any other file (a *loose* file) is its canonical path on disk, and is
  *named* — in diagnostics, `state` keys, qualified function names and
  `module_manifest` — by its path from the entry file's directory: `level`,
  `ui/hud`, `../shared/rng`.

So `import ./motion` inside `bloom` and `import bloom/motion` from outside
it are **one module**: it loads once, its top-level code runs once, and
there is one set of its `state` and `var`s. The same holds for one loose
file reached by two different relative paths. Deduplication and cycle
detection work on that identity. Errors from a module name it the same way
(`bloom/menu.ptl line 2`).

## Exporting

A module's top-level declarations are private unless marked `pub`. That
includes a `native fn` — the declaration of a function the host implements
([ffi.md](ffi.md#native-interfaces-native-fn)): `pub native fn mouse_x() -> int`
exports `mouse_x` like any other name, which is how a host's interface module
(petal-ui's `ui`) hands its natives to scripts.

```petal ignore
// ui.ptl
pub fn button(label)         // importable: button, ui.button, `import ui: button`
  "[" ++ label ++ "]"
end

fn helper(x)                 // private to ui.ptl
  x + 1
end
```

`pub` goes directly before the declaration keyword: `pub fn`, `pub let`,
`pub var`, `pub state`, `pub enum`, `pub class`. A `pub enum` exports its
name, and its variants are members of that name (`palette.Color.red`); a `pub class` exports its constructor and its type name. `pub` in
the entry file is allowed but does nothing, since nothing imports the entry
file.

`pub` used to be spelled `export`. The old word still works everywhere `pub`
does, with no removal planned, and means exactly the same thing; `petal check`
and the language server flag it as deprecated, and `petal lint --fix` (rule
`prefer-pub`) or `petal fmt` rewrites it. Both words are reserved.

A leading underscore carries no meaning: `fn _helper` is private because it
lacks `pub`, not because of the name.

Naming something that is not exported is an error. A selective import
fails at compile time and lists what the module does export:

```
Error: module 'ui' has no export 'helper' (exports: button, palette, twice)
```

A qualified access (`ui.helper()`) fails when it runs:

```
Error: module 'ui' has no export 'helper' (declarations are private unless marked `pub`)
```

### Re-exporting

A **facade** module presents one import surface over a library's several
implementation modules. `pub import` builds it declaratively — the
facade names the modules, not each of their exports, so a name added to an
implementation module needs no edit here:

```petal ignore
// bloom.ptl — the whole facade
pub import ./button               // every export of bloom/button
pub import ./menu
pub import ./theme: accent        // just these
```

An importer of `bloom` sees those names as if `bloom` had declared them:

```petal ignore
import bloom
button("ok")        // bloom/button's, whole overload set and all
bloom.open()        // bloom/menu's
```

The two forms:

- **`pub import m`** re-exports every export of `m` under its own name,
  *and* binds it locally like any plain import, so the facade can use what
  it passes on. Whole overload sets travel: a `button` with two arities
  arrives with both.
- **`pub import m: a, b`** re-exports exactly those names. Naming
  something `m` does not export is the usual compile error —
  `module 'bloom/menu' has no export 'nope' (exports: close, open)` — which
  is the check a hand-written facade could not give you.

What a facade passes on is names, never the module itself: the qualifier a
plain `pub import` binds is local to the facade, so `bloom.menu.open()` does
not work. Import `bloom/menu` when you want the `menu` qualifier.

The facade's own imports follow [the usual tiers](#which-name-wins):

- a top-level declaration in the facade wins over a re-exported name,
  silently — a `pub fn` of its own is what gets exported, and a private
  declaration keeps the name out of the facade's surface;
- a re-export over a name a lower tier already provides composes with
  [overload merging](function-overloading.md#sets-merge-across-modules);
- **two `pub import`s offering the same name** follow the plain-import
  rules. Function sets with different parameter counts merge. Otherwise the
  name is ambiguous — which is *not* an error in the facade, where nobody
  used it. The error belongs to the file that finally uses the name, bare or
  as `facade.name`, and names the two original modules:

```
Error: `shared` is ambiguous: 'a' and 'b' export it — write `a.shared` or `b.shared`, or
pin one with `import ./a: shared`
```

Chains work — a facade over a facade re-exports what it received — and a
re-export cycle is caught by the ordinary cycle check
(`import cycle: a -> b -> a`) rather than hanging. Re-exporting does not
widen privacy: a `pub import` only ever carries names the target marked
`pub`.

### Classes and methods

Methods are program-wide. A module that declares `fn Rect.area(r: Rect)`
gives every `Rect` in the program that method, and an importer can add its
own methods to a class it imported.

The class *name* follows `pub`, and it is one name for both uses: the
constructor `Circle(...)` and the type in an annotation `c: Circle`. An
unexported class is private in both positions; using it as a type elsewhere
is the usual `unknown type name` warning.

```petal ignore
// shapes.ptl
class Hidden          // no `pub`
  a: int
end
pub class Circle
  radius: int
end

// app.ptl
import ./shapes
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
import ./tally: hits
set hits = 5     // Error: `hits` is a `var` exported by module `tally`;
                 //        only `tally` can write it — call a function it exports instead
```

Export a function that does the write instead. Reading an imported `var`
inside a function still needs [`get`](language-guide.md#get), because it is
still a cell read and the importer sees the owner's writes as they happen:

```petal ignore
import ./tally: hits, bump

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
name, so either all of them are `pub` or none. A mixed group is an
error:

```
Error: overloaded function 'f' has mixed `pub` markers: mark all overloads `pub` or none
```

Selectively importing `f` from two modules is a collision like any other.
Sets declared in different modules do merge by arity where one binding lands
on another in a lower tier, and where two plain imports offer different
arities — see [Which name wins](#which-name-wins) and [Sets merge across
modules](function-overloading.md#sets-merge-across-modules).

## Where packages are found

A [relative path](#import-paths) needs no search: it is a file, relative to
the importing one. A path that starts with a package name is looked up in
this order, and the first place that has a package of that name wins:

1. **What the host registered in memory**: a module registered under the
   full path (`env.register_module("bloom/menu", …)` is what
   `import bloom/menu` finds; `register_module("ui", …)` is the one-module
   package `ui`), then the packages it registered (`env.add_package`,
   `env.register_package`). This is how an embedding app ships a Petal
   prelude, and how a browser host with no filesystem works.
2. **The manifest.** The package the importing file belongs to — the one
   whose `petal.toml` is nearest at or above the file — answers for its own
   name (a library's modules and its examples can say `import bloom/menu`
   with nothing configured) and for every key of its
   [`[dependencies]`](packages.md#dependencies). The *project's* manifest — the one
   nearest the program's entry file — is asked next, which is how code with
   no manifest of its own (`-e` code run inside a project, the modules of a
   bundled or in-memory package) picks up the project's choice of a package.
   A `git` dependency is found where [the package manager](packages.md#the-package-manager)
   put it: `vendor/<name>` at the project root, else the cache, as
   `petal.lock` pins it. A `vendor/<name>` directory also answers for a name
   no manifest lists.
3. **The search path**: packages under the directories given with
   `petal run -I <dir>` (repeatable, and accepted by every command that
   compiles) or `env.add_module_path`, then under the `PETAL_PATH`
   directories. A search directory offers the package it *is* (when it holds
   a `petal.toml`) and each package directly under it. It is a place to find
   packages, not loose files: `-I lib` does not make `lib/util.ptl`
   importable as `util`.
4. **The packages bundled in the `petal` binary**, which every embedding
   of the runtime carries as well:

   | Package | What it is |
   | --- | --- |
   | `core` | the declarations of the global builtins (`import core` binds the qualifier `core`, so `core.len(xs)` reaches a builtin a declaration of yours has shadowed) |
   | `noise`, `color`, `vec2`, `autodiff`, `json`, `io`, `creative` | the domain builtins: natives every runtime registers, in scope only where the package is imported. See [Builtins](Builtins.md#which-builtins-need-an-import) for which names each one declares |
   | `ui` | the petal-ui interface — the `native fn` declarations of every petal-ui native and the widget library written over them. `ui/input` is its input, timing and frame half alone |
   | `bloom` | the component library ([core-runtime/bloom](../core-runtime/bloom)) |
   | `text_layout` | text placement ([core-runtime/text-layout](../core-runtime/text-layout)) |
   | `glsl` | Petal to GLSL, experimental ([core-runtime/glsl](../core-runtime/glsl)) |

   A bundled package is Petal source and `native fn` declarations only, so
   `petal check` resolves `import ui` with no host. *Running* a script that
   uses a declared native needs a host that registers it; see
   [Host interface packages](#host-interface-packages). A package of the
   same name found at an earlier step — a host registration, a
   `[dependencies]` entry of the project — wins over the bundled copy.

Every command that compiles, and the language server, finds the manifest by
walking up from the file it is given. Inside a project nothing needs a flag.

Within the package, the rest of the path names the module:
`import bloom/widgets/button` is `widgets/button.ptl` under the package's
module directory, and a bare `import bloom` is the module named after the
package (`bloom.ptl`).

`petal run -e '...'` and code piped on stdin have no importing file;
`import ./x` there means `x.ptl` in the current directory.

A missing module is an error naming the importer and why:

```
Error: cannot find module 'geom' (imported by g.ptl): there is no package named 'geom':
not registered by the host, not the importing file's own package or a [dependencies]
entry of its petal.toml, not on a search path (-I, PETAL_PATH), and not bundled. A file
beside this one is imported as `./geom`
```

Each module is loaded at most once per program, [whatever it was imported
as](#module-identity). If `a` and `b` both import `base`, there is one
`base`, and its top-level code runs once. Import cycles are an error that
shows the path, and so is importing the program's entry file:

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

`std` and the `core` builtins are the only names in scope with no import. A
script that uses a host's names imports the host's package (`import ui`),
like any other library, and the domain builtins are behind imports too
(`import creative`, `import vec2`, …; see
[Builtins](Builtins.md#which-builtins-need-an-import)).

## How a multi-file program runs

- A module's top-level statements run **exactly once**, before the code of
  any file that imports it. Modules run in dependency order, then the entry
  file. So `let palette = {...}` at a module's top level is computed before
  any importer touches `palette`. Keep top-level side effects small; the
  language does not forbid them.
- The `core` builtins and `std` are visible in every file. Every other name
  is visible only in a file that imports it: an import in one module does
  not reach the files that import that module.
- Each file has its own scope. A `let x` in one module never collides with
  a `let x` in another.

### Collisions are loud, shadowing is quiet

A selective import is an explicit request, so a conflict with another
explicit request is a compile error naming both sides:

- `import ./a: draw` and `import ./b: draw` —
  `'draw' is imported from both 'a' and 'b'`.
- `import ./a: draw` plus a top-level `fn draw` in the same file —
  `'draw' is imported from 'a' but is also declared in this file`.
- `import ./a as x` and `import ./b as x` —
  `'x' is already an alias for module 'a' and cannot also alias 'b'`.

Everything else is [the tier order](#which-name-wins), and it is silent: a
declaration over a plain import, a selective import over a plain one, any
of them over the prelude. `let m1 = 5` after `import ./m1` just rebinds the
name. Two plain imports that offer one name are neither: they are an error
only where the name is used bare.

### `state` in modules

A [`state`](language-guide.md#state) slot is keyed by where it is declared,
and the module is part of that key. Two modules declaring `state scroll`
get two slots, as do two functions declaring `state row`. The entry file's
keys are unchanged by the module system, so a single-file program's
hot-reload state is unaffected.

Moving a `state` declaration to another file, or renaming a module, changes
its key and drops that state on the next reload — the same as renaming the
variable. The module half of the key is the module's [canonical
name](#module-identity), so it does not depend on how any importer spelled
the path — but a loose file's name is its path from the entry file, and
moving a directory of modules into a package (giving it a `petal.toml`)
renames them from `level` to `<package>/level`.

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
right file. A stack trace that crosses into a module names the entry file on
its own frames too, so no frame is left bare beside a named one:

```
Stack trace:
  in boom() [m.ptl line 6, column 3]
  in go() [main.ptl line 3, column 3]
  in outer() [main.ptl line 5, column 1]
```

(A trace that stays in the entry file, and `-e` code, which has no file
name, keep the bare `[line N, column M]`.)

## Migrating with `petal fix-imports`

Code written before these rules does not load: it imports sibling files by
bare name, writes `import m: *`, or uses a host's names and the domain
builtins with no import at all. `petal fix-imports` rewrites it:

```bash
petal fix-imports .                 # every .ptl file under the directory, in place
petal fix-imports --check .         # list what would change, exit 1 if anything would
petal fix-imports --diff src/       # show the rewrite as a diff
petal fix-imports -I libs game/     # with the packages under libs/ known
```

It changes import lines and nothing else:

| Before | After | When |
|---|---|---|
| `import level` | `import ./level` | `level.ptl` sits beside the file, and no package is called `level` |
| `import color` (with `color.ptl` beside the file) | `import ./color` | a package is also called that, and the file takes a name from it the package does not export (or writes `: *`). Left alone when everything it takes is the package's; a note when it takes nothing by name |
| `import rng` (found through `-I shared`) | `import ../shared/rng` | the same `-I` is passed to `fix-imports` |
| `import m: *`, `pub import m: *` | `import m`, `pub import m` | always |
| `import m: a` … `m.b` | `import m: a` + `import m` | the file reaches through `m.`, which a selective import no longer binds |
| *(nothing)* | `import kit` | the file uses a bare name nothing in it provides, and exactly one available package exports it |
| *(nothing)* | `import ui`, `import garden`, … | the same, for a host's names. When several packages export the name and all but one are built on that one (bloom's `button` over ui's), the one underneath is imported |
| *(nothing)* | `import creative`, `import vec2`, … | the same, for a builtin that moved into a package (`lerp`, `vec2`, `noise`, `hsv`, `json_parse`, …) |
| *(nothing)* | `import vec2` (for `.normalize()`) | the file calls a moved builtin with method syntax and nothing in the program defines a function, method or record field of that name |

A path that already resolves as a package is left alone, so a file written
for the current rules is unchanged and running the command twice does
nothing more. "Available" means the packages bundled in the binary, the
`[dependencies]` of the file's project, and the packages under `-I` and
`PETAL_PATH`. A name that several unrelated packages export is reported and
left for you to choose. A file carrying the comment
`// petal-fix-imports-ignore` on a line of its own gets no import added.

What it cannot do for you is decide an [ambiguity](#which-name-wins) the
new bare binding introduced: qualify the use, or pin the name with a
selective import.

## Not supported

- Dotted module names (`import lib.geom`) and path strings. Nesting is
  spelled with `/`, and a file is reached with `./` or `../`.
- Absolute paths, and a bare name for a sibling file.
- A star import (`import m: *`). A plain `import m` binds every export.
- Imports anywhere but the top of a file, including conditional imports.
- Reaching a module through a facade's qualifier
  (`bloom.menu.open()`); module names are not values, and a facade passes on
  names, not qualifiers.
- A package registry. `path` and `git` [dependencies](packages.md#dependencies)
  resolve; a version string is the error "registries are not supported yet".
- Distributing a module as compiled IR. Programs always compile from
  source.

## Packages

A `petal.toml` at the root of a directory turns it from "some `.ptl` files"
into a package with a name:

```toml
[package]
name = "bloom"
version = "0.1.0"
modules = "src"      # optional: defaults to src/ when it exists, else the manifest's directory

[dependencies]
text_layout = { path = "../text-layout" }      # the key is the import name

[requires]
ui = ">=0.1"                                   # a host interface the runner must provide
```

Every `.ptl` file under the module directory is importable as
`<name>/<module>`, nested directories included, and the module named after
the package (`src/bloom.ptl`) is a bare `import bloom`:

```petal ignore
import bloom
import bloom/menu
import bloom/menu: open, close
```

Inside the package, modules reach each other with a relative import —
`bloom/menu.ptl` says `import ./motion` — or by the package's own name,
`import bloom/motion`, which works from anywhere under the package root (its
examples and tests included). The two are [the same module](#module-identity).
A relative import cannot leave the package root.

[packages.md](packages.md) is the reference for all of it: the manifest keys,
the three dependency forms, `[requires]` and what a host provides, and how a
library is shipped.

### The package manager

`petal init`, `add`, `remove`, `install`, `update`, `vendor` and `bundle`
manage a project's dependencies: they resolve the graph to exactly one
version of each package, fetch git dependencies into a cache, pin them in
`petal.lock`, and can copy them into `vendor/` or emit them as one JSON
bundle for a host with no filesystem. `petal packages` shows the result.
`petal run` and `petal check` install on demand. See
[packages.md](packages.md#the-package-manager).

## Embedding

Hosts that embed Petal manage modules through `Env`:

```rust
env.register_module("ui", include_str!("ui.ptl")); // in-memory module: the package name `ui`
env.add_module_path(dir);                          // a directory of packages (or a package)
env.add_package("core-runtime/bloom")?;              // a whole petal.toml library, one call
env.register_package("bloom", modules)?;           // the same, from in-memory sources; a source named
                                                   // "petal.toml" is the package's manifest
env.packages();                                    // every registered package
env.verify_interface(&["ui", "game"])?;            // registered natives == what these packages declare
env.provides("game", "0.2.0")?;                    // this host provides the interface `game`
env.provides_package("game")?;                     // the same, at the registered package's manifest version
env.project_of(&path)?;                            // the package whose petal.toml is nearest above a file
env.project_dependencies(&path);                   // that manifest's [dependencies], as they resolve
env.set_install_on_demand(true);                   // fetch a missing git dependency at load (off by default)
env.set_inline_import_dir(Some(dir));              // where `./x` leads for a program with no path
env.resolve_import("./level", &path)?;             // the canonical module name an import resolves to
env.load_program_at(&source, &path)?;              // entry file: `./x` resolves against `path`
env.reload_program(stack, &source, Some(&path))?;  // hot reload: every source file diffed, the cheapest exact path taken
env.compile_program_at(pid, &source, &path)?;      // the recompile on its own
env.module_manifest(pid);                          // every source file: name, origin, content hash
```

A program loaded from a string (`load_program(source)`) has no file:
`import ./x` in it reaches a module registered in memory as `x`, and
nothing on disk unless the host opts in with `set_inline_import_dir`. A
relative import *between* in-memory modules resolves against the importer's
registered name — `register_module("bloom/menu", …)` saying
`import ./motion` finds `bloom/motion`.

### Host interface packages

A host adds no names to a script's scope by itself. A native is reachable
from a script only through a `native fn` declaration in scope
([ffi.md](ffi.md#native-interfaces-native-fn)), so a host ships an
**interface package** — the declarations of its natives, plus whatever it
writes in Petal over them — registers it, and its scripts import it:

```rust
env.register_native("spawn", native_spawn, NativeEffects::EMITS);
env.register_package("game", [("game", include_str!("../packages/game/game.ptl"))])?;
// game.ptl:   pub native fn spawn(kind: string, props: record)
// a script:   import game
//             spawn("ball", {x: 1})
```

Register the natives first: registering a package gives the natives already
registered the parameter names it declares, so `spawn(kind: "ball", …)` binds
by name. Keep the package in a directory with a `petal.toml` beside the Rust
code. A project whose manifest lists it as a path dependency
(`game = { path = "../host/packages/game" }`) is then checked by the stock
CLI and the language server with no flag and no knowledge of the host;
`petal check -I <that directory>` does the same for a loose script.

**Say what you provide.** A package that needs the host says so in its
manifest (`[requires] game = ">=0.2"`), and the host announces the interface
where it registers the natives:

```rust
env.register_package("game", [
    ("game", include_str!("../packages/game/game.ptl")),
    ("petal.toml", include_str!("../packages/game/petal.toml")),
])?;
env.provides_package("game")?;       // at the version in that petal.toml
// or: env.provides("game", "0.2.0")?;
```

The interface version is the interface package's version.
`petal_ui::register_all` provides `ui`; a host that registers petal-ui's
pieces itself calls `petal_ui::provide_interface(env)` once it has registered
all of them. A host that registers only part of an interface (the input half,
for `import ui/input`) does not provide it.

| Host | Package | Where it lives |
| --- | --- | --- |
| petal-ui (`petal_ui::register_all`) | `ui`, `ui/input` | `core-libs/petal-ui/prelude/` (bundled) |
| Garden panels | `garden` | `garden/garden-script/packages/garden/` |
| Garden's config host | `garden_config` | `garden/garden-script/packages/garden_config/` |
| petal-desktop-sdl | `sdl` | `integrations/petal-desktop-sdl/packages/sdl/` |
| the wasm element-tree runtime | `web` | `core/packages/web/` |
| the `petal-fps` example host | `fps` | `examples/custom-apps/petal-fps/packages/fps/` |
| the `petal-fantasy-nes` example host | `nes`, `nes/nes_host`, `nes/nes_sound` | `examples/custom-apps/petal-fantasy-nes/prelude/` |

Only `ui` is bundled in the `petal` binary. The others are found by the stock
CLI as path dependencies of the script's project (the example trees in this
repository carry a `petal.toml` that lists them) or with `-I`.

`Env::verify_interface(packages)` is the host's drift test: every native the
named packages declare is registered in the `Env`, and every native registered
beyond the builtins is declared by one of them. It returns an
`InterfaceMismatch { unregistered, undeclared, problems }`. A name may be a
package, one module of a package (`ui/input`, for a host that registers only
that half), or a module registered in memory. Each host in this repository
asserts it in a test.

Loading a program into an `Env` to run *links* it: every declared native the
program's code refers to must be registered, or the load fails before any
line runs, naming the module and the missing natives. A check
(`env.set_check_only(true)`, `petal check`, the language server) does not
link. Method-call syntax follows the same rule: `x.name(…)` falls back to the
native `name(x, …)` for a builtin, or for a host native declared (under its
own name) in the file that writes the call — that file, not some other file
of the program. A program loaded from serialized IR is linked too, by the
natives its calls name.

`module_manifest` lists every file a program was compiled from, so a host
can watch all of them and hot-reload when an imported file changes.
Reloading is a recompile plus `transfer_state`; state whose key survives is
preserved. Module functions are addressable by qualified name
(`env.call_function(stack, "ui::button", args)`), and module `state`
appears in state JSON under `ui::scroll`-style names.

Custom resolvers implement the `ModuleResolver` trait in
`core/src/module.rs`: `resolve(path, importer)` is handed the path as
written and the importing module's name and origin, and returns the source
with the module's canonical name. The wasm bindings expose `register_module`,
`register_package` (a JSON object of module name to source) and
`verify_interface`; the C bridge has `pb_vm_register_module`,
`pb_vm_register_package`, `pb_vm_add_package`, `pb_vm_verify_interface`,
`pb_vm_provides` and `pb_vm_provides_package`; the wasm bindings have
`provides` and `provided_interfaces` too. A host with no filesystem loads a
whole project from the output of [`petal bundle`](packages.md#bundles). See
the [Embedding guide](embedding-guide.md) and
[Embedding from C/C++](embedding-c.md) for the rest of the host API.
