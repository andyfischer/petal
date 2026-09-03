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

There are two ways to make a library reachable, and which one you want depends
on where your script's source comes from.

**On the module path** — for scripts that live on disk:

```bash
petal run -I petal-libs/bloom/src app.ptl
petal-ui-run app.ptl -I petal-libs/bloom/src --frames 60
```

or `PETAL_PATH=petal-libs/bloom/src`, or by copying `src/*.ptl` next to the
script (the importing file's own directory is searched first).

**Registered in memory** — for a host, and the only option for a script whose
source did not come from a file it sits beside (a pushed panel drawer, a
browser host with no filesystem):

```rust
for (name, source) in MODULES {          // include_str! each .ptl
    env.register_module(name, source);
}
```

Garden does exactly this in
[`garden/garden-script/src/bloom.rs`](../garden/garden-script/src/bloom.rs), so
`import bloom` works in every Garden panel with no setup.

## Writing one

- **One directory of modules, one facade.** Name modules with the library's
  own prefix (`bloom_menu`, `bloom_motion`) — module names are flat and global
  to a program, so a bare `menu.ptl` would collide with the app's. Add a facade
  module (`bloom.ptl`) that re-exports the surface with
  `export let button = bloom_button.button`, so users import one name.
- **Take from the host through `import`, not through the prelude.** A host's
  implicit imports reach the *entry file only*, so a library module must
  `import ui: draw_rect, …` explicitly. See
  [Sharing Petal libraries](../docs/dev/sharing-petal-libraries.md) for this and
  the rest of the sharp edges.
- **Keep state per callsite.** `state` inside a library function is keyed by the
  call path, so a component can hold its own animation without the caller
  passing anything. That is the single biggest thing Petal gives a library
  author, and it is what bloom is built on.
