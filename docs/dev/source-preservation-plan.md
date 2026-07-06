# Source preservation plan — comments & original-source fidelity

Status: **in progress** (2026-07-05). End goal committed: **full lossless
representation (Option C — a concrete syntax tree)**. Step 1 (a lossless
lexer/trivia foundation) is **done and proven**; see "Progress" below.

## Motivation

A core Petal goal is that **a program can be changed programmatically at
runtime**, and when it is, *the source should be preserved as much as possible —
including comments and layout*. An embedder (e.g. the garden editor) treats a
`.ptl` file as a live document: a tool rewrites one call or inserts one
statement, then writes the file back. Today that round-trip silently deletes
comments in any region that gets regenerated, because our parsed representation
throws comments away.

## Where we are today

The pipeline is **lossy at the lexer**:

- `lexer.rs::tokenize` calls `skip_line_comment` and **discards** `//` comments
  entirely — they never become tokens, never reach the AST.
- Non-newline whitespace is also skipped (`skip_whitespace_no_newline`).
  Newlines *are* emitted as `Token::Newline`, so line structure survives to the
  token stream but not to the AST.
- The AST (`ast.rs`) carries a `SourceSpan` on every `Expr` / `Stmt` (byte...
  actually **char** offsets — the lexer indexes source as `Vec<char>`), but no
  comment or whitespace nodes.

Programmatic editing today lives in `rewrite.rs`, which sidesteps the lossiness
with **surgical span splicing**: parse only to locate a node's span, then
replace exactly those characters in the *original text*, copying every other
byte verbatim. Consequences:

- ✅ Comments and layout **outside** an edited span already survive.
- ❌ Comments **inside** a replaced/regenerated span are lost (there's no way to
  recover them — they were never captured).
- ❌ Any edit that must *reprint* a node from the AST (insert a new statement,
  reorder, or fix indentation across the whole file) loses all comments in the
  affected region.
- ❌ There is no representation an edit can consult to answer "what comment is
  attached to this statement?" so comment-aware moves are impossible.

Also relevant: the parser (`parse.rs`) consumes tokens by **positional index**
into `self.tokens` / `self.token_spans` and pattern-matches specific token kinds
in dozens of places (`matches!(self.peek(), Token::Newline | Token::End | …)`).
This constrains how we can introduce comment tokens (see Non-goals / risks).

## Design space

Three broadly different targets, in increasing ambition:

### Option A — Comment side-table (trivia captured, AST unchanged)

Capture comments in the **lexer** into a parallel `Vec<Comment>` (span + text +
kind), *without* putting them in the parser's token stream. The AST stays as-is.
A post-parse **attachment pass** associates each comment with the nearest AST
node by span adjacency (leading vs. trailing), producing a
`HashMap<NodeKey, Trivia>` side-table carried alongside the AST.

- Effort: **low–medium**. Lexer change is ~10 lines; parser untouched.
- Enables: comment-aware span-splicing (an edit can look up and re-emit the
  comments belonging to the node it replaces); a printer that reattaches
  leading/trailing comments.
- Limits: attachment is heuristic; comments in *interior* positions (between two
  args of a call, inside a record literal) attach coarsely. "Other details"
  (exact whitespace, blank-line counts, quote style) are still not captured.

### Option B — Trivia-attached AST (comments as first-class node fields)

Give AST nodes explicit `leading: Vec<Trivia>` / `trailing: Vec<Trivia>` fields
(Trivia = comment | blank-line | whitespace run). The parser attaches trivia as
it consumes tokens. This is the "Roslyn-lite" model: the semantic tree *is* the
lossless-ish tree.

- Effort: **medium–high**. Every AST node grows fields; the parser must thread
  trivia through every construct; `Serialize`/IR-serialize and every AST
  consumer (compiler, desugar, extract, show-ast) must tolerate the new fields.
- Enables: full leading/trailing comment preservation across inserts/reorders; a
  faithful pretty-printer.
- Limits: interior trivia still awkward; the AST is the "abstract" tree so it
  drops tokens like parens/commas — perfect byte round-trip is not guaranteed.

### Option C — Lossless concrete syntax tree (CST / red-green tree)

A full lossless tree à la rust-analyzer's `rowan` or Roslyn: every token
(keywords, punctuation, whitespace, comments) is a node; the AST becomes a typed
*view* over the CST. Byte-perfect round-trip by construction; edits are tree
splices that inherently carry trivia.

