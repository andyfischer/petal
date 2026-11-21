# core-stdlib

Petal's standard packages. Every directory here is a *package*: a
`petal.toml` and a directory of `.ptl` modules (`src/`) that a script reaches
with an ordinary `import`. Most are written **entirely in Petal**: no Rust, no
build step, no assets, so you can copy one into a project wholesale. A
**native package** also carries, in `native/`, the Rust crate that registers
the natives its modules declare with `native fn`; a host links that crate.

| Package | What it is |
|---------|------------|
| [`ui/`](ui/) | **Native.** The petal-ui interface: input, drawing, hit testing, text and the widget prelude. `src/` declares and wraps the natives; `native/` is the Rust crate `petal-ui` that hosts link to register them (`petal_ui::register_all`) |
| [`bloom/`](bloom/) | Petal's widget library: buttons, menus, controls, text and inspector fields, tabs, splitters, tables, overlays, an animation core, and a vector icon set. Built on `ui` |
| [`text-layout/`](text-layout/) | Placing text: cap-height centring, alignment on both axes, wrapping, elision, caret hit-testing. What bloom draws every one of its labels through |
| [`query/`](query/) | **Native interface.** Async data for a script that re-runs every frame: `query(kind, arg)` and the readers of an answer still on its way (`is_ready`, `is_loading`, `is_error`, `error_of`). Bundled (so `petal lint` resolves it); its natives are the crate `petal-stdlib-query` (`native/`), which petal-ui registers, and the data is the host's |
| [`regex/`](regex/) | **Native, optional.** Regular expressions: `regex_find`, `regex_all`, `regex_test`, `regex_replace`, `regex_split`, `regex_escape`. Not bundled and not in any `Env` by default; a host opts in by linking the crate `petal-regex` (`native/`) and calling `petal_regex::register` |
| [`glsl/`](glsl/) | **Experimental.** Translates a Petal function to GLSL (`to_glsl(lambda)`), for hosts that take shader hooks as Petal code. Needs no host layer; built on the core `fn_info` / `fn_ast` builtins |
| [`ptl_dataflow/`](ptl_dataflow/) | **Experimental, not bundled.** A Petal program drawn as its dataflow diagram: folds the compiler's user-only IR to one node per binding, lays it out in layers with nested function and loop boxes, and draws an interactive viewer linked to the source. Built on `ui`; the host supplies the IR (`dataflow_ir`) |

`bloom`, `text_layout` and `glsl` are downstream of `ui`'s natives (or of
`core` alone): ordinary Petal source. The layout of a native package, and the
plan for a standard way to register one, is in
[docs/proposals/native-packages.md](../docs/proposals/native-packages.md).

## Using one

`ui`'s declarations are always embedded, so `petal lint` resolves
`import ui` with no host; running it needs a host that links `petal-ui`.
The other three are **bundled in the runtime**: the `petal` binary and every host
that embeds Petal carry their source, so a script imports them with nothing
installed, registered or passed on the command line.

**bloom is the widget set.** It is opinionated (one look, one component
set), and it is bundled on purpose: it is what Garden's start screen and the
example apps draw with, and a script that wants a button should not have to
install one. `ui` stops at primitives and interaction logic; its own drawn
widgets are deprecated in bloom's favour (docs/tasks/ui-refactor.md, R4). A project that wants a different look
brings its own library, or its own copy of bloom (see *Using a different
copy* below); the bundled one is only the fallback. Its API is versioned
with the runtime, so a breaking change to bloom is a breaking change to
Petal and goes in the changelog.

**glsl is bundled although it is experimental.** That breaks the bundling
bar in [docs/skills/project-organization.md](../docs/skills/project-organization.md#the-bundling-bar)
and is listed there as an open misplacement: Cheesecake's engine imports
`glsl` from the bundled copy, so unbundling it waits on that host
registering its own copy.

```petal ignore
import bloom              # the facade module, named like its package
import bloom/menu         # one implementation module
import text_layout        # another package, by its manifest name
import ui                 # the host interface bloom and text_layout draw through
```

```bash
petal lint app.ptl                     # prints: requires host interfaces: ui >=0.1 (bloom), …
petal-ui-run app.ptl --frames 60       # runs it; petal-ui-run provides `ui`
petal packages                         # lists the bundled packages
```

`bloom` and `text_layout` state `[requires] ui = ">=0.1"` in their manifests,
so a program that imports them loads only in a host that provides the `ui`
interface (a petal-ui runner such as petal-sdl, Garden, `petal-ui-run`).
Plain `petal run` refuses it at load and says so. `glsl` requires no host.

Every library here carries a `petal.toml`, so it is a *package*: its manifest
name is what its modules answer to, wherever the directory sits.

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

A package name is the first segment of an import path, so it has to be
spellable as an identifier. Where a directory name is not (`text-layout`), the
manifest names the package (`text_layout`) and that is what `import` answers
to; the directory can be called whatever reads best.

**Using a different copy.** The bundled copy is the last place a package name
is looked up, so a project can swap in its own: copy the directory and name
it in the project's `petal.toml` (`bloom = { path = "third_party/bloom" }`),
or drop it at `vendor/bloom`. A host can register its own copy too
(`env.add_package(dir)` or `env.register_package("bloom", MODULES)`), which
wins over everything. bloom imports `text_layout`, which resolves through
bloom's own manifest when the two directories sit side by side and falls
back to the project's choice or the bundled copy otherwise. See
[docs/packages.md](../docs/packages.md).

## Writing one

- **A manifest, a directory of modules, one facade.** Write a `petal.toml`
  naming the library; its modules are then `bloom/menu`, `bloom/motion`, and
  cannot collide with the app's own `menu.ptl`. Add a facade module named like
  the package (`src/bloom.ptl`) that re-exports the surface with
  `pub import ./menu`, so users import one name. Inside the library, reach a
  sibling module with a relative import (`import ./motion`) or by the
  package's own name (`import bloom/motion`); the two are the same module.
- **State what you need in the manifest.** List the packages your modules
  import under `[dependencies]` and the host interfaces they need under
  `[requires]`.
- **Import the host's package in every module that uses it.** An import
  reaches only the file it is written in: a library module that calls
  `draw_rect(r, c)` says `import ui` at its top, like bloom's modules do, and
  one that calls `noise` says `import noise`. The one catch: if your library
  exports a name `ui` also has *and* one arity of it calls another, route both
  through a private implementation — inside a variant's own body the name
  cannot see its siblings and escapes outward to `ui`'s set. See
  [Sharing Petal libraries](../docs/dev/sharing-petal-libraries.md) for this and
  the rest of the sharp edges.
- **Keep state per callsite.** `state` inside a library function is keyed by the
  call path, so a component can hold its own animation without the caller
  passing anything. That is the single biggest thing Petal gives a library
  author, and it is what bloom is built on.
