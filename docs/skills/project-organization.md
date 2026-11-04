---
name: project-organization
description: Decide whether new code belongs in the Petal repo at all and, if it does, which directory it goes in. Use before adding a new library, host, builtin, tool or app, when moving code between directories, or when reviewing a change that adds a new top-level piece. Includes the layering rules and a list of code that is in the wrong place today.
---

# Project organization

Use this skill to answer two questions about a piece of code:

1. **Should it be in this repo at all,** or in user space (its own repo, or a
   downstream project)?
2. **If it belongs here, which directory?**

Then check that the dependency direction is legal ([Layering](#layering-rules)).
If the change being discussed is a language or core-design change, also use
the `propose-language-change` skill. That skill covers *whether* to change the
language; this one covers *where the code lives*.

## The principle

This repo is the **language and the things every Petal user shares**. Petal's
users write games, creative and generative work, simulations, web pages,
native desktop apps, editor scripts and embedded/engine scripting (cheesecake,
worlds-fair). Code belongs here only when it serves many of those users. Code
that serves one of them, however good, belongs with that user.

The cost of adding code here is paid by everyone: it is built and tested on
every change, its API is versioned with the language, and anything bundled is
in every `petal` binary and every embedding. The cost of putting code
elsewhere is small, because a Petal package can live in any git repo and be
used as a dependency (`{ git = "…", tag = "…" }`, see
[docs/packages.md](../packages.md)) or vendored.

**When in doubt, put it outside.** Moving a proven library *in* later is easy.
Moving one *out* breaks every user who imported it from the bundled copy.

## Part 1 — In this repo or elsewhere?

### Requirements for code to live here

Code belongs in this repo if it meets **all** of the first three, plus **at
least one** of the last three.

| # | Requirement | Question to ask |
|---|---|---|
| R1 | **Broad.** It is useful to at least two of these kinds of user: games, creative/generative, simulation, web, native desktop apps, embedded/engine scripting, tooling. | Name two real users (in the repo or in the downstream list) that would use it. If you can only name one, it fails. |
| R2 | **Host-neutral, or a host contract.** It does not assume one host's window, file system, audio or event loop, *unless* it is the reference implementation of a host interface other hosts follow. | Would it work unchanged under the C bridge in cheesecake, and in wasm? |
| R3 | **Maintained with the language.** Someone will keep it building when the language changes. It has tests that run in this repo's suites. | If a breaking language change lands tomorrow, does this get fixed in the same commit? |
| R4 | **Needs privileged access.** It needs VM internals, compiler internals or a native (Rust) implementation that cannot be written against the public embedding API. | Could it be written as an ordinary package or crate using only `petal`'s public API? If yes, this does not apply. |
| R5 | **Defines a shared contract.** It is a protocol, host interface package, file format or IR schema that several independent implementations must agree on. | Do two or more separate programs need to agree on it? |
| R6 | **Is the reference example or test corpus.** It exists to show or test how the language is used, and it is small and kept passing. | Is it run by `make test` or the golden suites? Would a newcomer read it to learn Petal? |

### Signs it belongs outside

Any one of these is a strong signal to put the code somewhere else:

- It is an **application**: it has a product purpose of its own (a game, a
  database browser, a git viewer), not a demonstration of a language feature.
- It is **domain-specific**: a physics engine, a particular game's rules, an
  audio synth, an ML pipeline, a business-data format.
- It depends on a **heavy or platform-specific dependency** that most users do
  not need (a GPU API, a database driver, a specific OS framework).
- It **ships on its own schedule**: it wants its own releases, version
  numbers or users.
- It is **opinionated** where users reasonably differ (a particular UI look,
  game architecture, or state management style beyond what the language
  provides).
- It is **experimental and might be abandoned.** Experiments can start in
  `docs/aspirational-ideas/` or a branch. Code only lands here once it passes
  R1–R3.

### Where it goes instead

| What it is | Put it in |
|---|---|
| A reusable Petal library that is domain-specific or opinionated | Its own git repo with a `petal.toml` at the root. Users depend on it with a `git` dependency or vendor it. |
| A reusable Rust host/integration for one platform (a specific engine, a specific GPU API) | Its own repo, depending on the `petal` crate (and `petal-ui` if it draws). |
| A feature of one downstream product | That product: `~/cheesecake`, `~/worlds-fair/ui/ptl`, `~/petal-creative`, and so on. |
| A full application built with Petal | Its own repo, or `sample-apps/`, which is outside the core build. It does **not** go in `examples/` unless it is small and teaches something. |
| Marketing, tutorials, the playground site | `~/biz/petal-lang.org`. |
| A one-off script used during one investigation | The scratchpad, or `temp/` (git-ignored). Never commit it. |

## Part 2 — Where in this repo

### The directories, by layer

Lower layers must never depend on higher ones ([Layering](#layering-rules)).

| Layer | Directory | What belongs there | Language |
|---|---|---|---|
| 0 | `core/src/` | The language: lexer, parser, CST, compiler, IR, type checker, VM, GC, state, reload, introspection (provenance, DM, autodiff), and the embedding API (`Env`, natives, packages). | Rust |
| 0 | `core/src/builtins/` + `core/packages/<name>/` | Natives registered in every `Env`. The `.rs` file implements them; the package declares them. `core` is always in scope; the domain packages (`vec2`, `color`, `noise`, `json`, …) need an `import`. | Rust + `.ptl` decls |
| 0 | `core/prelude/std.ptl` | Pure-Petal helpers implicitly imported everywhere. Must cost nothing when unused. | Petal |
| 0 | `core/src/cli/`, `lsp/`, `fmt/`, `lint/`, `pkgman.rs` | The `petal` command-line tool and the developer tooling every Petal user runs. | Rust |
| 1 | `core-libs/<crate>/` | Rust crates that **hosts** link to provide a shared host interface. Each one holds the natives plus the Petal interface package that declares them (`core-libs/petal-ui` gives `ui`). | Rust (+ `.ptl` interface) |
| 2 | `core-runtime/<package>/` | **Pure Petal** libraries over a host interface or over `core`. No Rust, no build step. Bundled into the binary only if they meet the bundling bar below. | Petal |
| 3 | `integrations/<host>/` | Hosts: the programs and libraries that embed Petal on one platform (SDL desktop, browser canvas, browser DOM, C/C++). Host-specific natives and that host's interface package live here, inside the host. | Rust/TS/C |
| 4 | `garden/` | The Petal IDE. It is an application, kept here as the reference editor and the heaviest user of the embedding API. New code goes in Garden only if it is about the editor. | Rust |
| 5 | `examples/` | Small, runnable programs that teach the language or a host. `examples/console/` is a golden-tested corpus; do not add to it casually. See `examples/AUTHORING.md`. | Petal (Rust only in `custom-apps/`) |
| — | `tools/` | Scripts that develop **this repo**: test runners, generators, benchmarks, the repo's MCP servers. | TypeScript |
| — | `test/` | Cross-cutting test suites (vitest, golden output). Unit tests live next to their code. | |
| — | `editor-support/` | Syntax definitions and grammars for other editors (tree-sitter, vim). | |
| — | `docs/` | User docs at the top level; `dev/` for contributors; `proposals/`, `plan/`, `tasks/` for design records; `skills/` for agent procedures. | Markdown |
| — | `dist/` | The installer scripts that get published. | sh |

### Decision tree

Work through these in order and stop at the first "yes".

1. **Does it change what Petal programs mean,** or how they are parsed,
   compiled, checked, executed, reloaded or introspected?
   → `core/src/`, in the module for that stage. If no module fits, read
   [docs/dev/Architecture.md](../dev/Architecture.md) before creating one.

2. **Is it a function every Petal program should be able to call,** that
   needs native speed or VM access, and that is meaningful on *every* host
   (including headless and wasm)?
   → A native in `core/src/builtins/<group>.rs`, declared in
   `core/packages/<group>/`.
   - Always-in-scope (`core`) only for very general operations: collections,
     strings, math, formatting.
   - Anything with a domain (geometry, color, noise, encoding) goes in an
     import-gated domain package.
   - If it can be written in Petal at acceptable speed, it goes in
     `core/prelude/std.ptl` instead (step 3).

3. **Is it a small, general Petal helper** (list/number/string utilities)
   with no host needs?
   → `core/prelude/std.ptl`. It must be zero-cost when unused.

4. **Is it part of the `petal` command or developer tooling every user runs**
   (a subcommand, formatter rule, lint, LSP feature, package command)?
   → `core/src/cli/`, `lint/`, `fmt/`, `lsp/`, `pkgman.rs`. Put the logic in a
   library module the CLI calls, not in `cli/handlers.rs`, so Garden and the
   LSP can reuse it.

5. **Is it a host interface that several hosts must implement the same way**
   (input, drawing, data fetching, a protocol)?
   → A crate in `core-libs/`. It may depend only on `core` and other
   `core-libs` crates. If the interface is a wire protocol, the protocol
   definition lives here too, not inside one client of it.

6. **Is it a reusable library written in Petal** over `core` or a
   `core-libs` interface, and does it pass R1–R3?
   → A package in `core-runtime/<name>/` with a `petal.toml`, a facade module
   and a README. See
   [docs/dev/sharing-petal-libraries.md](../dev/sharing-petal-libraries.md).
   Bundle it into the binary (`core/build.rs`) only if it meets the
   [bundling bar](#the-bundling-bar).

7. **Is it about running Petal on one platform** (a window system, a browser
   API, a C ABI, an audio backend)?
   → `integrations/<host>/`. Natives and interface packages that only this
   host provides live inside it (as `petal-desktop-sdl/packages/sdl` does),
   not in `core/packages/`.

8. **Is it about editing Petal in Garden** (a pane, keybinding, renderer
   feature, GPP app that ships with Garden)?
   → `garden/`. Follow [garden/CLAUDE.md](../../garden/CLAUDE.md).

9. **Does it exist to teach or demonstrate?**
   → `examples/<category>/`. Keep it small. A program that needs its own Rust
   host or grows into a product goes outside the repo instead (Part 1).

10. **Is it a script for working on this repo?**
    → `tools/` (or `garden/tools/` for Garden-only tooling). If it only serves
    one example or one integration, put it next to that code.

If none of these fit, the code probably belongs outside the repo.

### The bundling bar

Everything in `core/build.rs`'s package list ships inside every `petal` binary
and wasm build, and its API is locked to the runtime version. A package is
bundled only if it:

- passes R1–R3 above,
- is stable (not marked experimental), and
- is used by at least two hosts or downstream projects *without being
  vendored*.

Packages that don't meet this bar can still live in `core-runtime/`. Users
then reach them with a path dependency or `petal.toml`, and they are not
compiled into the binary.

## Layering rules

Dependencies point **down** the layer table only:

```
examples/, garden/ (apps)
        ↓
integrations/ (hosts)
        ↓
core-runtime/ (pure Petal packages)
        ↓
core-libs/ (shared host-interface crates)
        ↓
core/ (language + CLI)
```

- `core/` depends on no other directory in the repo, and neither its source
  nor its tests name any specific host.
- `core-libs/` depends on `core/` and other `core-libs/` crates only. **Never
  on `garden/` or `integrations/`.**
- `core-runtime/` packages depend on `core`, `core-libs` interface packages
  (via `[requires]`), and other `core-runtime` packages.
- `integrations/` never depend on `garden/` or on `examples/`.
- `examples/` and `garden/` may depend on anything below them.
- `tools/` may depend on anything, but nothing may depend on `tools/`.

A quick check for Rust crates:

```bash
git ls-files '*Cargo.toml' | xargs grep -Hn 'path *= *"\.\./'
```

Any `path` that climbs to a higher layer is a violation.

## Naming

- Directory names are kebab-case and say what the thing *is*
  (`petal-desktop-sdl`, `text-layout`). The crate or package name should match
  the directory. Where an identifier can't (`text_layout`), the manifest says
  so.
- Do not create new top-level directories without a decision recorded in
  `docs/`. The top level is the table in [README.md](../../README.md#repository-layout);
  update that table when it changes.

## Current misplacements (audit of 2026-11-04)

These are the places where today's tree does not follow the rules above, most
important first. Each has a proposed fix. Nothing here has been moved yet.
Check the current tree before acting on one, and remove an entry from this
list once it is fixed.

### Layering violations

1. **`core-libs/petal-query` depends on `garden/gpp`.** A layer-1 shared
   library depends on the IDE, and through petal-query's optional feature
   `integrations/petal-c-bridge` pulls in `garden/gpp` too. GPP is already a
   cross-language protocol (`garden/gpp-python`, the spec in
   `garden/docs/gpp.md`), so it is a shared contract (R5), not a Garden
   internal.
   *Fix:* move `garden/gpp` to `core-libs/gpp` (and consider
   `garden/gpp-python` alongside it). Garden then depends on it from below.

2. **The `web` host lives in `core`.** `core/src/wasm.rs` holds
   `PetalRuntime`, the element-tree host used only by
   `integrations/petal-web-html`, and its interface package is
   `core/packages/web`. That is host-specific code in layer 0, against R2.
   *Fix:* keep in `core` only the generic wasm embedding surface that any
   browser host needs. Move the element-tree natives and `packages/web` into
   `integrations/petal-web-html` (a small wasm crate there, like
   `petal-web-canvas/rust`).

### Too much inside the `petal` library crate

3. **The CLI, LSP, formatter, linter, package manager and `fix-imports` are
   all public modules of the `petal` library crate** (`core/src/lib.rs`: `cli`,
   `lsp`, `fmt`, `lint`, `pkgman`, `fix_imports`, `suggest`). So every
   embedder (C bridge, Garden, petal-sdl, wasm) compiles them, and they are
   part of the public API surface. That is about 25k lines. These belong in
   this repo (decision-tree step 4), but not in the runtime crate.
   *Fix, in order of cost:* (a) put them behind a default-on `tooling` cargo
   feature that embedders and the wasm build turn off. (b) Later, split them
   into a `petal-cli` crate (a binary that also exposes `lsp`/`fmt`/`lint` as
   a library) that depends on `petal`. Garden already needs `fmt`/`lint`, so
   it would depend on that crate.

### Bundled packages that don't meet the bundling bar

4. **`glsl` is bundled but marked experimental** (`core-runtime/glsl`). An
   experimental API inside every binary gets locked to runtime versions.
   *Fix:* drop it from `core/build.rs`'s list and keep it in `core-runtime/`
   as a path dependency until it stabilizes.

5. **`bloom` is bundled.** It is an opinionated widget kit (a look, a
   component set). It is useful, but it fails "opinionated" and should be
   free to version on its own. Worth a decision, not urgent: either keep it
   bundled as *the* default widget set and say so in
   `core-runtime/README.md`, or unbundle it.

### Applications and app-specific tooling in the repo

6. **`examples/custom-apps/petal-fantasy-nes`** (about 5,200 lines of Rust,
   a PPU/APU emulator core) and **`examples/custom-apps/petal-fps`** (about
   1,600 lines of Rust) are products with their own hosts, not small teaching
   examples. They do act as the main test of the petal-sdl `Host` trait.
   *Fix:* move them to their own repos (or `sample-apps/`) once petal-sdl's
   `Host` trait has a small in-repo example and tests that cover what they
   exercise.

7. **`garden/gpp-apps/sqlite-browser`, `git-viewers`, `directory-browser`.**
   These are applications that use GPP. Keep the ones Garden ships as
   built-in panes and the test app. Move the rest out, or justify them as
   examples of writing GPP apps.

8. **`tools/petal-diagram-mcp.ts`** serves only
   `examples/custom-apps/diagram-canvas`. *Fix:* move it next to that example.

9. **`tools/calc-to-ir.ts`** is a reference external front end that shows
   off the IR-as-target contract. It is an example, not a repo tool.
   *Fix:* move it to `examples/` (for instance `examples/ir-target/`) and
   link it from `docs/dev/ir-as-target.md`.

### Inconsistencies and leftovers

10. **`core/prelude/std.ptl` sits outside `core/packages/`,** even though
    `std` is a package-shaped module like the others. *Fix:* consider
    `core/packages/std/` with a `petal.toml`, so every bundled Petal source
    sits in one place. Low priority.

11. **`integrations/petal-desktop-sdl`'s crate and binary are still named
    `petal-sdl`.** Known (its README says so). Rename when downstream
    references can be updated in the same change.

12. **Untracked leftover directories at the repo root:** `petal-ui/` (empty),
    `playground/` and `ts/` (only `node_modules`), `rust/pkg/` (an old wasm
    build). None are tracked by git. *Fix:* delete them locally. They are
    leftovers from older layouts and mislead anyone who reads the tree.

## Procedure

When asked where something should go, or when you are about to add a new
library, host, builtin, tool or app:

1. Run Part 1. If it fails R1–R3, recommend a home outside the repo and say
   which signal it hit.
2. Run the decision tree in Part 2 and name the directory.
3. List its dependencies and check them against the layering rules.
4. If it would be bundled, check the bundling bar.
5. If it touches the language, also use `propose-language-change`. If it
   changes something downstream projects import, check them as
   `CLAUDE.local.md` describes.
6. Give the answer as: **where**, **why** (which rule), and **what it must
   not depend on**.

When you move code to fix one of the misplacements above, do the move and
nothing else in its own commit. Update the README layout table and any docs
that link the old path, and remove the entry from the list above.
