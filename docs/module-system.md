# The Petal module system

Petal programs can be split across files (or embedder-provided sources) with
`import`. Modules are resolved and **merged into one program at compile
time** — there is no runtime linker and no separate compilation; the bytecode
VM runs merged programs unchanged.

```petal
import ui                       // qualified: ui.button(...), ui.palette
import ui: button, clicked      // selective: bare button(...), clicked(...)
import ui as u                  // alias: u.button(...)
```

## Syntax

- `import` statements are only allowed **before any other statement** in a
  file. This keeps resolution strictly ahead of the compiler's declaration
  prescan and makes execution order obvious. An import after other
  statements is a parse error.
- Module names are identifiers. What a name resolves *to* is the resolver's
  business (see Resolution below) — the language does not know about files.
- `as` is contextual, not a keyword: `import ui as u`.
- The selective form takes a comma-separated name list: `import ui: a, b`.
- A module alias is a **compile-time binding, not a runtime value**.
  `ui.button(rect)` is resolved statically in `ui`'s exports; using `ui`
  where a value is expected is an error ("`ui` is a module, not a value").
  A local binding of the same name (`let ui = ...`) shadows the alias, after
  which `ui.x` is ordinary field access.

## Exports

Every top-level `fn`, `enum` (its variants), and `let`/`state` in a module is
exported. Names starting with `_` are module-private: not resolvable via
`m._helper` and not importable. Imports are **not re-exported** — importing
`helper` into `mid.ptl` does not make it an export of `mid`.

A `pub` keyword can tighten visibility later without breaking v1 programs.

## Resolution

Resolution order for `import name`:

1. **Embedder-registered in-memory modules** — `env.register_module("ui",
   source)`. This is how a host ships a Petal-source prelude
   (`include_str!`) and how wasm hosts with no filesystem work. Always wins.
2. **The importing file's directory** — `<dir>/<name>.ptl`. Two sibling
   scripts can share a `palette.ptl` next to them.
3. **Registered search paths** — `env.add_module_path(dir)`; the CLI's
   `-I <dir>` flag lands here.
4. **`PETAL_PATH`** — colon-separated directories from the environment.

First hit wins. A module name resolves **at most once per program load**, so
diamond imports (`a` and `b` both import `base`) share one copy. Import
cycles are a compile error that lists the cycle path.

Custom stores can implement the `ModuleResolver` trait
(`rust/src/module.rs`); `ModuleRegistry` is the built-in implementation
behind the `Env` methods.

### Implicit imports

An embedder can declare modules that every loaded program imports
selectively-by-default:

```rust
env.register_module("ui", include_str!("ui.ptl"));
env.set_implicit_imports(&["ui"]);
```

User scripts then call `button(...)` with zero ceremony. Implicit bindings
are *weak*, like builtins: a script's own declarations shadow them silently,
and an explicit `import ui` on top is a no-op. This replaces source
concatenation entirely — same ergonomics, but with per-file error
attribution and no namespace pollution beyond the exports.

## Semantics

### Merge-at-compile-time

`Env::load_program` parses the entry file, walks its imports depth-first
(each module lexed/parsed independently, so line/column stay file-local),
then runs one compiler pass over the modules in **dependency post-order,
then the entry file**, into a single `Program`:

- Each module compiles inside its own scope frame; its top-level bindings
  become its exports. Exports are also bound in the global scope under
  qualified names (`"ui::button"`), so importer references ride the ordinary
  scope-lookup and closure-capture machinery — no relocation, no new backend
  work.
- **A module's top-level statements execute exactly once**, in dependency
  order, before its importers' — they compile into the shared root block
  ahead of the entry file's statements. `let palette = {...}` at module top
  level works for exactly this reason. Keep top-level side effects minimal
  by convention; the language doesn't police it in v1.
- Natives/builtins are global and visible in every module, shadowable per
  file as always.
- Overload sets (`fn f(a)` + `fn f(a, b)`) are grouped per module and export
  as one callable. Overload sets do **not** merge across an import boundary;
  importing `f` from two modules is a collision.

### Collisions are loud, shadowing stays quiet

Selective imports are explicit requests and get loud conflicts, at compile
time:

