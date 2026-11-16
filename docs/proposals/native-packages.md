# Proposal: native packages, one package type for Petal and Rust libraries

Status: proposed, 2026-11-15

## Summary

A Petal package is already one type at the language level: a `petal.toml`
plus `.ptl` modules, where `native fn` declarations make some of it native and
an unbound native is a load error. What is missing is everything around that:
the Rust half has no declared home, no standard entry point and no mention in
the manifest, so every host re-writes the same registration boilerplate and
core carries a hard-coded special case (`kind = "ui"`, `UI_RUNNER_HINT`) for
the one first-party native package.

This proposes:

1. **A `[native]` manifest section** naming the Rust crate that registers a
   package's natives. It is descriptive: it drives error messages, `petal
   packages`/`petal doc` output and build checks, and changes no load
   semantics.
2. **A standard Rust entry point**, `petal::NativePackage` and
   `Env::register_native_package(&PKG)`, which registers the natives, registers the package
   source with its manifest, provides the interface at the manifest's version
   and (in debug builds) verifies that every `native fn` is bound.
3. **A uniform directory layout** for `core-stdlib/<pkg>/`: `petal.toml`,
   Petal modules in `src/`, and an optional `native/` Rust crate.
4. **Moving `core-libs/petal-ui` to `core-stdlib/ui`** (crate in
   `core-stdlib/ui/native`), with the bundled kind `ui` generalised to
   `native`.

**Recommendation: adopt, but move only `petal-ui`.** The other five
`core-libs` crates (`gpp`, `petal-query`, `petal-pkgman`,
`petal-ui-headless`, `programs-as-documents`) expose nothing to Petal scripts:
no `petal.toml`, no `.ptl`, no package natives. They are Rust infrastructure,
and filing them under a standard library of importable packages would
mislabel them. See [Open questions](#open-questions).

## Motivation

### The pattern exists, unnamed, in 16 packages

Every package below is `native fn` declarations plus a Rust crate that
registers the natives:

| Package | Declarations | Natives registered by |
|---|---|---|
| `core`, `noise`, `color`, `vec2`, `json`, `io` | `core/packages/*` | the `petal` crate itself, in every `Env` |
| `ui` | `core-libs/petal-ui/prelude` | `petal-ui` (the only first-party host-linked one) |
| `sdl` | `integrations/petal-desktop-sdl/packages/sdl` | petal-sdl |
| `web` | `integrations/petal-web-html/packages/web` | petal-web-html |
| `garden`, `garden_config` | `garden/garden-script/packages/*` | garden-script |
| `fps` | `examples/custom-apps/petal-fps/packages/fps` | the petal-fps example |
| `wf_seam` | `~/worlds-fair/ui/ptl/packages/wf_seam` | wf-ui-core |
| `studio`, `design` | `~/petal-creative/…/packages/*` | petal-design / creative-core |

### Each host writes the same three steps by hand

From `integrations/petal-desktop-sdl/src/native_fns.rs:72`:

```rust
env.register_package(
    SDL_PACKAGE,
    [
        (SDL_PACKAGE, SDL_INTERFACE),
        (petal::package::MANIFEST_FILE, SDL_MANIFEST),
    ],
)
.expect("the sdl interface package registers");
env.provides_package(SDL_PACKAGE)
    .expect("the sdl interface has a version");
```

The same sequence (`register_prelude` + `provide_interface`, or
`register_package` + `provides_package`) appears at
`integrations/petal-web-canvas/rust/src/lib.rs:158`,
`garden/garden-script/src/panel.rs:2991`,
`examples/custom-apps/petal-fps/src/host.rs:64` (which forgets
`provides_package`) and `core-libs/petal-ui/src/lib.rs:206-246`.
petal-creative wrote its own helper for it
(`creative-core/src/script.rs:42`, `register_package`). The drift test
`verify_interface` is opt-in and only `petal-ui`'s own tests run it
(`core-libs/petal-ui/tests/host_natives.rs:32`).

Listing the files is manual too. `petal-ui` keeps a hand-written 13-module
array (`package_modules()`, `lib.rs:186`) whose `.ptl` text it reads back from
`petal::bundled`, the copy that `core/build.rs` embedded from `petal-ui`'s
directory.

### Core special-cases the one first-party native package

- `core/Cargo.toml`: bundled kind `ui` exists only for `petal-ui/prelude`.
  `core/build.rs:119` treats it as "always embedded, natives not ours".
- `core/src/native_decl.rs:635`: `UI_RUNNER_HINT` is a hard-coded suffix
  for the unbound-native load error when the package is `ui`:

  ```
  Error: module 'ui/_base' needs native fns this host does not register: draw_rect, …
  — the program cannot run here; run it with a host that provides them (`ui` is the
  petal-ui interface: use a petal-ui runner such as petal-sdl; `petal run` provides
  the core builtins only)
  ```

  The same failure for `sdl` gets no hint, because there is nowhere to state
  one:

  ```
  Error: module 'sdl' needs a native fn this host does not register: file_exists
  — the program cannot run here; run it with a host that provides it
  ```

  (Both outputs come from `petal run` on 2026-11-15.)

### The split makes the source tree read wrong

`ui` is a Petal package, yet its source lives at
`core-libs/petal-ui/prelude/`, inside a crate, in a directory whose other
members are protocol and tooling crates. `bloom`, which is built on it, sits in
`core-stdlib/`. `docs/skills/project-organization.md` has to explain that
split in two decision-tree steps (5 and 6).

## Design

### Directory layout

```
core-stdlib/
  bloom/            # pure Petal: unchanged
    petal.toml
    src/*.ptl
  ui/               # native: Petal half + Rust half
    petal.toml      # name = "ui", modules = "src", [native]
    src/*.ptl       # was core-libs/petal-ui/prelude/*.ptl
    native/         # the crate `petal-ui`: Cargo.toml, src/*.rs, tests/, examples/
    docs/  README.md
```

`src/` holds Petal modules in every stdlib package, and a native package adds
a self-contained crate under `native/`. Cargo dependents write
`petal-ui = { path = ".../core-stdlib/ui/native" }`. The crate name stays
`petal-ui`, so Rust `use` paths do not change.

### `[native]` in `petal.toml`

```toml
[package]
name = "ui"
version = "0.2.0"
modules = "src"

[native]
crate = "petal-ui"              # the Rust crate that registers these natives
register = "petal_ui::register" # what a host calls; quoted in errors and docs
```

Both keys are strings and `crate` is required. Any other key is an error that
names the line, as `[host]` does today.

Semantics:

- **Loading is unchanged.** A program that uses a package whose natives are
  unbound fails at load with today's error. When the package has
  `[native]`, the error ends with the manifest's hint instead of a hard-coded
  one:

  ```
  Error: module 'sdl' needs a native fn this host does not register: file_exists
  — the program cannot run here: package `sdl` is native; its natives come from
  the Rust crate `petal-sdl` (a host links it and calls `petal_sdl::register`)
  ```

  `UI_RUNNER_HINT` is deleted, and `ui`'s manifest carries the "use a
  petal-ui runner" wording in an optional `hint` key.
- **`[requires]` is unchanged** and stays the way a *dependent* states a
  version need (`bloom`: `ui = ">=0.1"`). `Env::register_native_package` provides the
  interface, so `[requires] ui` and `[native]` on `ui` agree by construction.
- **`petal packages` and `petal doc`** mark a `[native]` package as native and
  print its crate. `petal lint` adds nothing: it already reports
  `requires host interfaces`.
- A package with `native fn` declarations and **no** `[native]` stays valid,
  for a host that registers its own interface in memory. Requiring the
  section everywhere would break 9 external packages for no runtime gain.
  `core/build.rs` requires `[native]` only on the bundled `native` kind.

### Rust side: `NativePackage` and `Env::register_native_package`

```rust
// in petal (core/src/package.rs)
pub struct NativePackage {
    pub name: &'static str,
    /// Package-relative paths and sources, `petal.toml` included.
    pub files: &'static [(&'static str, &'static str)],
    pub register_natives: fn(&mut Env),
}

impl Env {
    /// Register the natives, then the package, then provide its interface
    /// at the manifest's version. Debug builds also check that every
    /// `native fn` the package declares is bound (`verify_interface`).
    pub fn register_native_package(&mut self, pkg: &NativePackage) -> Result<(), RegisterError>;
}
```

```rust
// in petal-ui
pub static PACKAGE: NativePackage = NativePackage {
    name: "ui",
    files: petal::embedded_package!("ui"), // the bundled copy, as today
    register_natives: register_natives,    // input + draw + host_data
};
pub fn register(env: &mut Env) { env.register_native_package(&PACKAGE).expect("ui registers") }
```

For first-party packages, `files` comes from `petal::bundled`, which already
holds them because `core/build.rs` embeds every bundled package. For a
third-party package (`sdl`, `garden`, `studio`), `files` comes from a new
`petal-build` build-dependency: `petal_build::embed_package("../packages/sdl")`
writes the same table into `OUT_DIR`. That is `core/build.rs`'s embedding loop,
lifted into a crate. Until it exists, a host can write `files` by hand with
`include_str!`, as it does today.

Partial registration stays possible. The fps host registers only
`ui/input`, and petal-ui keeps `register_input`, `register_draw_with` and
friends. `register_all`, `register_all_with`, `register_prelude` and
`provide_interface` remain as `#[deprecated]` wrappers for one release.

### Bundled kinds

`core/Cargo.toml [package.metadata.petal.bundled]` goes from four kinds to:

| Kind | Declarations embedded | Natives registered by |
|---|---|---|
| `core` | always | the petal crate, always in scope |
| `builtin` | always | the petal crate, in every `Env`, import-gated |
| `native` (was `ui`) | always, so `petal lint` resolves it with no host | the crate named in the package's `[native]` |
| `library` | when `feature` is on | none (pure Petal) |

`ui = { dir = "../core-stdlib/ui", kind = "native" }`. `build.rs` checks that a
`native` entry's manifest has `[native]`. The `ui` kind name and its
`"ui"`-specific branches go away.

## Before and after

All scripts are unchanged: `import ui`, `import bloom` and `import sdl` resolve
as before. The pairs below are host code, manifests and layout. "Before" is
quoted from the tree; "after" is proposed and has not been built.

**1. A first-party host (petal-sdl, `native_fns.rs:45`)**

```rust
// before
petal_ui::register_prelude(env);
petal_ui::provide_interface(env);
// …register sdl natives…
env.register_package(SDL_PACKAGE, [(SDL_PACKAGE, SDL_INTERFACE),
    (petal::package::MANIFEST_FILE, SDL_MANIFEST)]).expect(…);
env.provides_package(SDL_PACKAGE).expect(…);
```
```rust
// after (proposed)
petal_ui::register(env);
env.register_native_package(&petal_sdl::PACKAGE)?;
```

The writer no longer has to know the order (natives before the prelude, which
is documented at `lib.rs:200`) or remember `provides_package`. That omission
is exactly the one `petal-fps/src/host.rs:64` has today.

**2. The `ui` manifest**

```toml
# before: core-libs/petal-ui/prelude/petal.toml
[package]
name = "ui"
version = "0.2.0"
```
```toml
# after (proposed): core-stdlib/ui/petal.toml
[package]
name = "ui"
version = "0.2.0"
modules = "src"

[native]
crate = "petal-ui"
register = "petal_ui::register"
hint = "use a petal-ui runner such as petal-sdl"
```

**3. The case where the old form is still better: a host-defined interface**

`garden` and `wf_seam` are interfaces that exactly one host implements, and
worlds-fair implements `wf_seam` twice (native in the engine, and in Petal in
the Garden shim, per `wf_seam/petal.toml`). For a package like that,
`[native] crate = …` would be wrong for one of its two implementations. These
keep no `[native]` section and may still use `Env::register_native_package` for the
boilerplate. The manifest section is for packages with one canonical native
crate.

**4. Interaction with hot reload and `state`**

Garden re-registers packages on reload (`panel.rs:2991` runs per panel
`Env`). `Env::register_native_package` is idempotent in the same way `register_package` is:
re-registering replaces the sources, re-registering a native by name replaces
the function, and `provides` overwrites. No `state` slot or call path
involves a package's registration, so reload behaviour is unchanged.

## Assessment

### Project goals
Findings: none of the four README goals involves package layout. Indirectly,
the explicit `[native]` gives introspection something to report: `petal doc ui`
can say where its natives come from, which today needs reading the source.

Opinion: this is maintenance and embedder ergonomics, not language surface.
That is a reason to keep it small, not to skip it.

### Ease of writing
- Script authors: nothing changes.
- Embedders: one call per package instead of 2–4 order-sensitive calls. The
  error for a missing host names the crate to link.
- Library authors: one more manifest section to learn, only for those
  shipping a native crate.

### Bug-freedom
- Prevents: forgetting `provides_package` (petal-fps), registering the prelude
  before the natives (which loses declared parameter names, `lib.rs:200`),
  and silent drift between `native fn` declarations and registered natives,
  since `register_native_package` verifies in debug builds and today only petal-ui's tests do.
- Newly possible: a stale `[native] crate`/`register` string. It is a hint,
  so the cost is a wrong message, not wrong behaviour. `build.rs` can check
  first-party ones against the workspace.

### Performance
No effect on compile or run time. Registration does the same work.
`verify_interface` adds a parse of the declarations at registration in debug
builds only (it parses a package once; `ui` is 13 modules).

### Live coding and hot reload
Covered in pair 4: no change.

### Introspection and program manipulation
No effect on provenance, explain, slicing, time travel, `speculate` or
`propose-edit`. None of them look at how natives were registered.

### Maintainability of the implementation
- Removes: the `ui` bundled kind and its branches in `build.rs`,
  `UI_RUNNER_HINT`, and the hand-kept module list in `petal-ui`.
- Adds: `[native]` parsing (`package.rs`, about 60 lines), `NativePackage` +
  `Env::register_native_package` (about 80), and later a `petal-build` crate (mostly moved
  from `build.rs`).
- Move cost: 247 references to `core-libs/petal-ui` across 119 files in this
  repo, mechanical, plus the `../../core` relative paths inside the crate's
  own `Cargo.toml`, which gain one `../`.

### Domains
- Embedding hosts (Rust, C): helps. The C bridge (`vm.rs:210`) becomes one call.
- Games: helps worlds-fair and cheesecake hosts with the same boilerplate.
- Editor scripting: Garden's three register sites shrink.
- Graphics/web: neutral. web-canvas is first-party and uses `petal_ui::register`.
- Productivity/dashboards: neutral (scripts are unchanged).

### Compatibility and migration
- Scripts: 0 changes.
- Rust API: deprecated wrappers keep every current call compiling for one
  release.
- Paths that break when `petal-ui` moves (Cargo `path` dependencies):
  - this repo: `integrations/petal-desktop-sdl`, `petal-c-bridge`,
    `garden/garden-web`, `test/ui-corpus` and docs (mechanical);
  - petal-creative: `crates/creative-editor/Cargo.toml:13`,
    `crates/retained/Cargo.toml:10`, `petal-movies/Cargo.toml:22`;
  - worlds-fair: the path list in `tools/vendor-petal.ts:51-76` and its
    `ui/Cargo.toml` paths into `vendor/petal`, refreshed on the next vendor;
  - cheesecake: re-vendor `third_party/petal`.

### Prior art
- **Python**: a package may contain C extension modules beside `.py` files,
  one import name and one distribution. That is the model proposed here.
  What went wrong there is building native code at install time. Petal avoids
  that: the host links the crate statically, and the package manager never
  compiles Rust.
- **Lua/LuaRocks**: `require` resolves both `.lua` and C modules via
  `luaopen_*`, a standard entry point like `register`. A C module that is not
  loaded fails late. Petal already fails at load.
- **Node**: N-API addons beside JS, with `package.json` naming the binding.
  `[native] crate` is the same idea, minus building at install time.

### Alternatives considered
- **Do nothing.** Costs: the boilerplate and the `ui` special case stay, and
  `ui` stays filed away from its dependents.
- **Move `petal-ui` only, with no manifest or API change.** This is the
  cheapest version and fixes the tree's reading. It leaves the special
  cases, and `core-stdlib/ui` would be the only package whose layout differs.
  This is a reasonable first step, and phase 1 below is exactly that.
- **Move all of `core-libs` into `core-stdlib`.** Rejected: five of six crates
  are not packages, and "stdlib" would then mean "anything first-party".
- **Dynamic loading of native packages (cdylib).** Out of scope. There is no
  stable Rust ABI, and wasm hosts cannot use it.
- **Require `[native]` on every package with `native fn`.** Rejected; see
  pair 3.

## Implementation sketch

1. **Move** `core-libs/petal-ui` to `core-stdlib/ui`: `prelude/` → `src/`,
   crate → `native/`, `modules = "src"`. Update `core/Cargo.toml`, the Cargo
   path dependencies and the docs. The test is the whole suite unchanged,
   plus `npm run lint:docs`.
2. **`[native]`** in `package.rs`, the `native` bundled kind in `build.rs`, and
   the manifest hint in the unbound-native error, replacing `UI_RUNNER_HINT`.
   Tests: manifest parse/reject cases, and the error text for `ui` and for a
   fixture package.
3. **`NativePackage` + `Env::register_native_package`**, with `petal_ui::register` and the
   wrappers deprecated. Port the in-repo hosts (sdl, web-canvas, c-bridge,
   garden, fps). Tests: the existing `host_natives.rs` drift test runs through
   `register_native_package`.
4. **`petal-build`** (optional): lift the embedding loop out of
   `core/build.rs` so third-party hosts get `files` without listing them.
5. Update `docs/packages.md`, `docs/usage/ffi.md`, the embedding guide,
   `docs/skills/project-organization.md` (steps 5–6 merge) and the
   `core-stdlib` README.

Each phase lands on its own. Phase 1 needs no design decision beyond this
document.

## Decisions

Recorded 2026-11-16 by the maintainer:

- **The other five `core-libs` crates stay where they are.** `core-libs/`
  keeps the first-party Rust crates that are not Petal packages: `gpp`,
  `petal-query`, `petal-pkgman`, `petal-ui-headless`,
  `programs-as-documents`.
- **The domain builtins (`noise`, `color`, …) stay in `core/packages/`**,
  next to their natives in `core/src/builtins/`.
- **The entry point is `Env::register_native_package`, not `Env::install`.**
  "Install" already means fetching git dependencies, both in the CLI
  (`petal install`) and in the Rust API (`Env::set_install_on_demand`,
  `petal_pkgman::install`). The new name fits `register_package` and
  `register_native`. A crate exports `register(env)` (`petal_ui::register`),
  and the manifest key is `[native] register = "…"`.

## Open questions

None outstanding. Phase 1 (the move) can start.