- Effort: **high**. New syntax-tree crate/module, a second parse target (or
  reworking the parser to build the CST and derive the AST from it), and
  migrating consumers to the typed-view API.
- Enables: everything — perfect fidelity, incremental reparse, robust structural
  editing, and it's the ideal substrate for the `petal lint` re-indenter.
- Cost: largest change to the codebase; risk of a long-lived half-migrated
  parser.

## Direction: build toward Option C (full lossless CST)

The end goal is committed: a **fully lossless** representation, so *every* detail
of the original source round-trips (Option C). We get there in load-bearing
steps, each independently useful, rather than one big-bang parser rewrite. The
foundation — a lexer that loses nothing — is done; the remaining steps grow a
concrete tree on top of it and migrate the AST to a typed view over it.

## Progress

### ✅ Step 1 — Lossless lexer + trivia (`rust/src/trivia.rs`)

Done 2026-07-05. The lexer no longer discards anything positionally:

- `crate::trivia::reconstruct(source, &lexer.token_spans)` rebuilds the original
  source **byte-for-byte** from token spans alone. It cursor-walks the tokens,
  emitting inter-token gaps (trivia) verbatim and each token's own source text,
  and is robust to the lexer's two span irregularities — zero-width tokens
  (collapsed JSX text, empty interpolation parts) and forward-overlapping spans
  (`InterpStart` covers the opening quote + first literal run).
- `crate::trivia::leading_trivia(...)` classifies each gap into typed `Trivia`
  runs (`Whitespace` / `LineComment` / `Other`) attached to the following token.
- `Lexer::tokenize` now populates `Lexer::token_leading_trivia` (parallel to
  `tokens`). The parser consumes `tokens` unchanged — zero parser churn, as
  planned.

**Invariant, tested:** `reconstruct(src, spans) == src`. Proven not just on
handcrafted snippets (core syntax, strings, interpolation, JSX, colors, raw
strings, comment-only and whitespace-only files) but over **the entire repo
`.ptl` corpus** (100+ programs) — `round_trips_entire_repo_corpus`. A guard test
(`no_other_trivia_in_core_language`) asserts core-language gaps only ever produce
whitespace/comment trivia, catching span regressions early.

`TriviaKind::Other` currently appears only for interpolation/JSX delimiter chars
whose tokens have loose spans; reconstruction is unaffected, but these mark the
spans a later CST step should tighten (see Step 2).

### ▶ Step 2 — Tighten token spans so they tile the source exactly (next)

Prerequisite for a clean CST: make every token's span cover exactly its own
source text, eliminating `Other` trivia. The offenders (identified in the lexer
survey):
- `read_string` emits `InterpStart` with the *string's opening-quote* position
  as its start, so its span overlaps the first literal part; the string-part
  tokens are then zero-width. Give `InterpStart` the `{`'s own span (or the
  opening quote alone) and give each literal part its true `[start,end)`.
- `flush_jsx_text` emits `JsxText` with a collapsed value and a zero-width span
  at the *end* of the consumed text; stamp it with the real `[start,end)` span
  of the raw text it consumed.
- JSX `{`/`}` holes: ensure the delimiter chars belong to a token span.

Keep `reconstruct`'s corpus round-trip green throughout (it already tolerates the
loose spans, so this is a safe, test-guarded refactor), and extend
`no_other_trivia_in_core_language` to assert **zero** `Other` trivia anywhere,
including interpolation/JSX, once done.

### ▢ Step 3 — Concrete syntax tree

Build a green/red lossless tree (rowan/Roslyn style): every token, including
trivia, is a node; the typed AST becomes a view over it. The parser builds the
CST (or the CST is derived and the AST projected from it). Migrate consumers
(compiler, desugar, `rewrite.rs`, `show-ast`) to the typed-view API. This is
where inserts/reorders carry comments structurally and whole-file reprint
becomes faithful — unblocking the `petal lint` re-indenter
([linter-plan.md](linter-plan.md)).

---

## Original staging notes (superseded by the committed Option-C direction)

The following captured the incremental A→B path considered before we committed
to full losslessness; kept for context.

Do the **minimum that makes runtime source edits comment-safe first**, then grow
toward structural fidelity only if a concrete need appears.

