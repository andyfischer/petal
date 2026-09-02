# tree-sitter-petal

A [tree-sitter](https://tree-sitter.github.io/) grammar for the
[Petal](../../README.md) language. Editors that embed tree-sitter (Neovim,
Helix, Zed, Emacs) can use it to highlight `.ptl` files, and Rust programs can
depend on it as a crate. Garden, the editor in `garden/`, uses it this way.

The grammar follows the canonical lexer (`rust/src/lexer.rs`) and parser
(`rust/src/parse.rs`): declarations (`let`, `var`/`set`/`get`, `state`, `fn`,
`enum`, `class` and `fn Class.method`), control flow (`if`/`elsif`/`else`,
`for`, `while`, `match`/`when`), the full operator precedence ladder
(`|>` → `||` → `&&` → equality → comparison → `??` → `++` → additive →
multiplicative → unary → postfix, including `?.`), records, lists, lambdas,
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

The generated `src/` is committed so the Rust crate builds without the
tree-sitter CLI.

## Using it from Rust

The crate exposes the pair `tree-sitter-highlight` expects:

```rust
let language = tree_sitter_petal::LANGUAGE;          // LanguageFn
let query    = tree_sitter_petal::HIGHLIGHTS_QUERY;  // &str
```

Depend on it by path:

```toml
tree-sitter-petal = { path = "path/to/petal/editor-support/tree-sitter-petal" }
```

## Developing

The tree-sitter CLI is a dev dependency; `npm install` in this directory
fetches it. Then:

```sh
npx tree-sitter generate      # regenerate src/ from grammar.js
npx tree-sitter test          # run test/corpus
npx tree-sitter parse FILE.ptl
cargo test                    # build the C parser and load it from Rust
```

(`npm run generate` / `npm test` / `npm run parse` are aliases for the first
three.) After editing `grammar.js`, re-run `generate` and commit the
regenerated `src/`.

## Known differences from the real parser

- **Newlines are insignificant.** They are treated as whitespace, like `;`.
  The real parser uses them as statement separators, but statement boundaries
  are recoverable from structure in practice.
- **The `@` rebind operator** ([docs/syntax/rebind-operator.md](../../docs/syntax/rebind-operator.md))
  is not modelled. Files that use it produce parse errors.
- **Type annotations** (`let x: int = …`, `state n: int = 0`,
  `fn f(a: int) -> int`) are `type_annotation` / `return_type` nodes wrapping a
  `type_name`. Type names are contextual identifiers, not a closed keyword set,
  so an unknown name still parses — the real parser's checker warns about it.
  Lambdas take parameter annotations but no return annotation, since their
  `->` introduces the body.
- **Named arguments** (`f(x, limit: 10)`) parse as a `named_argument` inside
  the `argument_list`. As with `record_field`, only an identifier is accepted
  as the name; the real parser also accepts a keyword there (`f(end: 1)`).
- **Commas are required** between the elements of every delimited list,
  matching the real parser ([docs/syntax/commas.md](../../docs/syntax/commas.md));
  a trailing comma is allowed. Because a comma always ends an element, `-` is
  unambiguous and no external scanner is needed.
- **JSX** is highlighted but not deeply validated. A `<tag` / `</tag` opening
  is one token (`<` immediately followed by a letter, as in the lexer), so
  comparisons must be written with a space: `a < b`.
- **String interpolation** parses the embedded `{expr}` as a real expression,
  so interpolated code highlights correctly.
