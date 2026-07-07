# Handoff: Step 3b-ii — wire the parser to `EventBuilder`

Part of the source-preservation work; read
[source-preservation-plan.md](source-preservation-plan.md) first for the full
context. This document is self-contained for the next step: a fresh session
needs only this plus the plan.

## Goal

Make `parse.rs` emit a CST `Event` stream **alongside** its existing AST
construction (dual output — the AST stays authoritative). Then
`cst::build_tree` turns those events + the lexer output into a *structured*
green tree that round-trips the whole corpus. **Do not change any AST-building
logic or control flow** — the event calls are a pure side channel.

## What's already in place (don't rebuild)

- `cst::EventBuilder` with `open(kind)`, `close()`, `token()`,
  `checkpoint() -> Checkpoint`, `wrap(cp, kind)`, `events()`.
- `cst::build_tree(events, tokens, spans, leading_trivia, source) -> Rc<GreenNode>`
  — interleaves trivia and flushes unconsumed trailing tokens (Eof) into `Root`.
- All grammar `SyntaxKind` node variants (`LetStmt`, `BinaryExpr`, `CallExpr`,
  `ArgList`, `Block`, `ParenExpr`, `MatchArm`, …).
- The `wrap` semantics are proven: checkpoint **before** the left operand,
  `wrap` **after** the right, repeated wraps nest outward → left-associative.

## Parser plumbing to add (in `parse.rs`)

1. Fields on `Parser`:
   ```rust
   events: cst::EventBuilder,
   record_cst: bool,   // false in the normal `new`; a new ctor sets it true
   ```
2. Guarded helpers (no-ops when `!record_cst`; only `open`/`close`/`token`/`wrap`
   need the guard — `checkpoint` is harmless when not recording since `events`
   stays empty):
   ```rust
   fn ev_open(&mut self, k: SyntaxKind)   { if self.record_cst { self.events.open(k); } }
   fn ev_close(&mut self)                 { if self.record_cst { self.events.close(); } }
   fn ev_checkpoint(&self) -> Checkpoint  { self.events.checkpoint() }
   fn ev_wrap(&mut self, cp: Checkpoint, k: SyntaxKind) { if self.record_cst { self.events.wrap(cp, k); } }
   ```
3. **Token events come for free**: add one line to `advance()` (parse.rs:59) —
   `if self.record_cst { self.events.token(); }`. Every consumed token routes
   through `advance()` (via `expect`, `expect_ident`, `skip_newlines`,
   `skip_separator`), so you never call `token()` by hand. **Verify** no code
   reads `self.tokens[self.pos]` and bumps `pos` without going through
   `advance()`.
4. Entry point (put in `cst.rs` or `rewrite.rs`):
   ```rust
   pub fn parse_cst(source: &str) -> Result<Rc<GreenNode>, String> {
       let mut lexer = Lexer::new(source); lexer.tokenize()?;
       let mut p = Parser::new_recording(lexer.tokens.clone(), lexer.token_spans.clone());
       p.parse_program()?;                       // discard the Vec<Stmt>; we want the events
       Ok(build_tree(p.events.events(), &lexer.tokens, &lexer.token_spans,
                     &lexer.token_leading_trivia, source))
   }
   ```

## Instrumentation pattern

- **Non-left-recursive constructs** (statements, primaries, collections,
  if/match/lambda/jsx): `ev_open(KIND)` at the point where the node's first token
  is about to be consumed, `ev_close()` at the end. Place `ev_open` **after** any
  leading `skip_newlines()` so inter-statement newlines stay outside the node.
- **Left-recursive constructs** (all Pratt levels, `parse_postfix`,
  `parse_pipe`): `let cp = self.ev_checkpoint();` **before** parsing `left`;
  inside the loop, **after** parsing `right`, `self.ev_wrap(cp, KIND)`. The
  operator token is consumed by the existing `advance()`.
- **`parse_program` must NOT open a `Root`** — `build_tree` already frames `Root`.

## Function → node-kind map

