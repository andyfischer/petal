# core-runtime

Shared libraries written **entirely in Petal**. No Rust, no build step, no
assets: a library here is a directory of `.ptl` modules that any host embedding
Petal can use, and that you can copy into a project wholesale.

| Library | What it is |
|---------|------------|
| [`bloom/`](bloom/) | A UI component library: buttons, menus, controls, overlays, an animation core, and a vector icon set. Built on the `petal-ui` host layer |
| [`text-layout/`](text-layout/) | Placing text: cap-height centring, alignment on both axes, wrapping, elision, caret hit-testing. What bloom draws every one of its labels through |
| [`glsl/`](glsl/) | **Experimental.** Translates a Petal function to GLSL (`to_glsl(lambda)`), for hosts that take shader hooks as Petal code. Needs no host layer; built on the core `fn_info` / `fn_ast` builtins |

This is deliberately separate from [`core-libs/petal-ui/`](../core-libs/petal-ui/), which is a Rust
crate that gives a host its input and draw natives (plus the `ui` package that
declares and wraps them). Everything in `core-runtime` is downstream of that: ordinary Petal
source, importable by an ordinary `import`.

## Using one

All three are **bundled in the runtime**: the `petal` binary and every host
that embeds Petal carry their source, so a script imports them with nothing
installed, registered or passed on the command line.

```petal ignore
import bloom              // the facade module, named like its package
import bloom/menu         // one implementation module
import text_layout        // another package, by its manifest name
import ui                 // the host interface bloom and text_layout draw through
```

```bash
petal check app.ptl                    # prints: requires host interfaces: ui >=0.1 (bloom), …
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
  one that calls `lerp` says `import creative`. The one catch: if your library
  exports a name `ui` also has *and* one arity of it calls another, route both
  through a private implementation — inside a variant's own body the name
  cannot see its siblings and escapes outward to `ui`'s set. See
  [Sharing Petal libraries](../docs/dev/sharing-petal-libraries.md) for this and
  the rest of the sharp edges.
- **Keep state per callsite.** `state` inside a library function is keyed by the
  call path, so a component can hold its own animation without the caller
  passing anything. That is the single biggest thing Petal gives a library
  author, and it is what bloom is built on.
