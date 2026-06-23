# editor-support

Tooling that helps editors and IDEs work with Petal source (`.ptl`).

## Contents

- [`tree-sitter-petal/`](./tree-sitter-petal/) — the reference
  [tree-sitter](https://tree-sitter.github.io/) grammar for Petal, used for
  syntax highlighting. It ships a `grammar.js`, a committed generated parser, a
  `highlights.scm` query, and a Rust crate exposing `LANGUAGE` +
  `HIGHLIGHTS_QUERY`. See its [README](./tree-sitter-petal/README.md).

## Consumers

- **Garden** (`~/garden`) — a GPU IDE that embeds Petal as its scripting layer;
  it depends on `tree-sitter-petal` as a path dependency and registers it in
  `garden-app/src/syntax.rs`, so `.ptl` files highlight in the editor.
- Any tree-sitter-based editor (Neovim, Helix, Zed, Emacs) can install the
  grammar + `queries/highlights.scm` to highlight Petal.
