//! Ties every downstream keyword list back to the lexer.
//!
//! Petal's keyword set is written out by hand in five independent places: the
//! lexer (authoritative), the LSP completion list, the tree-sitter grammar, the
//! vim syntax file and the tree-sitter highlight queries. Nothing used to
//! compare them and they had all drifted. Each test below re-derives one
//! downstream list from its real source file and asserts set equality against
//! `petal::lexer::KEYWORDS`, so the failure message names the exact missing and
//! extra words.
//!
//! Adding a keyword therefore means touching all five files in one commit —
//! which is the point.

use petal::lexer::{CONTEXTUAL_KEYWORDS, KEYWORDS};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn editor_support(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../editor-support")
        .join(rel)
}

fn read(rel: &str) -> String {
    let path = editor_support(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn set(words: impl IntoIterator<Item = impl Into<String>>) -> BTreeSet<String> {
    words.into_iter().map(Into::into).collect()
}

/// The lexer's hard keywords.
fn lexer_set() -> BTreeSet<String> {
    set(KEYWORDS.iter().copied())
}

/// What a *tooling* list should contain: the hard keywords plus the words the
/// parser treats as keywords even though the lexer emits them as identifiers
/// (`as`). Highlighting and completion care about the surface language, not
/// about which layer recognizes the word.
fn tooling_expected() -> BTreeSet<String> {
    let mut s = lexer_set();
    s.extend(CONTEXTUAL_KEYWORDS.iter().map(|w| w.to_string()));
    s
}

fn is_identifier_shaped(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '?')
}

/// Every extractor asserts a plausible harvest before comparing, so an
/// extractor that silently stops matching fails as a broken extractor instead
/// of quietly passing (or reporting the whole language as "missing").
fn assert_plausible(what: &str, found: &BTreeSet<String>) {
    assert!(
        found.len() >= 20,
        "the {what} extractor found only {} words ({found:?}) — the extractor is broken, \
         not (only) the list",
        found.len()
    );
}

// ---------------------------------------------------------------------------
// The lexer itself
// ---------------------------------------------------------------------------

#[test]
fn lexer_keyword_set_is_sane() {
    let lexer = lexer_set();
    assert_eq!(lexer.len(), KEYWORDS.len(), "KEYWORDS contains a duplicate");
    assert_plausible("lexer", &lexer);
    for kw in &lexer {
        assert!(is_identifier_shaped(kw), "`{kw}` is not identifier-shaped");
    }
    for ctx in CONTEXTUAL_KEYWORDS {
        assert!(
            !lexer.contains(*ctx),
            "`{ctx}` is in both KEYWORDS and CONTEXTUAL_KEYWORDS"
        );
    }
}

// ---------------------------------------------------------------------------
// LSP completion — rust/src/lsp/document.rs
// ---------------------------------------------------------------------------

#[test]
fn lsp_completion_keywords_match_lexer() {
    let lsp = set(petal::lsp::document::KEYWORDS.iter().copied());
    assert_plausible("LSP", &lsp);
    assert_eq!(
        lsp,
        tooling_expected(),
        "rust/src/lsp/document.rs KEYWORDS has drifted from the lexer"
    );
}

// ---------------------------------------------------------------------------
// tree-sitter grammar — editor-support/tree-sitter-petal/src/grammar.json
// ---------------------------------------------------------------------------

const GRAMMAR_JSON: &str = "tree-sitter-petal/src/grammar.json";
const GRAMMAR_JS: &str = "tree-sitter-petal/grammar.js";

/// Anonymous tokens in a generated tree-sitter grammar are
/// `{"type": "STRING", "value": "..."}` nodes. Field names use
/// `{"type": "FIELD", ...}` and rule references `{"type": "SYMBOL", ...}`, so
/// restricting to `STRING` plus an identifier shape yields exactly the
/// keyword literals (operators and punctuation are filtered out by the shape).
fn collect_string_tokens(node: &Value, out: &mut BTreeSet<String>) {
    match node {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("STRING")
                && let Some(v) = map.get("value").and_then(Value::as_str)
                && is_identifier_shaped(v)
            {
                out.insert(v.to_string());
            }
            for v in map.values() {
                collect_string_tokens(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_string_tokens(v, out);
            }
        }
        _ => {}
    }
}

fn tree_sitter_set() -> BTreeSet<String> {
    let json: Value = serde_json::from_str(&read(GRAMMAR_JSON)).expect("grammar.json is not JSON");
    let rules = json.get("rules").expect("grammar.json has no `rules`");
    let mut out = BTreeSet::new();
    collect_string_tokens(rules, &mut out);
    out
}

#[test]
fn tree_sitter_grammar_keywords_match_lexer() {
    let ts = tree_sitter_set();
    assert_plausible("tree-sitter grammar", &ts);
    assert_eq!(
        ts,
        tooling_expected(),
        "{GRAMMAR_JSON} has drifted from the lexer (regenerate with `npm run generate` \
         after editing grammar.js)"
    );
}

/// The generated `src/grammar.json` is checked in, so it can lag `grammar.js`.
/// Containment only: every keyword the generated grammar knows about must
/// still appear as a quoted literal in the source grammar. (The reverse is too
/// noisy — `field('name', …)` arguments are identifier-shaped quoted strings
/// too.)
#[test]
fn generated_grammar_is_not_stale() {
    let js = read(GRAMMAR_JS);
    let ts = tree_sitter_set();
    assert_plausible("tree-sitter grammar", &ts);
    let missing: Vec<&String> = ts
        .iter()
        .filter(|kw| !js.contains(&format!("'{kw}'")) && !js.contains(&format!("\"{kw}\"")))
        .collect();
    assert!(
        missing.is_empty(),
        "{GRAMMAR_JSON} mentions {missing:?}, which no longer appear in {GRAMMAR_JS} — \
         the generated parser is stale, re-run `npm run generate`"
    );
}

// ---------------------------------------------------------------------------
// vim syntax — editor-support/vim/syntax/petal.vim
// ---------------------------------------------------------------------------

const VIM_SYNTAX: &str = "vim/syntax/petal.vim";

/// `syn keyword <group> word word …`, minus the comment-tag group and vim's
/// own argument words.
fn vim_set() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in read(VIM_SYNTAX).lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("syn keyword ")
            .or_else(|| line.strip_prefix("syntax keyword "))
        else {
            continue;
        };
        let mut words = rest.split_whitespace();
        // The group name; `petalTodo` holds TODO/FIXME comment tags, not
        // language keywords.
        match words.next() {
            Some("petalTodo") | None => continue,
            Some(_) => {}
        }
        for word in words {
            // vim `:syn keyword` arguments — `contained`, `nextgroup=…`, etc.
            if word.contains('=')
                || matches!(
                    word,
                    "contained"
                        | "containedin"
                        | "transparent"
                        | "skipwhite"
                        | "skipnl"
                        | "skipempty"
                        | "conceal"
                        | "display"
                        | "fold"
                        | "extend"
                )
            {
                continue;
            }
            out.insert(word.to_string());
        }
    }
    out
}

