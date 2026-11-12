//! This crate provides Petal language support for the [tree-sitter][] parsing
//! library.
//!
//! Typically, you will use the [LANGUAGE][] constant to add this language to a
//! tree-sitter [Parser][], and then use the parser to parse some code:
//!
//! ```
//! let code = r#"
//! fn square(n)
//!   n * n
//! "#;
//! let mut parser = tree_sitter::Parser::new();
//! let language = tree_sitter_petal::LANGUAGE;
//! parser
//!     .set_language(&language.into())
//!     .expect("Error loading Petal parser");
//! let tree = parser.parse(code, None).unwrap();
//! assert!(!tree.root_node().has_error());
//! ```
//!
//! [Parser]: https://docs.rs/tree-sitter/*/tree_sitter/struct.Parser.html
//! [tree-sitter]: https://tree-sitter.github.io/

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_petal() -> *const ();
}

/// The tree-sitter [`LanguageFn`][LanguageFn] for this grammar.
///
/// [LanguageFn]: https://docs.rs/tree-sitter-language/*/tree_sitter_language/struct.LanguageFn.html
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_petal) };

/// The content of the [`node-types.json`][] file for this grammar.
///
/// [`node-types.json`]: https://tree-sitter.github.io/tree-sitter/using-parsers#static-node-types
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

/// The syntax highlighting query for this language.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

/// The indentation query for this language, in the nvim-treesitter capture
/// convention (`@indent.begin`, `@indent.branch`, `@indent.end`).
pub const INDENTS_QUERY: &str = include_str!("../../queries/indents.scm");

#[cfg(test)]
mod tests {
    #[test]
    fn test_can_load_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("Error loading Petal parser");
    }

    #[test]
    fn test_highlights_query_compiles() {
        let language = tree_sitter::Language::from(super::LANGUAGE);
        tree_sitter::Query::new(&language, super::HIGHLIGHTS_QUERY)
            .expect("highlights.scm must compile against the grammar");
    }

    #[test]
    fn test_indents_query_compiles() {
        let language = tree_sitter::Language::from(super::LANGUAGE);
        tree_sitter::Query::new(&language, super::INDENTS_QUERY)
            .expect("indents.scm must compile against the grammar");
    }

    /// Blocks close by indentation and `end` is optional
    /// (docs/tasks/pythonic-syntax.md). This goes through the external
    /// scanner (`src/scanner.c`), so it also proves the build compiles and
    /// links it: without the scanner the parser does not load at all.
    #[test]
    fn test_parses_blocks_with_and_without_end() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();
        let endless = concat!(
            "# a comment\n",
            "fn clamp_all(xs, lo, hi)\n",
            "  for x in xs do\n",
            "    if x < lo then lo\n",
            "    else if x > hi then hi\n",
            "    else x\n",
            "let tint = :f80\n",
            "defer(fn()\n",
            "  draw(tint)\n",
            ")\n",
        );
        let legacy = concat!(
            "// a comment\n",
            "fn clamp_all(xs, lo, hi)\n",
            "  for x in xs do\n",
            "    if x < lo then lo\n",
            "    elsif x > hi then hi\n",
            "    else x\n",
            "    end\n",
            "  end\n",
            "end\n",
            "let tint = :f80\n",
            "defer(fn()\n",
            "  draw(tint)\n",
            "end)\n",
        );
        let a = parser.parse(endless, None).unwrap();
        let b = parser.parse(legacy, None).unwrap();
        assert!(!a.root_node().has_error(), "{}", a.root_node().to_sexp());
        assert!(!b.root_node().has_error(), "{}", b.root_node().to_sexp());
        // `end` is an anonymous token, so the two spellings are one tree.
        assert_eq!(a.root_node().to_sexp(), b.root_node().to_sexp());
    }

    /// The scanner keeps an indent stack, which tree-sitter saves and
    /// restores around every token for incremental parsing. A reparse after
    /// an edit must give the tree a fresh parse gives.
    #[test]
    fn test_incremental_reparse_matches_a_fresh_parse() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();
        let before = "fn f(xs)\n  for x in xs do\n    if x then\n      a()\n    b()\n  c()\nlet y = 1\n";
        // Indent `b()` into the `if`.
        let at = before.find("    b()").unwrap();
        let after = format!("{}  {}", &before[..at], &before[at..]);
        let mut tree = parser.parse(before, None).unwrap();
        let row = before[..at].matches('\n').count();
        tree.edit(&tree_sitter::InputEdit {
            start_byte: at,
            old_end_byte: at,
            new_end_byte: at + 2,
            start_position: tree_sitter::Point::new(row, 0),
            old_end_position: tree_sitter::Point::new(row, 0),
            new_end_position: tree_sitter::Point::new(row, 2),
        });
        let reparsed = parser.parse(&after, Some(&tree)).unwrap();
        let fresh = parser.parse(&after, None).unwrap();
        assert!(!fresh.root_node().has_error());
        assert_eq!(reparsed.root_node().to_sexp(), fresh.root_node().to_sexp());
        assert_ne!(
            fresh.root_node().to_sexp(),
            parser.parse(before, None).unwrap().root_node().to_sexp(),
            "the edit moves `b()` into the `if`, so the tree must change"
        );
    }

    #[test]
    fn test_parses_import_forms() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();
        let code = concat!(
            "import ui\n",
            "import ui: button, clicked\n",
            "import ui as u\n",
            // Package paths and the re-export form (docs/module-system.md).
            "import bloom/menu\n",
            "import bloom/menu: open, close\n",
            "pub import bloom/motion\n",
            // Relative paths.
            "import ./level\n",
            "import ../shared/rng as r\n",
            "pub import ../../kit/menu: open, close\n",
        );
        let tree = parser.parse(code, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "every import form must parse without error"
        );
    }

    /// `pub` prefixes a declaration, and `export` is its deprecated
    /// spelling. Every form the real parser accepts after either word
    /// (rust/src/parse.rs `parse_export`) must parse here too, or the keyword
    /// loses its highlight along with the rest of the line.
    #[test]
    fn test_parses_every_pub_form() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();
        for modifier in ["pub", "export"] {
            for decl in [
                "fn f() 1 end",
                "let a = 1",
                "var b = 2",
                "config let c = 3",
                "state d = 4",
                "enum E A, B end",
                "class R x: num end",
                "import bloom/menu",
                "import ./menu: open",
            ] {
                let code = format!("{modifier} {decl}");
                let tree = parser.parse(&code, None).unwrap();
                assert!(!tree.root_node().has_error(), "failed to parse: {code}");
            }
        }
    }
}
