# Packages

A **package** is a directory with a `petal.toml` manifest: a named set of
modules that other code imports by that name. A **project** is the package
your program's entry file lives in. This page covers the manifest,
dependencies, the host interfaces a package requires, the lockfile, the
cache, vendoring, and every package command.

[Module system](module-system.md) covers the other half: the `import` forms,
which name wins, and how a module is identified.

```bash
petal init                                               # write a petal.toml here
petal add tween --git https://example.org/tween --tag v1.2.0
petal add util --path ../libs/util
petal run main.ptl                                       # no flags: the manifest is found from the file
```

```petal ignore
// main.ptl
import tween          // a [dependencies] key
import util
import ./level        // a file beside this one
```

Every command that compiles (`run`, `check`, `explain`, the `show-*`
commands, …) and the language server find the manifest by walking up from
the file they are given. Inside a project nothing needs a flag.

## The manifest

```toml
[package]
name = "marble"
version = "0.1.0"
modules = "src"      # optional: where the modules are.
                     # Defaults to src/ when it exists, else the manifest's
                     # own directory.

[dependencies]
text_layout = { path = "../third_party/text-layout" }
tw          = { path = "../libs/tween" }                  # the package calls itself `tween`
shapes      = { git = "https://example.org/shapes", tag = "v1.2.0" }
pinned      = { git = "https://example.org/pinned", rev = "9f3c2ab" }

[requires]
ui = ">=0.1"
```

### `[package]`

| Key | Meaning |
| --- | --- |
| `name` | The package name: the first segment of every import of it. Required. |
| `version` | Dotted numbers (`0.1.0`). It is what `petal packages` and `petal.lock` show, and what a host announces when the package is its interface. |
| `modules` | The directory holding the modules, relative to the manifest. |

