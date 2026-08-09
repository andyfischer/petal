//! Tree-sitter syntax highlighting.
//!
//! Pure (no rendering): turns buffer text into per-line [`Span`]s tagged with a
//! [`TokenKind`]. [`EditorView::build_scene`](crate::editor_view::EditorView)
//! caches the result by [`Buffer::revision`](garden_core::Buffer::revision) and
//! paints each span run with the matching `theme.syntax_*` color.
//!
//! Languages are a data-driven [`REGISTRY`] of [`LangDef`] entries — adding one
//! is a single table row plus its `tree-sitter-*` dep. Markdown uses only the
//! *block* grammar (headings, fenced code, list markers, blockquotes); inline
//! emphasis and links — and injected languages generally (HTML↔JS, fenced code)
//! — would need a separate inline/injection parser, which the single-grammar
//! `tree_sitter_highlight::Highlighter` can't drive simply.
//!
//! Span columns are CHAR offsets within a line (matching `Point.col`), not
//! bytes, so multi-byte UTF-8 text highlights in the right place.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

use crate::theme::Theme;
use garden_render::Color;

/// One language described as data: how to build its grammar, its highlight
/// query, and which file names/extensions select it. The registry below is a
/// table of these — adding a language is one new entry, nothing else.
pub struct LangDef {
    /// Display + cache key, e.g. `"Rust"`. Must be unique in the registry.
    pub name: &'static str,
    /// Extensions (without the dot) that select this language.
    pub exts: &'static [&'static str],
    /// Exact file names that select this language (extensionless tools like
    /// `Dockerfile`, `Makefile`). Empty for most languages.
    pub filenames: &'static [&'static str],
    /// Builds the tree-sitter grammar. A per-entry closure so the registry can
    /// absorb crate API differences (`LANGUAGE` const vs `language()` fn).
    pub grammar: fn() -> tree_sitter::Language,
    /// The grammar's highlights query (`HIGHLIGHTS_QUERY`, or a vendored `.scm`).
    pub highlights: &'static str,
    /// Injection query, or `""` when none. Reserved — the single-grammar
    /// highlighter can't drive injections yet (see module docs).
    pub injections: &'static str,
}