| Function (parse.rs line) | Instrumentation |
|---|---|
| `parse_let` 166 | open/close `LetStmt` |
| `parse_state` 174 | `StateStmt` |
| `parse_import` 197 | `ImportStmt` |
| `parse_fn_decl` 227 | `FnDecl`; wrap param list in `ParamList`, body via `parse_block_until` |
| `parse_enum_decl` 239 | `EnumDecl` |
| `parse_for` 261 / `parse_while` 273 | `ForStmt` / `WhileStmt` |
| `parse_return` 283 | `ReturnStmt` |
| Break/Continue inline (parse_stmt 151–158) | `BreakStmt` / `ContinueStmt` |
| `parse_expr_or_assign` 293 | `cp` before `parse_expr`; if `=`/compound → `ev_wrap(cp, AssignStmt)`, else `ev_wrap(cp, ExprStmt)` |
| `parse_block_until` 333 | open/close `Block` around the loop |
| `parse_or/and/equality/comparison/concat/additive/multiplicative` 429–605 | checkpoint+`wrap(BinaryExpr)` per the left-recursive pattern |
| `parse_unary` 607 | only the `Minus`/`Bang` arms: open/close `UnaryExpr` (the `_ => parse_postfix()` arm opens nothing) |
| `parse_postfix` 630 | `cp` before `parse_primary`; per arm wrap `FieldAccessExpr` / `IndexAccessExpr` / `CallExpr`; wrap the `(...)` args in `ArgList` |
| `parse_primary_inner` 729 | literals/ident/at → `LiteralExpr`/`IdentExpr`/`AtVarExpr`; `Color` → `LiteralExpr` (or a color kind); **`LParen` grouping → wrap in `ParenExpr`** (open before `(`, close after `)`) |
| `parse_list_literal` 798 | `ListExpr` |
| `parse_record_literal` 819 | `RecordExpr`; each field → `RecordField` |
| `parse_if_expr` 841 / `parse_else_chain` 859 | `IfExpr`; elsif/else tails → `ElseBranch` |
| `parse_match_expr` 889 / `parse_match_arm` 904 / `parse_pattern` 928 | `MatchExpr` / `MatchArm` / `Pattern` |
| `parse_lambda` 1046 | `LambdaExpr` (+ `ParamList`) |
| `parse_string_interp` 1066 | `StringInterpExpr` (when it degrades to a plain string with no exprs, `LiteralExpr` is fine) |
| `parse_jsx_element` 1108 | `ElementExpr`; each attribute → `JsxAttr`; nested elements recurse |

`parse_pipe` 406 is left-recursive and rewrites `a |> f` into a `Call` in the
AST. For the CST, checkpoint+`wrap` it as `CallExpr` (or add a `PipeExpr` kind if
you want to distinguish pipe syntax) — a judgment call; `CallExpr` matches the
semantics and keeps the kind set small.

## Gotchas

- **Zero behavioral change**: instrumentation must not alter which tokens are
  consumed or the AST produced. Guard is: every existing test still passes with
  `record_cst=false` (the default), and the AST is byte-identical.
- **Error paths**: on a parse error the event stream is unbalanced — fine,
  because `parse_cst` returns the error and never calls `build_tree`. Only build
  the tree on `Ok`.
- **Pratt checkpoint placement**: take `cp` *before*
  `let mut left = self.parse_lower()?`, not after.
- **`parse_primary` wrapper** (717) toggles `in_juxta` around
  `parse_primary_inner`; put the `ev_open`/`ev_close` inside `parse_primary_inner`,
  not the wrapper.
- **Don't hand-write `token()`** anywhere; it's only in `advance()`.
- **Trivia placement is not a correctness concern** — losslessness is defined by
  `text() == src`, not by which node a comment lands in. Don't chase "perfect"
  comment attachment now; refine in a later pass.

## Validation (definition of done)

1. New test mirroring the existing corpus tests: for every repo `.ptl` that
   parses, `parse_cst(src).text() == src`. (Copy the `collect_ptl` walker from
   `cst.rs`/`trivia.rs`.)
2. `build_tree`/`GreenNodeBuilder::finish` already assert balance — an
   unbalanced open/close will panic, catching missed `ev_close`.
3. Structural spot tests: `1 + 2 * 3` → outer `BinaryExpr` whose right child is a
   `BinaryExpr`; `f(a, b)` → `CallExpr` containing an `ArgList`; `(a + b) * c` → a
   `ParenExpr` under the `BinaryExpr`.
4. Full existing suite green (AST unchanged): `cargo test`.

## After 3b-ii

- **3c**: write `ast::project(&SyntaxNode) -> Vec<Stmt>`; differential-test it
  equals the directly-built AST over the corpus.
- **3d**: flip `parse_cst` to be the sole parser, project the AST, delete direct
  AST construction, migrate `rewrite.rs` / `show-ast` / compiler.
