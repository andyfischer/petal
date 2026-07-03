# editor-support

Tooling that helps editors and IDEs work with Petal source (`.ptl`).

## Contents

- [`tree-sitter-petal/`](./tree-sitter-petal/) — the reference
  [tree-sitter](https://tree-sitter.github.io/) grammar for Petal, used for
  syntax highlighting. It ships a `grammar.js`, a committed generated parser, a
  `highlights.scm` query, and a Rust crate exposing `LANGUAGE` +
  `HIGHLIGHTS_QUERY`. See its [README](./tree-sitter-petal/README.md).

## Consumers

- Any tree-sitter-based editor (Neovim, Helix, Zed, Emacs) can install the
  grammar + `queries/highlights.scm` to highlight Petal.
- Applications that embed Petal as a scripting layer can depend on the
  `tree-sitter-petal` Rust crate (as a path dependency) to highlight `.ptl`
  files in their own UI.
