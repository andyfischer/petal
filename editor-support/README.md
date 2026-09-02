# editor-support

Tooling that helps editors and IDEs work with Petal source (`.ptl`).

## Contents

- [`tree-sitter-petal/`](./tree-sitter-petal/) — the reference
  [tree-sitter](https://tree-sitter.github.io/) grammar for Petal, used for
  syntax highlighting. It ships `grammar.js`, a committed generated parser, a
  `queries/highlights.scm` query, and a Rust crate exposing `LANGUAGE` and
  `HIGHLIGHTS_QUERY`. See its [README](./tree-sitter-petal/README.md).
- [`vim/`](./vim/) — syntax highlighting for stock Vim, and for Neovim without
  a tree-sitter parser installed. Derived from the tree-sitter grammar. See its
  [README](./vim/README.md).

## Who uses it

- Tree-sitter-based editors (Neovim, Helix, Zed, Emacs) install the grammar
  plus `queries/highlights.scm` to highlight Petal.
- Rust applications that embed Petal depend on the `tree-sitter-petal` crate
  by path to highlight `.ptl` files in their own UI. Garden
  (`garden/garden-app`) does this.