/// All bundled languages. Each grammar is a C parser compiled into the binary
/// by its crate's build script. To add one: add the `tree-sitter-*` dep in
/// `Cargo.toml` and append an entry here.
static REGISTRY: &[LangDef] = &[
    LangDef {
        name: "Rust",
        exts: &["rs"],
        filenames: &[],
        grammar: || tree_sitter_rust::LANGUAGE.into(),
        highlights: tree_sitter_rust::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "JSON",
        exts: &["json"],
        filenames: &[],
        grammar: || tree_sitter_json::LANGUAGE.into(),
        highlights: tree_sitter_json::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "TOML",
        exts: &["toml"],
        filenames: &[],
        grammar: || tree_sitter_toml_ng::LANGUAGE.into(),
        highlights: tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        injections: "",
    },
    // Block-level markdown only: the block grammar highlights headings, fenced
    // code, list markers, blockquotes, etc. Inline emphasis/links would need the
    // separate inline parser, which the single-config highlighter can't drive.
    LangDef {
        name: "Markdown",
        exts: &["md", "markdown"],
        filenames: &[],
        grammar: || tree_sitter_md::LANGUAGE.into(),
        highlights: tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        injections: "",
    },
    LangDef {
        name: "Python",
        exts: &["py", "pyi", "pyw"],
        filenames: &[],
        grammar: || tree_sitter_python::LANGUAGE.into(),
        highlights: tree_sitter_python::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "JavaScript",
        exts: &["js", "mjs", "cjs", "jsx"],
        filenames: &[],
        grammar: || tree_sitter_javascript::LANGUAGE.into(),
        highlights: tree_sitter_javascript::HIGHLIGHT_QUERY,
        injections: "",
    },
    LangDef {
        name: "TypeScript",
        exts: &["ts", "mts", "cts"],
        filenames: &[],
        grammar: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "TSX",
        exts: &["tsx"],
        filenames: &[],
        grammar: || tree_sitter_typescript::LANGUAGE_TSX.into(),
        highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Go",
        exts: &["go"],
        filenames: &[],
        grammar: || tree_sitter_go::LANGUAGE.into(),
        highlights: tree_sitter_go::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "C",
        exts: &["c", "h"],
        filenames: &[],
        grammar: || tree_sitter_c::LANGUAGE.into(),
        highlights: tree_sitter_c::HIGHLIGHT_QUERY,
        injections: "",
    },
    LangDef {
        name: "C++",
        exts: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        filenames: &[],
        grammar: || tree_sitter_cpp::LANGUAGE.into(),
        highlights: tree_sitter_cpp::HIGHLIGHT_QUERY,
        injections: "",
    },
    LangDef {
        name: "Java",
        exts: &["java"],
        filenames: &[],
        grammar: || tree_sitter_java::LANGUAGE.into(),
        highlights: tree_sitter_java::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "C#",
        exts: &["cs"],
        filenames: &[],
        grammar: || tree_sitter_c_sharp::LANGUAGE.into(),
        highlights: tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Ruby",
        exts: &["rb"],
        filenames: &["Rakefile", "Gemfile"],
        grammar: || tree_sitter_ruby::LANGUAGE.into(),
        highlights: tree_sitter_ruby::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "PHP",
        exts: &["php"],
        filenames: &[],
        grammar: || tree_sitter_php::LANGUAGE_PHP.into(),
        highlights: tree_sitter_php::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "HTML",
        exts: &["html", "htm"],
        filenames: &[],
        grammar: || tree_sitter_html::LANGUAGE.into(),
        highlights: tree_sitter_html::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "CSS",
        exts: &["css"],
        filenames: &[],
        grammar: || tree_sitter_css::LANGUAGE.into(),
        highlights: tree_sitter_css::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Bash",
        exts: &["sh", "bash", "zsh"],
        filenames: &[".bashrc", ".zshrc", ".bash_profile"],
        grammar: || tree_sitter_bash::LANGUAGE.into(),
        highlights: tree_sitter_bash::HIGHLIGHT_QUERY,
        injections: "",
    },
    LangDef {
        name: "YAML",
        exts: &["yaml", "yml"],
        filenames: &[],
        grammar: || tree_sitter_yaml::LANGUAGE.into(),
        highlights: tree_sitter_yaml::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Lua",
        exts: &["lua"],
        filenames: &[],
        grammar: || tree_sitter_lua::LANGUAGE.into(),
        highlights: tree_sitter_lua::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Scala",
        exts: &["scala", "sc", "sbt"],
        filenames: &[],
        grammar: || tree_sitter_scala::LANGUAGE.into(),
        highlights: tree_sitter_scala::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Haskell",
        exts: &["hs"],
        filenames: &[],
        grammar: || tree_sitter_haskell::LANGUAGE.into(),
        highlights: tree_sitter_haskell::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "SQL",
        exts: &["sql"],
        filenames: &[],
        grammar: || tree_sitter_sequel::LANGUAGE.into(),
        highlights: tree_sitter_sequel::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Zig",
        exts: &["zig"],
        filenames: &[],
        grammar: || tree_sitter_zig::LANGUAGE.into(),
        highlights: tree_sitter_zig::HIGHLIGHTS_QUERY,
        injections: "",
    },
    LangDef {
        name: "Nix",
        exts: &["nix"],
        filenames: &[],
        grammar: || tree_sitter_nix::LANGUAGE.into(),
        highlights: tree_sitter_nix::HIGHLIGHTS_QUERY,
        injections: "",
    },
    // Petal — Garden's own scripting language (init.ptl). Reference grammar:
    // ../editor-support/tree-sitter-petal.
    LangDef {
        name: "Petal",
        exts: &["ptl"],
        filenames: &[],
        grammar: || tree_sitter_petal::LANGUAGE.into(),
        highlights: tree_sitter_petal::HIGHLIGHTS_QUERY,
        injections: "",
    },
];

/// A supported source language, resolved from a file path. A thin `Copy` handle
/// over a registry entry, so it threads through `EditorView` like the old enum.
#[derive(Clone, Copy)]
pub struct Language(&'static LangDef);

impl Language {
    /// The language for `path` (by file name, then extension), or `None`.
    pub fn from_path(path: &Path) -> Option<Language> {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(def) = REGISTRY.iter().find(|d| d.filenames.contains(&name)) {
                return Some(Language(def));
            }
        }
        let ext = path.extension().and_then(|e| e.to_str())?;
        REGISTRY
            .iter()
            .find(|d| d.exts.contains(&ext))
            .map(Language)
    }

    /// The language's registry name (also its cache key).
    pub fn name(self) -> &'static str {
        self.0.name
    }
}

