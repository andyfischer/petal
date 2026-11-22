# core-sdk

First-party Rust crates for building programs around Petal: hosts, tools and
applications that embed the language. Each crate is built on the public API
of `petal` (the `core/` crate). A Rust program links the crates it needs, and
an embedder that doesn't need one leaves it out.

| Crate | What it is | Who links it |
|-------|------------|--------------|
| [`gpp/`](gpp/) | The Garden Pane Protocol wire definition: the JSON-RPC envelope, params/results and NDJSON framing. Kept dependency-slim (serde only) so a host and a client can both link it | Garden, `petal-query` |
| [`petal-query/`](petal-query/README.md) | A React-Query-style data layer for Petal UI panels: `Provider`/`Reply` on the data side, `Cache` with freshness on the host side | GPP apps, hosts that answer `query(kind, arg)` |
| [`petal-ui-headless/`](petal-ui-headless/README.md) | Runs a `ui` script with no renderer: the `Headless` test driver, input scenarios, drag tracing, and the `petal-ui-run` / `petal-ui-propose-edit` CLIs | Tests, repo tools, Garden's panel runner |
| [`programs-as-documents/`](programs-as-documents/README.md) | A `.ptl` script used as an application's document: edits made in the running app are resolved back into the source and verified by a rerun | Apps whose files are Petal scripts |
| [`transpile_to_react_js/`](transpile_to_react_js/README.md) | Petal source → React.js source: element functions become components, `state` becomes `useState`, a Mixit design becomes components on a small runtime, and what has no React form is a warning | The `petal-to-react` CLI, Mixit's implementation tools |
| [`petal-pkgman/`](petal-pkgman/) | The package manager: git dependencies, `petal.lock`, `vendor/`, bundles | Only the `petal` CLI (`cli/`) |

## Why these aren't in core-stdlib

[`core-stdlib/`](../core-stdlib/README.md) holds **Petal packages**. Each one
has a `petal.toml` and `.ptl` modules that a script reaches with `import`.
Some also have a Rust crate in `native/` that registers the natives the
modules declare.

The crates here are **Rust libraries for the Rust side**. None of them has a
`petal.toml` or anything a script can import. Hosts, the CLI and repo tools
link them, the same way they link `petal` itself. Petal code never sees them.

They also sit a layer above `core-stdlib`. `petal-ui-headless` depends on
`core-stdlib/ui/native`, and a `core-stdlib` native crate may depend only on
`core`. So a crate here may depend on `core`, on `core-stdlib` native crates
and on other `core-sdk` crates. It may never depend on `garden/` or
`integrations/`.

**The rule of thumb:** if a script calls it, it's a package in `core-stdlib/`.
If only Rust code calls it, it's a crate here. The full layering is in
[docs/skills/project-organization.md](../docs/skills/project-organization.md).

## Why "sdk" and not "extensions"

An extension usually adds abilities that a script can call. In Petal, those
are the native packages in `core-stdlib/*/native`. The crates here go the
other direction: they help Rust programs host, test, drive or package Petal.
That makes them an SDK.
