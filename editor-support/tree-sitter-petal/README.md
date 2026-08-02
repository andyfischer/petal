# tree-sitter-petal

A [tree-sitter](https://tree-sitter.github.io/) grammar for the
[Petal](../../README.md) language. It is the reference editor-support
implementation: editors that embed tree-sitter (Neovim, Helix, Zed, …)
can use it to syntax-highlight `.ptl` files.

The grammar models the surface syntax produced by the canonical lexer
(`rust/src/lexer.rs`) and parser (`rust/src/parse.rs`): declarations (`let`,
`var`/`set`, `state`, `fn`, `enum`, `class` and the `fn Class.method` form),
control flow (`if`/`elsif`/`else`, `for`, `while`, `match`/`when`), expressions
with the full precedence ladder (`|>` → `||` → `&&` → equality → comparison →
`++` → additive → multiplicative → unary → postfix), records, lists, lambdas,
patterns, string interpolation, color literals, and JSX elements.

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

Embedding applications can depend on it as a path dependency
(`tree-sitter-petal = { path = "path/to/petal/editor-support/tree-sitter-petal" }`).

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
  recoverable from structure in practice. All non-aspirational `.ptl` files in
  this repo parse without errors, except those using the `@` rebind operator,
  which this grammar does not model yet.
- **Type annotations** (`let x: int = …`, `state n: int = 0`, `fn f(a: int) ->
  int`) are modelled as `type_annotation` / `return_type` wrapping a
  `type_name`. Type names are contextual identifiers, not a closed keyword set,
  so an unrecognized name still parses — matching the real parser, where the
  *checker* warns about it. Lambdas take parameter annotations but no return
  annotation, since their `->` introduces the body.
- **Commas are required** between the elements of every delimited list, matching
  the real parser (see `docs/syntax/commas.md`); a trailing comma before the
  closing delimiter is allowed. Because a comma always ends an element, `-` is
  unambiguous here too — no external scanner needed.
- **JSX** is highlighted but not deeply validated: a `<tag` / `</tag` opening is
  recognized as a single token (a `<` immediately followed by a letter, matching
  the lexer), so comparisons must be written with a space (`a < b`) — again
  matching the lexer, which treats `<` + letter as a tag start.
- **String interpolation** parses the embedded `{expr}` as a real expression,
  so interpolated code highlights correctly.
