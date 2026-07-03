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
//! end
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
    fn test_parses_import_forms() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();
        let code = "import ui\nimport ui: button, clicked\nimport ui as u\n";
        let tree = parser.parse(code, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "all three import forms must parse without error"
        );
    }
}