/// A highlight category, mapped to a `theme.syntax_*` color by [`color`](Self::color).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenKind {
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Constant,
    Punctuation,
    Variable,
}

impl TokenKind {
    /// The themed color for this kind.
    pub fn color(self, theme: &Theme) -> Color {
        match self {
            TokenKind::Keyword => theme.syntax_keyword,
            TokenKind::Type => theme.syntax_type,
            TokenKind::Function => theme.syntax_function,
            TokenKind::String => theme.syntax_string,
            TokenKind::Number => theme.syntax_number,
            TokenKind::Comment => theme.syntax_comment,
            TokenKind::Constant => theme.syntax_constant,
            TokenKind::Punctuation => theme.syntax_punctuation,
            TokenKind::Variable => theme.syntax_variable,
        }
    }
}

/// A highlighted run within one line, in char columns: `[start_col, end_col)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start_col: usize,
    pub end_col: usize,
    pub kind: TokenKind,
}

/// Highlight capture names we recognize, in a fixed order. The tree-sitter
/// highlight index is this slice's index; [`kind_for_capture`] maps each name
/// to a [`TokenKind`] by its first dotted segment.
const RECOGNIZED: &[&str] = &[
    "keyword",
    "function",
    "function.method",
    "function.macro",
    "type",
    "type.builtin",
    "constructor",
    "string",
    "string.special",
    "number",
    "comment",
    "constant",
    "constant.builtin",
    "variable",
    "variable.parameter",
    "variable.builtin",
    "property",
    "label",
    "operator",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "tag",
    "attribute",
    // Markdown (block grammar) capture names.
    "text.title",
    "text.literal",
    "text.reference",
    "text.uri",
    "text.emphasis",
    "text.strong",
    "string.escape",
];

/// Map a capture name to a [`TokenKind`] by its first dotted segment, with a
/// few names routed to a kind that better fits their role.
fn kind_for_capture(name: &str) -> Option<TokenKind> {
    // Markdown (block grammar) names that need a specific kind.
    match name {
        "text.title" => return Some(TokenKind::Keyword), // headings
        "text.literal" => return Some(TokenKind::String), // code spans / fences
        "text.reference" | "text.uri" => return Some(TokenKind::Function), // links
        "text.emphasis" | "text.strong" => return Some(TokenKind::Type),
        _ => {}
    }
    let head = name.split('.').next().unwrap_or(name);
    Some(match head {
        "keyword" => TokenKind::Keyword,
        "function" => TokenKind::Function,
        "constructor" => TokenKind::Function,
        "type" => TokenKind::Type,
        "string" => TokenKind::String,
        "number" => TokenKind::Number,
        "comment" => TokenKind::Comment,
        "constant" => TokenKind::Constant,
        "variable" => TokenKind::Variable,
        "property" => TokenKind::Variable,
        "operator" => TokenKind::Punctuation,
        "punctuation" => TokenKind::Punctuation,
        "tag" | "attribute" => TokenKind::Keyword,
        "label" => TokenKind::Keyword,
        _ => return None,
    })
}

/// One configured grammar plus the highlight-index → TokenKind lookup. Building
/// it compiles the grammar's HIGHLIGHTS_QUERY (expensive), so it is built at
/// most once per language and cached.
struct LangConfig {
    config: HighlightConfiguration,
    /// `kinds[i]` is the TokenKind for highlight index `i` (= RECOGNIZED[i]).
    kinds: Vec<Option<TokenKind>>,
}

impl LangConfig {
    fn build(def: &LangDef) -> Option<LangConfig> {
        let language = (def.grammar)();
        let mut config =
            HighlightConfiguration::new(language, def.name, def.highlights, def.injections, "")
                .ok()?;
        config.configure(RECOGNIZED);
        let kinds = RECOGNIZED.iter().map(|n| kind_for_capture(n)).collect();
        Some(LangConfig { config, kinds })
    }
}

/// Caches one compiled grammar per language (keyed by `LangDef::name`) and runs
/// highlighting. Building a config compiles the grammar's query, so it happens
/// at most once per language.
#[derive(Default)]
pub struct Highlighter {
    /// Cache slot per language name; the inner `Option` is whether the grammar
    /// built successfully (so a failed build is not retried every paint).
    configs: HashMap<&'static str, Option<LangConfig>>,
}