### Stage 1 — Capture comments in the lexer (Option A core) — *do first*

1. Add `#[derive] Comment { span: SourceSpan, text: String, kind: CommentKind }`
   where `CommentKind ∈ { OwnLine, Trailing }` (Trailing = non-whitespace
   precedes it on the line — decide via the lexer's column tracking / previous
   token on the same line).
2. In `tokenize`, where we currently `skip_line_comment` and `continue`, first
   record the comment text + span into a new `pub comments: Vec<Comment>` on the
   `Lexer`. Do **not** push a token — the parser's index-based stream is
   unchanged, so this is a zero-risk change to parsing.
3. Thread `comments` out alongside `tokens` / `token_spans` (Parser can take it
   and stash it, or callers read it off the lexer). Extend `rewrite::parse_ast`
   to return comments too.
4. Extend block-comment support here too if we ever add `/* … */` (currently
   only `//` exists). Capture the concept now even if only `//` is implemented.

Deliverable: nothing yet reattaches them, but comments are **no longer thrown
away**. Ship with a unit test asserting a file's comments survive lexing with
correct spans/kinds.

### Stage 2 — Attachment + comment-aware splice

1. Post-parse pass: assign each `Comment` to an AST node.
   - Own-line comment immediately above a statement → that statement's *leading*.
   - Trailing comment on a statement's last line → that statement's *trailing*.
   - Fall back to "floating" (attached to the enclosing block at an index) when
     it belongs to no single node.
2. Upgrade `rewrite.rs`: when a splice replaces a node's span, look up that
   node's leading/trailing comments and re-emit them around the replacement, so
   regenerated code keeps its comments. Add the failing round-trip test that
   motivated this (replace a call that has an inline comment → comment survives).

Deliverable: the runtime-edit round-trip preserves comments even when a node is
regenerated. This is the concrete goal from the motivation.

### Stage 3 (optional, later) — Trivia-attached AST (Option B) or CST (Option C)

Only if we need whole-file reprint fidelity (e.g. the `petal lint` re-indenter
or structural inserts/reorders that carry comments). Revisit then; Stage 1's
`Comment` type and spans are forward-compatible with both. See
[linter-plan.md](linter-plan.md) — the linter can proceed on Stage 1+2 alone by
using whitespace-only re-indentation (never reprinting from the AST).

## "Other details of the original source" — inventory

What a fully faithful representation would need to capture, and where each lands
in the staging above:

| Detail | Captured today? | Stage |
|---|---|---|
| `//` comments | ❌ (discarded) | Stage 1 |
| Comment own-line vs trailing | ❌ | Stage 1 (`kind`) |
| Newlines / line structure | tokens only, not AST | Stage 2 (attach) |
| Blank-line runs | ❌ | Stage 2/3 (Trivia) |
| Indentation / leading whitespace | ❌ | Not needed (lint re-derives) |
| Trailing whitespace | ❌ | N/A (normalize away) |
| Optional commas (comma vs juxtaposition) | ❌ (both parse same) | Stage 3 / CST |
| Paren grouping `(a + b)` vs `a + b` | ❌ (AST drops parens) | Stage 3 / CST |
| String quote style, raw `"""` | partial | Stage 3 / CST |
| Numeric literal spelling (`#f80` vs `#ff8800`) | ❌ | Stage 3 / CST |

Anything marked "Stage 3 / CST" is genuinely only recoverable with a lossless
concrete tree (Option C) — flag that explicitly if/when a use case demands it,
rather than half-solving it in the AST.

## Risks / non-goals

- **Do not** inject `Comment` tokens into the parser's token stream in Stage 1.
  The parser indexes tokens positionally and `matches!`-es specific kinds in
  many places; a stray token kind would require touching every one of those
  sites and is a large, bug-prone change for no Stage-1 benefit. Keep comments
  in a side channel until/unless we commit to a CST.
- IR serialization and `show-ast --json` currently serialize the AST; adding
  node fields (Option B) means versioning that output. Stage 1/2 avoid this by
  keeping comments out of the AST proper.
- Attachment heuristics will never be perfect for interior comments; document
  the rule and accept coarse attachment until Stage 3.

## First concrete step

Implement Stage 1 (lexer captures comments into a side `Vec<Comment>` with spans
and kind), with tests, as a standalone, low-risk PR. Everything else builds on
it.