#[test]
fn vim_syntax_keywords_match_lexer() {
    let vim = vim_set();
    assert_plausible("vim syntax", &vim);
    assert_eq!(
        vim,
        tooling_expected(),
        "{VIM_SYNTAX} has drifted from the lexer"
    );
}

// ---------------------------------------------------------------------------
// tree-sitter highlights — editor-support/tree-sitter-petal/queries/highlights.scm
// ---------------------------------------------------------------------------

const HIGHLIGHTS: &str = "tree-sitter-petal/queries/highlights.scm";

/// `true` / `false` / `nil` are keywords to the lexer but are highlighted as
/// `(boolean)` / `(nil)` @constant.builtin instead of @keyword — see the
/// "Literals" section of highlights.scm (around lines 65-66). That is the
/// conventional capture for them, so they are legitimately absent from the
/// @keyword block.
const HIGHLIGHTED_ELSEWHERE: &[&str] = &["true", "false", "nil"];

/// The `[ "kw" … ] @keyword` block, plus node captures of the form
/// `(<name>_statement) @keyword` (used for `break` / `continue`, which are the
/// sole content of their own node).
fn highlights_set() -> BTreeSet<String> {
    let src = read(HIGHLIGHTS);
    let mut out = BTreeSet::new();

    // Bracketed alternation lists captured as @keyword.
    let mut rest = src.as_str();
    while let Some(close) = rest.find("] @keyword") {
        let open = rest[..close]
            .rfind('[')
            .expect("`] @keyword` with no opening `[` in highlights.scm");
        let block = &rest[open + 1..close];
        let parts: Vec<&str> = block.split('"').collect();
        for i in (1..parts.len()).step_by(2) {
            out.insert(parts[i].to_string());
        }
        rest = &rest[close + 1..];
    }

    // `(break_statement) @keyword` → `break`.
    let mut rest = src.as_str();
    while let Some(at) = rest.find(") @keyword") {
        let open = match rest[..at].rfind('(') {
            Some(i) => i,
            None => break,
        };
        let name = rest[open + 1..at].trim();
        if let Some(kw) = name.strip_suffix("_statement")
            && is_identifier_shaped(kw)
        {
            out.insert(kw.to_string());
        }
        rest = &rest[at + 1..];
    }

    out
}

#[test]
fn highlight_queries_match_lexer() {
    let hl = highlights_set();
    assert_plausible("highlights.scm", &hl);
    let mut expected = tooling_expected();
    for w in HIGHLIGHTED_ELSEWHERE {
        expected.remove(*w);
    }
    assert_eq!(hl, expected, "{HIGHLIGHTS} has drifted from the lexer");
}