impl Highlighter {
    pub fn new() -> Highlighter {
        Highlighter::default()
    }

    /// The cached config for `lang`, building it on first use.
    fn config(&mut self, lang: Language) -> Option<&LangConfig> {
        self.configs
            .entry(lang.name())
            .or_insert_with(|| LangConfig::build(lang.0))
            .as_ref()
    }

    /// Highlight `text`, returning one `Vec<Span>` per line (length = line
    /// count). Lines with no recognized tokens get an empty vec. Returns all
    /// empty vecs for an unsupported language or on a parse error.
    pub fn highlight_lines(&mut self, lang: Language, text: &str) -> Vec<Vec<Span>> {
        let line_count = line_count(text);
        let mut out: Vec<Vec<Span>> = vec![Vec::new(); line_count];

        // A byte-offset → (line, char-col) index, so byte ranges from
        // tree-sitter convert to the char columns the editor uses.
        let index = ByteIndex::new(text);

        let Some(config) = self.config(lang) else {
            return out;
        };

        let mut hl = TsHighlighter::new();
        let events = match hl.highlight(&config.config, text.as_bytes(), None, |_| None) {
            Ok(events) => events,
            Err(_) => return out,
        };

        // Stack of active highlights; the top one wins for any Source range.
        let mut stack: Vec<Option<TokenKind>> = Vec::new();
        for event in events {
            let Ok(event) = event else {
                return out;
            };
            match event {
                HighlightEvent::HighlightStart(h) => {
                    stack.push(config.kinds.get(h.0).copied().flatten());
                }
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let Some(Some(kind)) = stack.last().copied() else {
                        continue;
                    };
                    index.push_spans(start, end, kind, &mut out);
                }
            }
        }
        out
    }
}

