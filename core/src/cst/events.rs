//! The parser event stream and its projection into a green tree. The parser
//! records an abstract [`Event`] shape as it recognizes constructs; [`build_tree`]
//! later materializes that into a lossless green tree with trivia interleaved.

use std::rc::Rc;

use crate::lexer::{Lexer, Token};
use crate::source_map::SourceSpan;
use crate::trivia::{Trivia, TriviaKind};

use super::SyntaxKind;
use super::green::{GreenNode, GreenNodeBuilder, GreenToken};

/// A parse event: the abstract shape of the tree, emitted by the parser as it
/// recognizes constructs and later materialized by [`build_tree`]. Keeping
/// events separate from tree construction buys two things the parser needs:
/// checkpoints that retroactively *wrap* already-emitted nodes (for
/// left-associative operators — see [`EventBuilder::wrap`]), and a single place
/// to decide how trivia attaches to the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Open an interior node of this kind. Children follow until the matching
    /// [`Event::Close`].
    Open(SyntaxKind),
    /// Close the most recently opened node.
    Close,
    /// Consume the next significant token from the token stream (trivia is
    /// attached automatically by [`build_tree`]).
    Token,
}

/// A saved position in an [`EventBuilder`]'s event list, used to wrap the nodes
/// emitted after it. Only valid for the builder that produced it, and only
/// until an earlier `wrap` shifts it (the standard recursive-descent usage —
/// each parse function wraps at its own checkpoint after its callees have
/// returned — never does this).
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint(usize);

/// Records the parser's [`Event`] stream. Interior nodes are opened with
/// [`EventBuilder::open`] and closed with [`EventBuilder::close`]; a token is
/// marked consumed with [`EventBuilder::token`]. For left-recursive constructs,
/// take a [`EventBuilder::checkpoint`] *before* parsing the left operand and
/// call [`EventBuilder::wrap`] *after* the right operand to enclose the whole
/// range in a node — nesting outward on each loop iteration, which yields
/// left-associative trees.
#[derive(Debug, Default, Clone)]
pub struct EventBuilder {
    events: Vec<Event>,
}

impl EventBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// A checkpoint at the current position, for a later [`EventBuilder::wrap`].
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.events.len())
    }

    /// Open a node; subsequent events are its children until [`Self::close`].
    pub fn open(&mut self, kind: SyntaxKind) {
        self.events.push(Event::Open(kind));
    }

    /// Close the most recently opened node.
    pub fn close(&mut self) {
        self.events.push(Event::Close);
    }

    /// Mark the next significant token as consumed.
    pub fn token(&mut self) {
        self.events.push(Event::Token);
    }

    /// Retroactively wrap everything emitted since `cp` in a node of `kind`.
    /// Repeated `wrap`s at the same checkpoint nest outward, so a `a + b + c`
    /// loop builds the left-associative `((a + b) + c)`.
    pub fn wrap(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        self.events.insert(cp.0, Event::Open(kind));
        self.events.push(Event::Close);
    }

    /// The recorded events.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Consume the builder, returning the recorded events.
    pub fn into_events(self) -> Vec<Event> {
        self.events
    }
}

/// Materialize a structured green tree from a parser's [`Event`] stream and the
/// lexer output. Every `Event::Token` consumes the next significant token (and
/// first emits that token's leading trivia as leaves); `Open`/`Close` frame
/// interior nodes. The whole thing is wrapped in a [`SyntaxKind::Root`], and any
/// tokens the parser did not consume (the trailing `Eof` and its leading
/// trivia) are flushed as final `Root` children — so the tree always covers the
/// entire source and round-trips exactly.
///
/// `tokens`, `spans`, and `leading_trivia` are the parallel arrays the lexer
/// produces (see [`crate::lexer::Lexer`]); `source` is the original text.
pub fn build_tree(
    events: &[Event],
    tokens: &[Token],
    spans: &[SourceSpan],
    leading_trivia: &[Vec<Trivia>],
    source: &str,
) -> Rc<GreenNode> {
    let chars: Vec<char> = source.chars().collect();
    let mut builder = GreenNodeBuilder::new();
    builder.start_node(SyntaxKind::Root);

    // A monotonic cursor over the source, mirroring `trivia::reconstruct`: each
    // token contributes its leading-trivia gap plus its own clamped text. Step-2
    // tiling spans mean this covers every character exactly once.
    let mut cursor = 0usize;
    let mut k = 0usize;
    for event in events {
        match event {
            Event::Open(kind) => builder.start_node(*kind),
            Event::Close => builder.finish_node(),
            Event::Token => {
                emit_token(
                    &mut builder,
                    &mut k,
                    &mut cursor,
                    &chars,
                    tokens,
                    spans,
                    leading_trivia,
                );
            }
        }
    }
    // Flush tokens the parser never consumed (trailing Eof + its trivia) into
    // the root, so nothing is lost even though no event references them.
    while k < tokens.len() {
        emit_token(
            &mut builder,
            &mut k,
            &mut cursor,
            &chars,
            tokens,
            spans,
            leading_trivia,
        );
    }
    // Defensive: any characters past the final token (only reachable if the
    // stream somehow lacks an Eof whose trivia covers the tail).
    if cursor < chars.len() {
        let text: String = chars[cursor..].iter().collect();
        builder.token(GreenToken::Trivia {
            kind: TriviaKind::Whitespace,
            text,
        });
    }

    builder.finish_node(); // Root
    builder.finish()
}

/// Emit token `*k`'s leading trivia and its own clamped source text, advancing
/// `*k` and the monotonic `*cursor`.
#[allow(clippy::too_many_arguments)]
fn emit_token(
    builder: &mut GreenNodeBuilder,
    k: &mut usize,
    cursor: &mut usize,
    chars: &[char],
    tokens: &[Token],
    spans: &[SourceSpan],
    leading_trivia: &[Vec<Trivia>],
) {
    for tr in &leading_trivia[*k] {
        builder.token(GreenToken::Trivia {
            kind: tr.kind.clone(),
            text: tr.text.clone(),
        });
    }
    let span = spans[*k];
    let len = chars.len();
    let start = (span.start.offset as usize).min(len);
    let end = (span.end.offset as usize).min(len);
    *cursor = (*cursor).max(start);
    let text: String = if end > *cursor {
        let t = chars[*cursor..end].iter().collect();
        *cursor = end;
        t
    } else {
        // Zero-width token (Eof, empty interpolation parts): still emit a leaf so
        // the tree has one token leaf per lexer token.
        String::new()
    };
    builder.token(GreenToken::Token {
        token: tokens[*k].clone(),
        text,
    });
    *k += 1;
}

/// Build a flat, lossless green tree from `source`: a single [`SyntaxKind::Root`]
/// node whose children are every trivia run and significant token, in source
/// order. This is [`build_tree`] with an empty event stream — no grammar
/// structure yet, that arrives when the parser drives [`EventBuilder`] — and it
/// round-trips exactly: `build_lossless(src).text() == src`.
///
/// Returns the lexer's error if `source` does not tokenize.
pub fn build_lossless(source: &str) -> Result<Rc<GreenNode>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    Ok(build_tree(
        &[],
        &lexer.tokens,
        &lexer.token_spans,
        &lexer.token_leading_trivia,
        source,
    ))
}