- `import a: draw` + `import b: draw` — error with both provenances.
- `import a: draw` + a local top-level declaration of `draw` — error.
- Two imports aliasing the same name to different modules — error.
- Importing an unknown or `_`-private name — error (unknown-name errors list
  the module's exports).

Ordinary lexical shadowing (`let x = ...` rebinding an implicit import or an
alias name) stays silent, as everywhere else in Petal.

### `state` keys are module-qualified

A module's `state scroll = 0` gets the persistent-state key
`hash("ui::scroll")`; the **entry file keeps bare-name hashing**, so every
pre-module program's hot-reload state survives unchanged. Consequences:

- Two modules declaring `state scroll` no longer share a slot.
- Moving a `state` declaration between files, or renaming a module, changes
  its key and **drops that state on reload** — the same class of event as
  renaming the variable. (`Env::diff_state` remains available to hosts that
  want a migration affordance.)
- `transfer_state` needed no changes; it already operates on the key space.

### Host-visible names

Root-frame function harvesting qualifies module functions:
`env.call_function(stack, "ui::button", args)` invokes a module's function
directly; entry-file functions keep their bare names. State JSON and
`--term` lookups likewise see module state under `ui::scroll`-style names.

## Diagnostics

Spans carry a `FileId` (entry file = 0), and the program's `SourceMap` holds
a file table (`name`, `source`, filesystem `origin` when there is one).
Entry-file errors keep today's format:

```
Cannot add int and nil [line 4, column 3]
```

Errors in an imported module name the file:

```
Cannot add int and nil [bad.ptl line 2, column 3]
```

Caret snippets, provenance ("Caused by:"), and stack traces all render
against the correct file's source. Parse errors in a module are prefixed
with the module's display name. Multi-file programs serialize the file table
into their IR (schema v0.1, see [ir-as-target.md](dev/ir-as-target.md)); the IR
of single-file programs is unchanged.

`petal::rewrite` (source splicing) is unaffected: it operates on individual
source files, which is exactly the granularity modules preserve.

## Hot reload

`Env::module_manifest(program_id)` returns one entry per source file a
program was compiled from — display name, filesystem `origin` (None for
in-memory modules), and a content hash. Hosts use it to answer "does editing
file X invalidate program P?" and to watch every file a program depends on:
petal-sdl's watcher watches the manifest's directories, so editing an
imported `palette.ptl` hot-reloads the script that imports it.

Reloading is unchanged mechanically: recompile (which re-runs the module
walk) with `Env::compile_program_at(pid, source, path)` and
`transfer_state`. State whose (module-qualified) key survives the recompile
is preserved.

## Embedder API summary

```rust
env.register_module("ui", source);          // in-memory module
env.add_module_path(dir);                   // filesystem search path
env.set_implicit_imports(&["ui"]);          // zero-ceremony prelude
env.load_program_at(&source, &path)?;       // entry with importer-relative resolution
env.compile_program_at(pid, &source, &path)?; // hot-reload recompile
env.module_manifest(pid);                   // files → origins/hashes
```

CLI: `petal run <file> -I <dir>` (repeatable; accepted by every compiling
subcommand), plus `PETAL_PATH`. The wasm bindings expose `register_module`
and `set_implicit_imports`.

## What v1 explicitly defers

- **Separate compilation / IR linking.** Distributing a module as IR
  requires relocating program-local ids or real cross-program addressing
  (`GlobalTermId` is the dormant seed). Module scopes and qualified state
  keys are the shape a linker would need, but v1 always compiles from
  source.
- **`pub` / visibility beyond the `_` prefix.**
- **Packages, versions, registries.**
- **Overload-set merging across modules.**
- **Conditional / non-top-of-file imports.**

## Tests

- `rust/tests/modules.rs` — binding forms, execution order, collision/cycle
  errors, state-key stability, hot reload, implicit imports, resolver
  ordering, in-memory (wasm-shaped) embedding.
- `ts/test/modules.test.ts` + `ts/test/fixtures/modules/` — multi-file
  golden cases through the CLI, IR roundtrip with a file table, `-I`, and
  file-attributed errors.