Every `.ptl` file under the module directory is importable as
`<name>/<path>`, nested directories included: `src/widgets/button.ptl` of
`bloom` is `import bloom/widgets/button`. The module named after the package
(`src/bloom.ptl`) is a bare `import bloom`, which is where a
[facade](module-system.md#re-exporting) goes.

The **manifest name is the package name**, not the directory name: a library
keeps its name wherever it is unpacked.

A directory below a package that has its own `petal.toml` is a different
package, and its files are not modules of the one around it. `target/` and
`node_modules/` are never scanned for modules. A file under the package root
but outside the module directory (an `examples/` or `tests/` directory beside
`src/`) is a loose file that can still import the package by name.

Inside a package, modules reach each other with a relative import
(`import ./motion`) or by the package's own name (`import bloom/motion`). The
two are the same module. A relative import may not leave the package root.

### `[dependencies]`

**The key is the import name.** `tw = { … }` means this package's files write
`import tw` and `import tw/ease`, whatever the package calls itself. The
qualifier follows the key (`tw.go()`); the module's
[identity](module-system.md#module-identity) is the package's own name
(`tween`, `tween/ease`), so a package reached under two keys still loads
once.

| Form | Meaning |
| --- | --- |
| `{ path = "…" }` | A package directory, relative to this manifest. |
| `{ git = "…", tag = "…" }` | A git repository with a `petal.toml` at its root, pinned to a tag. |
| `{ git = "…", rev = "…" }` | The same, pinned to a commit. |
| `"0.3"` | A registry version. Parsed, but there is no registry: installing or importing one is the error `registries are not supported yet`. |

A `[dependencies.name]` section is accepted in place of an inline table. A
malformed entry (an unknown key, `git` with neither `tag` nor `rev`, both
`path` and `git`) is an error naming the line.

A manifest that does not parse stops every program under it: `petal run` and
`petal check` report the manifest's error and exit 1, whether or not the
program imports anything. It is never taken for a missing manifest, which
would drop the package name and `[requires]`.

A dependency is declared where it is used. A package resolves its imports
from its *own* manifest, so `tween`'s `import base` is answered by
`tween/petal.toml`. The project's manifest is the fallback, which is how code
with no manifest of its own (`-e` code run inside a project, the modules of a
bundled or in-memory package) picks up the project's choice of a package.

A git dependency's package is the repository's root. It may have `path`
dependencies of its own as long as they stay inside the repository.

### `[requires]`

`[requires]` names the **host interfaces** a package needs: the sets of
native functions a runner provides, such as `ui` for petal-ui. They are
provided by the host, never fetched as code.

| Requirement | Accepts |
| --- | --- |
| `">=0.4"` or a bare `"0.4"` | that version or any later one |
| `"^1.2"` | 1.2 or later within major version 1 |
| `"=1.2.0"` | exactly that version |
| `"*"` | any version |

Versions are dotted numbers compared numerically; `0.4` and `0.4.0` are the
same.

A program requires what the packages of its loaded files require: the
project's own `[requires]`, plus those of every package one of whose modules
the program imports, however deep. `bloom` requires `ui`, so an app that
imports `bloom` requires it too. A dependency that is listed but never
imported adds nothing.

A host says what it provides (`Env::provides(name, version)`; see
[Host interface packages](module-system.md#host-interface-packages)). When a
program is loaded to *run*, each requirement is checked, and one the host
does not meet is a load error before any line runs:

```
Error: package 'bloom' requires the host interface `ui` >=0.1, which this host does not
provide — the program cannot run here; run it with a host that provides it
```

`petal check` has no host, so it does not fail on requirements. It reports
them: one line on stdout, and a `requires` array with `--json`.

```
$ petal check app.ptl
requires host interfaces: ui >=0.1 (bloom), ui >=0.1 (text_layout)
```

Every runtime provides `core` at the version of the petal crate, so a
package can ask for a new enough runtime with `core = ">=0.1"`. `petal run`
provides nothing else; petal-sdl provides `ui` and `sdl`, and Garden provides
`ui`, `garden` and `garden_config`.

## Where a package name is looked up

A bare first segment in an import is always a package name. It is looked up
in this order, and the first place that has it wins:

1. what the host registered in memory;
2. the manifest: the importing file's own package (its own name, then its
   `[dependencies]`), then the project's manifest, then `vendor/<name>` at
   the project root;
3. the search path: `-I <dir>` and `PETAL_PATH`;
4. the packages bundled in the binary.

[Where packages are found](module-system.md#where-packages-are-found) has the
details of each step.

### Bundled packages

The `petal` binary, and every embedding of the runtime, carries these:

| Package | What it is |
| --- | --- |
| `core` | The declarations of the global builtins. Always in scope; `import core` only adds the qualifier. |
| `noise`, `color`, `vec2`, `autodiff`, `json`, `io`, `creative` | The domain builtins. Every runtime implements them; a script imports the package to use its names. See [Builtins](Builtins.md#which-builtins-need-an-import). |
| `ui` | The petal-ui interface and its widget library. Needs a petal-ui host to run. |
| `bloom` | The component library ([core-runtime/bloom](../core-runtime/bloom)). Requires `ui`. |
| `text_layout` | Text placement ([core-runtime/text-layout](../core-runtime/text-layout)). Requires `ui`. |
| `glsl` | Petal to GLSL, experimental ([core-runtime/glsl](../core-runtime/glsl)). |

A package of the same name found at an earlier step wins over the bundled
copy: a host registration, a `[dependencies]` entry, a `vendor/<name>`
directory, or a package on `-I`.

### The search path

`-I <dir>` (repeatable, on every command that compiles) and the `PETAL_PATH`
environment variable name directories to search for packages. A search
directory offers the package it *is*, when it holds a `petal.toml`, and each
package directly under it. It finds packages only: `-I lib` does not make
`lib/util.ptl` importable as `util`.

Prefer a `[dependencies]` entry. The search path is for ad-hoc use, such as
checking a loose script against a host's interface package.

## The package manager

All of these work on the project of the working directory: the nearest
`petal.toml` at or above it.

| Command | What it does |
| --- | --- |
| [`petal init`](#petal-init) | Write a `petal.toml`. |
| [`petal add`](#petal-add) | Add a dependency and install it. |
| [`petal remove`](#petal-remove) | Remove a dependency. |
| [`petal install`](#petal-install) | Resolve the graph, fetch what is missing, verify, write `petal.lock`. |
| [`petal update`](#petal-update) | Resolve packages afresh instead of from `petal.lock`. |
| [`petal vendor`](#petal-vendor) | Copy the git packages into `vendor/`. |
| [`petal bundle`](#petal-bundle) | Emit the whole graph as one JSON document for a host with no filesystem. |
| [`petal packages`](#petal-packages) | Show the resolved graph, the required interfaces and what else is available. |

### `petal init`

```
petal init [<dir>] [--name <name>]
```

Writes a `petal.toml` with a `[package]` section into `<dir>` (the working
directory when absent), creating the directory if needed:

```toml
[package]
name = "game"
version = "0.1.0"
```

The name is `--name`, or the directory's name with every character that is
not a letter or digit turned into `_`. An existing `petal.toml` is never
overwritten.

Giving a directory of scripts a `petal.toml` makes its files modules of the
package, which renames them: a sibling module that was `level` becomes
`game/level` in diagnostics, `state` keys and qualified function names. See
[Module identity](module-system.md#module-identity).

### `petal add`

```
petal add <name> --path <dir>
petal add <name> --git <url> --tag <tag>
petal add <name> --git <url> --rev <rev>
```

Sets the dependency `<name>` in `[dependencies]`, replacing an entry of the
same name, then installs. `<name>` is the import name. If the install fails,
`petal.toml` is put back as it was.

```
$ petal add ease --git https://example.org/ease --tag v1.0.0
    Fetching https://example.org/ease
Added ease to petal.toml
Locked 1 package in petal.lock
    ease 1.0.0  git https://example.org/ease tag v1.0.0 @ d9b1d92b16
```

### `petal remove`

```
petal remove <name>...
```

Deletes each named entry from `[dependencies]` and installs again, which
drops from `petal.lock` every package nothing depends on any more. A
`vendor/<name>` directory is left for you to delete.

### `petal install`

```
petal install [--locked]
```

Resolves the dependency graph into exactly one version of each package,
fetches what is missing, verifies every tree against the lockfile, and
writes `petal.lock` beside `petal.toml`. `--locked` fails instead of
changing `petal.lock`, for CI.

You rarely need to run it by hand: `petal add` installs, and the commands
that compile [install on demand](#install-on-demand).

### `petal update`

```
petal update [<name>...]
```

Like `install`, but the named packages (every package when none is named)
are resolved afresh instead of from `petal.lock`: a tag is looked up again in
the repository and the lockfile takes the commit and tree hash found now.
Use it when a tag was moved upstream, or when a locked tree no longer
verifies and the change is expected. A name the lockfile does not pin is an
error.

Changing a `tag` in `petal.toml` does not need `update`; the next install
follows the manifest.

### `petal vendor`

```
petal vendor
```

Installs, then copies each git package of the locked graph into
`vendor/<name>` at the project root. Path dependencies are already on disk
and are not copied. See [The cache and `vendor/`](#the-cache-and-vendor).

### `petal bundle`

```
petal bundle <entry> [-o <file>]
```

Prints every module of the installed graph, the project's own package
included, as one JSON document. See [Bundles](#bundles).

### `petal packages`

```
petal packages [--json] [-I <dir>]... [path]
```

Prints the project (the nearest `petal.toml` above `path`, or above the
working directory), its dependency graph as it resolves, the host interfaces
the graph requires, the packages on the search path, and the packages
bundled in the binary. It never fetches and never installs.

```
$ petal packages
project game 0.1.0  /work/game
    ease 1.1.0  (git https://example.org/ease tag v1.1.0 @ 529348c147 (vendored))
    tw = util 0.2.0  (path ../libs/util)
requires host interfaces:
    audio ^1.0  (package game)
    ui >=0.1  (package game)

No packages found on the search path. Point -I at a directory holding a
petal.toml, or at a directory of such libraries.

bundled in this binary:
    core 0.1.0  (builtins)
    noise 0.1.0  (builtins)
    ...
    ui 0.1.0
    bloom 0.1.0  (requires ui >=0.1)
    text_layout 0.1.0  (requires ui >=0.1)
    glsl 0.1.0
```

A dependency whose key renames its package is shown as `key = name`. A git
dependency that is not installed, or a registry version, is listed as
`UNRESOLVED` with the reason, after a warning that the graph does not
resolve. A `petal.toml` that will not parse is an error naming the file and
the line.

`--json` emits an object with `project` (`null` outside a project; otherwise
`name`, `version`, `root`, `module_dir`, `modules`, `locked`, `requires`,
`required_interfaces` and `dependencies`, each git dependency with `commit`,
`tree` and `vendored`), `packages` (the search path), `bundled` and
`errors`.

## One version of each package

A program holds one copy of each package, so every importer of `ease` sees
the same module and the same state. When two packages depend on the same
repository at different tags, the **higher tag wins**. This is *minimal
version selection*: the version used is the highest minimum anyone in the
graph asks for. No solver runs, and nothing is ever upgraded past what some
manifest names. Tags are compared as dotted versions (`v1.2.0`, `1.10`).

What cannot be ordered is an error:

- two different `rev`s of one repository, or a tag and a `rev` that are
  different commits (the same commit under two spellings is fine);
- two tags that do not read as versions and are different commits;
- two different sources (two repositories, or a repository and a directory)
  whose packages have the same name.

```
Error: /src/ease is wanted at rev '9f3c2ab' (by 'game') and at tag 'v1.1.0' (by 'tween'),
and those are different commits that cannot be ordered — a program holds exactly one
version of each package; pin both to the same tag or rev
```

## `petal.lock`

`petal install` writes `petal.lock` beside the project's `petal.toml`. Commit
it. Each package of the graph has an entry:

```toml
# Written by `petal install`. Commit this file; do not edit it by hand.
version = "1"

[[package]]
name = "ease"
version = "1.1.0"
git = "https://example.org/ease"
tag = "v1.1.0"
commit = "529348c147a5b2c77021a99c3dd1d9bd92899e24"
tree = "sha256:f8701d089ee26369c95f0cbfe92925469d145634417c45b9acd1d88e9de374b1"

[[package]]
name = "util"
version = "0.2.0"
path = "../libs/util"
```

A git package is pinned to the commit its tag or rev resolved to, and to a
SHA-256 hash of its checked-out tree (every file's path and content; `.git`
is left out). Later installs use the locked commit even if the tag has since
moved, and verify what they fetch, and every cached or vendored tree already
present, against the locked hash. A mismatch is an error; `petal update
<name>` re-pins when the change is expected.

The hash is checked by `petal install`, `petal update` and `petal vendor`,
not on every load: `petal run`, `petal check` and a host read a cached or
vendored tree as it is on disk. Run `petal install --locked` (in CI, say) to
confirm that nothing under the cache or `vendor/` has drifted from the lock.

A `path` package is recorded by its path, relative to the project root, and
is not hashed: it is live source on your disk. A path package inside a
fetched repository is recorded with `within = "<the git package>"`.

A locked higher tag stands only while something in the graph still asks for
it. Lower the pin in every manifest and the next install follows.

## The cache and `vendor/`

Git repositories are fetched with `git` (it must be on the `PATH`) into
`~/.petal/cache`. `PETAL_HOME` replaces `~/.petal`.

| Path under the cache | Holds |
| --- | --- |
| `git/<hash of the url>` | a bare clone of each repository |
| `trees/<sha256>` | a checked-out tree, named by its content hash |
| `commits/<commit>` | which tree a commit checked out to |

The cache is content-addressed: two projects locking the same tree share
it, and a tree is never modified once written. A damaged cache entry is
fetched again by the next install.

`petal vendor` copies each git package into `vendor/<name>` at the project
root. **`vendor/<name>` is used in preference to the cache** by every
command and every host, so a project with `vendor/` checked in builds with
no network and no cache. While `vendor/` exists, `petal install` and `petal
update` keep the packages in it in step with `petal.lock`. A vendored tree
edited by hand still loads, but fails `petal install`'s verification until
`petal vendor` rewrites it. Directories under `vendor/` that the lockfile
does not know are never deleted.

`vendor/<name>` holding a `petal.toml` also answers `import <name>` for a
name no manifest lists, ahead of the search path and the bundled copy. That
is the way to pin a patched `bloom` for one project.

## Install on demand

`petal run`, `petal check` and the other commands that compile install on
demand. The first time a program imports a git dependency, the project's
graph is compared with `petal.lock`, and if something is missing or the
manifest has moved past the lockfile, the install runs before the package is
loaded:

```
$ petal run main.ptl
Installing the dependencies of /work/game
    Fetching https://example.org/ease
9
```

A script that imports no git dependency never triggers it.
`PETAL_OFFLINE=1` forbids fetching; the missing install is then an error:

```
Error: cannot find module 'ease' (imported by main.ptl): dependency 'ease' of package
'game' is a git dependency (https://example.org/ease) that is not installed: there is no
petal.lock in /work/game — run `petal install`
```

`petal fix-imports`, `petal suggest`, `petal apply-change`, the language
server, and a host embedding the runtime never fetch. They read `petal.lock`
and open the vendored or cached tree, and report an uninstalled dependency
with the error above. A Rust host opts in with
`Env::set_install_on_demand(true)`.

## Bundles

`petal bundle <entry>` prints the installed graph in the in-memory form a
host's `register_package` accepts, for a host with no filesystem (a wasm
page, an embedded device):

```json
{
  "format": 1,
  "entry": { "file": "main.ptl", "path": "<package:game>/main.ptl", "source": "…" },
  "packages": [
    { "name": "game",  "version": "0.1.0", "modules": { "petal.toml": "…", "main": "…", "level": "…" } },
    { "name": "tween", "version": "1.2.0", "modules": { "petal.toml": "…", "tween": "…" } }
  ],
  "requires": [ { "interface": "ui", "version": ">=0.1", "package": "tween" } ]
}
```

A host registers each package and loads the entry at `entry.path`, which
places it inside the project's in-memory package so its `./x` imports
resolve:

```rust
for package in &bundle.packages {
    env.register_package(&package.name, package.modules.iter())?;
}
let program = env.load_program_at(&bundle.entry.source, Path::new(&bundle.entry.path))?;
```

- `modules` is exactly what `Env::register_package`, the wasm
  `register_package(name, json)` and the C bridge's `pb_vm_register_package`
  take.
- Each package's `petal.toml` rides along as the pseudo-module `petal.toml`,
  so its `[requires]` are checked against what the host provides.
- `entry.path` is `null` when the entry file is not under the project's
  module directory. Such an entry's relative imports are not in the bundle.
- Packages bundled in the binary (`ui`, `bloom`, the builtin packages) are
  not repeated, and neither is anything reached only through `-I`.
- A dependency whose key renames its package (`tw = …` for `tween`) cannot
  be bundled, because an in-memory package is registered under its own name.
  The error names the key and the package.

## Shipping a library

A library is a directory of modules with a `petal.toml`, usually with a
facade module named after the package that re-exports the rest
(`pub import ./button`; see [Re-exporting](module-system.md#re-exporting)).
State the host interfaces it needs under `[requires]` and the packages it
imports under `[dependencies]`.

Users reach it with a `[dependencies]` entry: a `path` for a copy on disk, or
`git` with a tag for a repository whose root holds the `petal.toml`. Tag
releases with dotted versions (`v1.2.0`) so that two dependents asking for
different tags can be ordered.

## Environment variables

| Variable | Effect |
| --- | --- |
| `PETAL_HOME` | Replaces `~/.petal`, the directory holding the cache. |
| `PETAL_OFFLINE` | `1` forbids fetching and on-demand installs. |
| `PETAL_PATH` | Directories to search for packages, like `-I`. |

## Migrating older code

Code written before this system does not load: sibling files imported by a
bare name, `import m: *`, and names that used to be in scope with no import
(a host's natives and widgets, and the builtins that now live in packages).
`petal fix-imports` rewrites the import lines:

```bash
petal fix-imports --check .     # list what would change
petal fix-imports .             # rewrite every .ptl file under the directory
```

```
$ petal fix-imports old.ptl
old.ptl
  line 1: `import level` -> `import ./level`
  line 2: `import ease: *` -> `import ease`
  added `import creative` (for `lerp`)
  added `import tw` (for `twice`)
fixed imports in 1 of 1 file(s)
```

It knows the bundled packages, the `[dependencies]` of the file's project,
and the packages under `-I` and `PETAL_PATH`. For a host whose interface
package is not a dependency yet, pass it with `-I`. Running it twice changes
nothing. [Migrating with `petal
fix-imports`](module-system.md#migrating-with-petal-fix-imports) lists every
rewrite and what is left for you to decide.

## Not there yet

- A registry: there is no index and no `publish`. A version string in
  `[dependencies]` is an error.
- A `subdir` for a git dependency: the package is the repository root.
- Overriding a transitive dependency's source from the project manifest. Two
  sources for one package name are an error.
- Bundling a project that renames a dependency, and bundling loose files
  outside the module directory.
- `--json` output for the package-manager commands other than `packages`.