/// Number of lines `text` spans the way `Buffer::line_count` counts them: a
/// trailing newline yields a final empty line; empty text is one line.
fn line_count(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// Maps a byte offset in the source to its `(line, char-col)`. Built once per
/// highlight pass by walking the text.
struct ByteIndex {
    /// For each char boundary, its byte offset and (line, col). Sorted by byte.
    /// A trailing entry marks the end of the text.
    points: Vec<(usize, usize, usize)>, // (byte, line, col)
}

impl ByteIndex {
    fn new(text: &str) -> ByteIndex {
        let mut points = Vec::with_capacity(text.len() + 1);
        let mut line = 0;
        let mut col = 0;
        for (byte, ch) in text.char_indices() {
            points.push((byte, line, col));
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        points.push((text.len(), line, col));
        ByteIndex { points }
    }

    /// The `(line, col)` at byte offset `byte` (clamped into range).
    fn locate(&self, byte: usize) -> (usize, usize) {
        let i = self.points.partition_point(|&(b, _, _)| b < byte);
        // `partition_point` gives the first entry with byte >= target; on an
        // exact char boundary that is the point itself, otherwise the byte
        // falls inside a multi-byte char and we round down to its start.
        let i = if self.points.get(i).map(|&(b, _, _)| b) == Some(byte) {
            i
        } else {
            i.saturating_sub(1)
        };
        let (_, line, col) = self.points[i];
        (line, col)
    }

    /// Split the byte range `[start, end)` at line boundaries and append a
    /// [`Span`] of `kind` to each covered line in `out`.
    fn push_spans(&self, start: usize, end: usize, kind: TokenKind, out: &mut [Vec<Span>]) {
        if end <= start {
            return;
        }
        let (start_line, start_col) = self.locate(start);
        // `end` is exclusive; locate the last included char's line.
        let (end_line, end_col) = self.locate(end);

        if start_line == end_line {
            if let Some(line) = out.get_mut(start_line) {
                line.push(Span {
                    start_col,
                    end_col,
                    kind,
                });
            }
            return;
        }
        // First line: from start_col to its end.
        if let Some(line) = out.get_mut(start_line) {
            let len = self.line_char_len(start_line);
            line.push(Span {
                start_col,
                end_col: len,
                kind,
            });
        }
        // Whole middle lines.
        for l in (start_line + 1)..end_line {
            if let Some(line) = out.get_mut(l) {
                let len = self.line_char_len(l);
                line.push(Span {
                    start_col: 0,
                    end_col: len,
                    kind,
                });
            }
        }
        // Last line: from its start to end_col.
        if end_col > 0 {
            if let Some(line) = out.get_mut(end_line) {
                line.push(Span {
                    start_col: 0,
                    end_col,
                    kind,
                });
            }
        }
    }

    /// Char length of line `line` (excluding the newline).
    fn line_char_len(&self, line: usize) -> usize {
        // The last point on a line is the newline char (col == len) or the
        // text-end sentinel. Find the first point on the next line; its col is
        // 0 and its predecessor's col is this line's length.
        let next = self.points.partition_point(|&(_, l, _)| l <= line);
        match self.points.get(next.wrapping_sub(1)) {
            Some(&(_, l, col)) if l == line => col,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolved language name for `path`, or `None`.
    fn lang_name(path: &str) -> Option<&'static str> {
        Language::from_path(Path::new(path)).map(|l| l.name())
    }

    /// The language for an extension, for the highlight tests below.
    fn lang(path: &str) -> Language {
        Language::from_path(Path::new(path)).expect("recognized extension")
    }

    #[test]
    fn from_path_recognizes_extensions() {
        assert_eq!(lang_name("a/b.rs"), Some("Rust"));
        assert_eq!(lang_name("data.json"), Some("JSON"));
        assert_eq!(lang_name("Cargo.toml"), Some("TOML"));
        assert_eq!(lang_name("README.md"), Some("Markdown"));
        assert_eq!(lang_name("notes.markdown"), Some("Markdown"));
        assert_eq!(lang_name("a.xyz"), None);
        assert_eq!(lang_name("noext"), None);
    }

    /// The span covering char columns `[col, col+len)` on `line`, if any.
    fn span_at(lines: &[Vec<Span>], line: usize, col: usize) -> Option<Span> {
        lines
            .get(line)?
            .iter()
            .find(|s| s.start_col <= col && col < s.end_col)
            .copied()
    }

    #[test]
    fn rust_keyword_and_number() {
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("x.rs"), "fn main() {}\nlet x = 42;");

        let kw = span_at(&lines, 0, 0).expect("span on `fn`");
        assert_eq!((kw.start_col, kw.end_col), (0, 2));
        assert_eq!(kw.kind, TokenKind::Keyword);

        // "let x = 42;" — the 42 is at cols 8..10. tree-sitter-rust tags
        // integer literals as `@constant.builtin`, so by the first-segment rule
        // the kind is Constant (which shares the number color in the theme);
        // JSON/TOML below cover a literal `@number` capture → Number.
        let num = span_at(&lines, 1, 8).expect("span on `42`");
        assert_eq!(num.kind, TokenKind::Constant);
        assert_eq!((num.start_col, num.end_col), (8, 10));
    }

    #[test]
    fn json_string_and_number() {
        let mut h = Highlighter::new();
        // {"k": "v", "n": 1}
        //  ^0   ^6        ^16
        let lines = h.highlight_lines(lang("x.json"), r#"{"k": "v", "n": 1}"#);

        let s = span_at(&lines, 0, 7).expect("span inside \"v\"");
        assert_eq!(s.kind, TokenKind::String);

        let n = span_at(&lines, 0, 16).expect("span on `1`");
        assert_eq!(n.kind, TokenKind::Number);
    }

    #[test]
    fn toml_highlights_key_and_value() {
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("x.toml"), "name = \"garden\"\nport = 4048\n");
        // The string value is highlighted.
        let s = span_at(&lines, 0, 8).expect("span inside the string");
        assert_eq!(s.kind, TokenKind::String);
        // The number value is highlighted.
        let n = span_at(&lines, 1, 7).expect("span on the number");
        assert_eq!(n.kind, TokenKind::Number);
    }

    #[test]
    fn rust_block_comment_splits_across_lines() {
        let mut h = Highlighter::new();
        // A block comment spanning two lines: both lines must carry a Comment.
        let lines = h.highlight_lines(lang("x.rs"), "/* a\n b */");
        let c0 = span_at(&lines, 0, 0).expect("comment span on line 0");
        assert_eq!(c0.kind, TokenKind::Comment);
        let c1 = span_at(&lines, 1, 0).expect("comment span on line 1");
        assert_eq!(c1.kind, TokenKind::Comment);
    }

    #[test]
    fn multibyte_columns_are_chars_not_bytes() {
        let mut h = Highlighter::new();
        // The é is one char but two bytes; the keyword after it must land on the
        // right char column.
        let lines = h.highlight_lines(lang("x.rs"), "// é\nfn f() {}");
        let kw = span_at(&lines, 1, 0).expect("span on `fn`");
        assert_eq!((kw.start_col, kw.end_col), (0, 2));
        assert_eq!(kw.kind, TokenKind::Keyword);
    }

    #[test]
    fn markdown_block_grammar_highlights_a_heading() {
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("x.md"), "# Title\n");
        assert!(
            !lines[0].is_empty(),
            "the heading line should carry at least one span"
        );
    }

    #[test]
    fn plain_paragraph_lines_keep_their_line_slots() {
        // No headings/fences — still one Vec per line (length preserved).
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("x.md"), "a\nb\nc");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn registry_extensions_resolve() {
        // A spread across the bundled set, including the ambiguous-header and
        // dual-grammar (TS/TSX, PHP) cases and a filename-only match.
        assert_eq!(lang_name("main.py"), Some("Python"));
        assert_eq!(lang_name("app.js"), Some("JavaScript"));
        assert_eq!(lang_name("app.ts"), Some("TypeScript"));
        assert_eq!(lang_name("app.tsx"), Some("TSX"));
        assert_eq!(lang_name("main.go"), Some("Go"));
        assert_eq!(lang_name("util.c"), Some("C"));
        assert_eq!(lang_name("util.h"), Some("C")); // `.h` → C, not C++
        assert_eq!(lang_name("util.cpp"), Some("C++"));
        assert_eq!(lang_name("App.java"), Some("Java"));
        assert_eq!(lang_name("Program.cs"), Some("C#"));
        assert_eq!(lang_name("page.html"), Some("HTML"));
        assert_eq!(lang_name("style.css"), Some("CSS"));
        assert_eq!(lang_name("run.sh"), Some("Bash"));
        assert_eq!(lang_name("config.yaml"), Some("YAML"));
        assert_eq!(lang_name("query.sql"), Some("SQL"));
        assert_eq!(lang_name("Gemfile"), Some("Ruby")); // filename, no extension
        assert_eq!(lang_name("init.ptl"), Some("Petal"));
    }

    #[test]
    fn petal_keyword_and_string() {
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("init.ptl"), "fn greet(name)\n  \"hi {name}\"\nend");
        let kw = span_at(&lines, 0, 0).expect("span on `fn`");
        assert_eq!((kw.start_col, kw.end_col), (0, 2));
        assert_eq!(kw.kind, TokenKind::Keyword);
        // The string body on line 1 highlights as a string.
        let s = span_at(&lines, 1, 3).expect("span inside the string");
        assert_eq!(s.kind, TokenKind::String);
    }

    /// Every registry grammar must build and compile its highlights query — a
    /// stale grammar/query or wrong const name would otherwise fail silently at
    /// runtime (highlighting just goes blank for that language).
    #[test]
    fn every_registry_grammar_builds() {
        for def in REGISTRY {
            assert!(
                LangConfig::build(def).is_some(),
                "grammar/query failed to build for {}",
                def.name
            );
        }
    }

    #[test]
    fn python_keyword_and_string() {
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("x.py"), "def f():\n    return \"hi\"");
        let kw = span_at(&lines, 0, 0).expect("span on `def`");
        assert_eq!((kw.start_col, kw.end_col), (0, 3));
        assert_eq!(kw.kind, TokenKind::Keyword);
        let s = span_at(&lines, 1, 12).expect("span inside the string");
        assert_eq!(s.kind, TokenKind::String);
    }

    #[test]
    fn go_keyword_highlights() {
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("x.go"), "func main() {}");
        let kw = span_at(&lines, 0, 0).expect("span on `func`");
        assert_eq!((kw.start_col, kw.end_col), (0, 4));
        assert_eq!(kw.kind, TokenKind::Keyword);
    }

    #[test]
    fn c_comment_highlights() {
        let mut h = Highlighter::new();
        let lines = h.highlight_lines(lang("x.c"), "// note\nint x = 1;");
        let c = span_at(&lines, 0, 0).expect("comment span");
        assert_eq!(c.kind, TokenKind::Comment);
    }
}
