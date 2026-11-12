# tree-sitter-petal

A [tree-sitter](https://tree-sitter.github.io/) grammar for the
[Petal](../../README.md) language. Editors that embed tree-sitter (Neovim,
Helix, Zed, Emacs) can use it to highlight `.ptl` files, and Rust programs can
depend on it as a crate. Garden, the editor in `garden/`, uses it this way.

The grammar follows the canonical lexer (`core/src/lexer.rs`) and parser
(`core/src/parse.rs`): declarations (`let`, `const`, `var`/`set`/`get`, `state`, `fn`,
`enum`, `class` and `fn Class.method`), control flow (`if`/`else if`/`else`,
`for`, `while`, `match`/`when`), indentation-delimited blocks with `end` as an
optional closer, the full operator precedence ladder
(`|>` → `||` → `&&` → equality → comparison → `??` → `++` → additive →
multiplicative → unary → postfix, including `?.`), records, lists, lambdas,
patterns, string interpolation, `:f80` colour literals, `#` comments, and JSX
elements.

## Layout

```
grammar.js              the grammar definition (source of truth)
queries/highlights.scm  syntax-highlighting query (tree-sitter capture names)
queries/indents.scm     indentation query (nvim-treesitter capture names)
tree-sitter.json        grammar metadata + file types
src/scanner.c           hand-written external scanner: layout, `else if`, colours
src/                    generated parser (parser.c, node-types.json, …) — committed
bindings/rust/          Rust crate (LANGUAGE + HIGHLIGHTS_QUERY + INDENTS_QUERY), built via cc
test/corpus/            parse tests (`tree-sitter test`)
```

The generated `src/` is committed so the Rust crate builds without the
tree-sitter CLI. `src/scanner.c` is the one file there written by hand;
`tree-sitter generate` leaves it alone. Anything that compiles the parser must
compile both `src/parser.c` and `src/scanner.c` (the Rust crate's
`bindings/rust/build.rs` does, which is all Garden needs).

## Layout: blocks, `end` and the external scanner

Blocks close by indentation, and `end` is an optional closer
([docs/tasks/pythonic-syntax.md](../../docs/tasks/pythonic-syntax.md)). Both
spellings parse, block by block, to the same tree (`end` is an anonymous
token):

```petal ignore
fn clamp_all(xs, lo, hi)          fn clamp_all(xs, lo, hi)
  for x in xs do                    for x in xs do
    if x < lo then lo                 if x < lo then lo
    else if x > hi then hi            elsif x > hi then hi
    else x                            else x
                                      end
                                    end
                                  end
```

The grammar still keeps newlines in `extras`. Layout comes from three
zero-width tokens the scanner emits only where the parser can take one:

| Token | Emitted |
|---|---|
| `_indent` | after a block opener (`then`, `else`, `do`, an arm's `->`, a `fn` / `class` / `enum` / `match` header) whose body starts on a later line, indented deeper than the enclosing block |
| `_dedent` | when a line starts left of the open block; at a closing bracket that ends a block opened inside the bracket; at the end of the file |
| `_inline_end` | at the line break that ends a one-line body with no `end` |

So every body has two shapes: an indented block (`_indent … _dedent`, then an
optional `end`), or a one-line body closed by an `end` on the line, by the line
break, or by a token that cannot continue it (the `)` in
`map(xs, fn(v) v * 2)`). Both are `block` nodes.

- **Inside brackets** (`(…)`, `[…]`, `{…}`, JSX) no layout token is valid, so
  line breaks there are whitespace. A block opened inside a bracket turns
  layout back on, measured from the enclosing block (the in-bracket offside
  rule, §3.2 of the plan); its closing bracket ends it.
- **A line starting with `else` / `else if`** continues a one-line `if` on the
  line above (B4), unless it sits left of the block that `if` is in, where it
  belongs to an enclosing `if`. An `end` line follows the same rule.
- **A line starting with a binary operator** (`+`, `|>`, `&&`, `.field`, … but
  not `-` or `<`) continues the statement above it and never closes anything.
- **Comment-only and blank lines** do not count. Lines inside a string do not
  start a line.
- **Base indent.** The first statement's column is the left margin, so a
  snippet indented as a whole parses as it would at column 0.
- **`else if`** chains when the two words share a line; the scanner lexes that
  `if`. `else`, a line break, then `if` is an `else` block containing an `if`.
  `elsif` is still accepted. All three give an `elsif_clause` node.
- **`:f80` colours** are lexed by the scanner too: `:` + 3, 4, 6 or 8 hex
  digits + a non-identifier character, and only where an expression may
  start, so `{x:100}`, `f(a:fff)` and `let c:add = 1` keep `:` as a separator.
- **`#` comments.** `#` starts a comment to the end of the line, `//` is the
  deprecated spelling, and `/* … */` stays. There is no `#f80` colour: it is a
  comment. A children line that starts with `#` is element text.

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

- **Newlines between statements are insignificant.** They are treated as
  whitespace, like `;`. The real parser uses them as statement separators, but
  statement boundaries are recoverable from structure in practice. One visible
  effect: a line starting with `-` reads as a subtraction from the line above.
- **Layout is lenient.** The real parser reports a misaligned `end`, an
  unindented body, an unexpected indent, and `;` after a one-line `then` with
  no `end`. This grammar reports none of them: it parses what it can and moves
  on, which is what a highlighter wants.
- **A stray `end` is an identifier.** Legacy `else if … end end` (an `if`
  nested in an `else` on one line) now chains, so its second `end` closes
  nothing. tree-sitter falls back to reading that word as an identifier
  instead of reporting an error; the real parser reports it, and
  `petal migrate-old-code` fixes it.
- **`@`** is not modelled. The language has no `@` form today (the old `f(@x)`
  rebind operator was removed on 2026-11-09, and `@` is reserved for
  annotations), so a file with one is a parse error in both parsers.
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
  unambiguous inside a list.
- **JSX** is highlighted but not deeply validated. A `<tag` / `</tag` opening
  is one token (`<` immediately followed by a letter, as in the lexer), so
  comparisons must be written with a space: `a < b`.
- **String interpolation** parses the embedded `{expr}` as a real expression,
  so interpolated code highlights correctly.
