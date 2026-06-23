# tree-sitter-petal

A [tree-sitter](https://tree-sitter.github.io/) grammar for the
[Petal](../../README.md) language. It is the reference editor-support
implementation: editors that embed tree-sitter (Garden, Neovim, Helix, Zed, …)
can use it to syntax-highlight `.ptl` files.

The grammar models the surface syntax produced by the canonical lexer
(`rust/src/lexer.rs`) and parser (`rust/src/parse.rs`): declarations (`let`,
`state`, `fn`, `enum`), control flow (`if`/`elsif`/`else`, `for`, `while`,
`match`/`when`), expressions with the full precedence ladder (`|>` → `||` → `&&`
→ equality → comparison → `++` → additive → multiplicative → unary → postfix),
records, lists, lambdas, patterns, string interpolation, color literals, and
JSX elements.

## Layout

```
grammar.js              the grammar definition (source of truth)
queries/highlights.scm  syntax-highlighting query (tree-sitter capture names)
tree-sitter.json        grammar metadata + file types
src/                    generated parser (parser.c, node-types.json, …) — committed
bindings/rust/          Rust crate (LANGUAGE + HIGHLIGHTS_QUERY), built via cc
test/corpus/            parse tests (`tree-sitter test`)
```

The generated `src/` is committed so downstream consumers (the Rust crate, in
particular) build without needing the tree-sitter CLI.

## Using it from Rust

The crate exposes the standard pair consumed by `tree-sitter-highlight`:

```rust
let language = tree_sitter_petal::LANGUAGE;          // LanguageFn
let query    = tree_sitter_petal::HIGHLIGHTS_QUERY;  // &str
```

Garden depends on it as a path dependency
(`tree-sitter-petal = { path = "../../petal/editor-support/tree-sitter-petal" }`)
and registers it in `garden-app/src/syntax.rs`.

## Developing

```sh
tree-sitter generate          # regenerate src/ from grammar.js
tree-sitter test              # run test/corpus
tree-sitter parse FILE.ptl    # inspect a parse tree
cargo test                    # build the C parser + load it from Rust
```

After editing `grammar.js` you must re-run `tree-sitter generate` and commit the
regenerated `src/`.

## Design notes & known limitations

- **Newlines are insignificant** (treated as whitespace, along with `;`). The
  real parser uses them as statement separators, but statement boundaries are
  recoverable from structure in practice, and ignoring them keeps comma-less
  juxtaposition (`[1 2 3]`, `color(0 1 2)`) simple. All 77 non-aspirational
  `.ptl` files in this repo parse without errors.
- **Spacing-sensitive minus** (`lexer.rs`'s `MinusPrefix`: a `-` with space
  before but not after begins a new negated element in comma-less lists, so
  `[1 -2]` is two elements). This grammar does not model it — it would need an
  external scanner — so `[1 -2]` parses as the subtraction `1 - 2`. This has **no
  effect on highlighting** (both color `1`, `-`, `2` identically). Lists that
  use commas (`[1, -2]`) are unaffected.
- **JSX** is highlighted but not deeply validated: a `<tag` / `</tag` opening is
  recognized as a single token (a `<` immediately followed by a letter, matching
  the lexer), so comparisons must be written with a space (`a < b`) — again
  matching the lexer, which treats `<` + letter as a tag start.
- **String interpolation** parses the embedded `{expr}` as a real expression,
  so interpolated code highlights correctly.
