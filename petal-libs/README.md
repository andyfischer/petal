# petal-libs

Shared libraries written **entirely in Petal**. No Rust, no build step, no
assets: a library here is a directory of `.ptl` modules that any host embedding
Petal can use, and that you can copy into a project wholesale.

| Library | What it is |
|---------|------------|
| [`bloom/`](bloom/) | A UI component library: buttons, menus, controls, overlays, an animation core, and a vector icon set. Built on the `petal-ui` host layer |

This is deliberately separate from [`petal-ui/`](../petal-ui/), which is a Rust
crate that gives a host its input and draw natives (plus the `ui` prelude that
wraps them). Everything in `petal-libs` is downstream of that: ordinary Petal
source, importable by an ordinary `import`.

## Using one

Every library here carries a `petal.toml`, so it is a *package*: its manifest
name is what its modules answer to, wherever the directory sits.

```toml
[package]
name = "bloom"
version = "0.1.0"
modules = "src"
```

There are two ways to make a package reachable, and which one you want depends
on where your script's source comes from.

**On the module path** — for scripts that live on disk. Point `-I` at this
directory (or at a library root); a manifest one level down is found either
way:

```bash
petal run -I petal-libs app.ptl
petal-ui-run app.ptl -I petal-libs --frames 60
petal packages -I petal-libs          # what that made importable
```

or `PETAL_PATH=petal-libs`. Then:

```petal ignore
import bloom              // the facade module, named like its package
import bloom/menu         // one implementation module
```

**Registered in memory** — for a host, and the only option for a script whose
source did not come from a file it sits beside (a pushed panel drawer, a
browser host with no filesystem). One call registers the whole library:

```rust
env.register_package("bloom", MODULES)?;   // include_str! each .ptl
// or, from disk:  env.add_package("petal-libs/bloom")?;
```

Garden does exactly this in
[`garden/garden-script/src/bloom.rs`](../garden/garden-script/src/bloom.rs), so
`import bloom` works in every Garden panel with no setup.

## Writing one

- **A manifest, a directory of modules, one facade.** Write a `petal.toml`
  naming the library; its modules are then `bloom/menu`, `bloom/motion`, and
  cannot collide with the app's own `menu.ptl`. Add a facade module named like
  the package (`src/bloom.ptl`) that re-exports the surface with
  `export import bloom/menu: *`, so users import one name.
- **Take the host's prelude bare.** A host's implicit imports (`ui`) now reach
  every module of a program, so a library module writes `draw_rect(r, c)` with
  no `import ui` at all. The one catch: if your library exports a name the
  prelude also has *and* one arity of it calls another, route both through a
  private implementation — inside a variant's own body the name cannot see its
  siblings and escapes outward to the prelude's set. See
  [Sharing Petal libraries](../docs/dev/sharing-petal-libraries.md) for this and
  the rest of the sharp edges.
- **Keep state per callsite.** `state` inside a library function is keyed by the
  call path, so a component can hold its own animation without the caller
  passing anything. That is the single biggest thing Petal gives a library
  author, and it is what bloom is built on.
