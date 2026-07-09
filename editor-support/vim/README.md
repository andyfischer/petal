# Vim syntax support

Classic-Vim syntax highlighting for Petal (`.ptl`), for stock Vim and any
Neovim without a tree-sitter parser installed. Editors that support
[tree-sitter](https://tree-sitter.github.io/) (Neovim, Helix, Zed, Emacs) should
prefer the richer [`tree-sitter-petal`](../tree-sitter-petal/) grammar instead.

## Contents

- [`syntax/petal.vim`](./syntax/petal.vim) — the syntax definition, derived from
  the reference grammar in `../tree-sitter-petal/grammar.js`.
- [`ftdetect/petal.vim`](./ftdetect/petal.vim) — maps `*.ptl` to the `petal`
  filetype.

## Install

Copy both files into your Vim runtime directory, preserving the layout:

```sh
cp syntax/petal.vim    ~/.vim/syntax/petal.vim
cp ftdetect/petal.vim  ~/.vim/ftdetect/petal.vim
```

For Neovim, use `~/.config/nvim/` instead of `~/.vim/`. Plugin managers
(vim-plug, packer, lazy.nvim, etc.) that add this directory to the runtimepath
will pick the files up automatically.
