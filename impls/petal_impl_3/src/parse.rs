//! Parser - converts source code to Program

use std::collections::HashMap;
use logos::{Lexer, Logos, SpannedIter};

use crate::{
    program::{Program, ProgramKey, TermId, ConstantValue, TermOp},
    Error,
};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),

    #[error("Unexpected end of input")]
    UnexpectedEof,

    #[error("Invalid syntax: {0}")]
    InvalidSyntax(String),

    #[error("Unknown identifier: {0}")]
    UnknownIdentifier(String),
}

#[derive(Logos, Debug, PartialEq)]
#[logos(skip r"[ \t\n\f]+")]
enum Token {
    // Literals
    #[regex(r"[0-9]+", |lex| lex.slice().parse())]
    Int(i64),

    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse())]
    Float(f64),

    #[regex(r#"\"([^\"]|\\.)*\""#, |lex| lex.slice().trim_matches('"').to_string())]
    String(String),

    #[token("true")]
    True,

    #[token("false")]
    False,

    #[token("nil")]
    Nil,

    // Keywords
    #[token("let")]
    Let,

    #[token("state")]
    State,

    #[token("fn")]
    Fn,

    #[token("if")]
    If,

    #[token("else")]
    Else,

    #[token("while")]
    While,

    #[token("for")]
    For,

    #[token("in")]
    In,

    #[token("return")]
    Return,

    // Operators
    #[token("@")]
    At,

    #[token("+")]
    Plus,

    #[token("-")]
    Minus,

    #[token("*")]
    Star,

    #[token("/")]
    Slash,

    #[token("==")]
    EqEq,

    #[token("!=")]
    NotEq,

    #[token("<")]
    Lt,

    #[token("<=")]
    LtEq,

    #[token(">")]
    Gt,

    #[token(">=")]
    GtEq,

    #[token("=")]
    Eq,

    // Punctuation
    #[token("(")]
    LeftParen,

    #[token(")")]
    RightParen,

    #[token("{")]
    LeftBrace,

    #[token("}")]
    RightBrace,

    #[token("[")]
    LeftBracket,

    #[token("]")]
    RightBracket,

    #[token(",")]
    Comma,

    #[token(":")]
    Colon,

    #[token(";")]
    Semicolon,

    #[token(".")]
    Dot,

    // Identifiers
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    // Comments
    #[regex(r"//[^\n]*", logos::skip)]
    Comment,
}

pub struct Parser<'a> {
    source: &'a str,
    program: Program,
    constants: HashMap<String, TermId>, // Cache constants
    next_register: u32,
    scope: Vec<HashMap<String, TermId>>, // Variable scope stack
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            program: Program::new(ProgramKey::default()),
            constants: HashMap::new(),
            next_register: 0,
            scope: vec![HashMap::new()], // Global scope
        }
    }

    pub fn parse(&mut self) -> Result<Program, ParseError> {
        let mut lexer = Token::lexer(self.source);
        let mut current = self.program.add_term(TermOp::NoOp);
        let mut prev = current;

        // Simple expression parser for now
        // Parse tokens and build a simple linear program
        while let Some(token) = lexer.next() {
            match token {
                Ok(Token::Int(i)) => {
                    let const_id = self.program.add_constant(ConstantValue::Int(i));
                    current = self.program.add_term(TermOp::Constant(const_id));
                    self.program.get_term_mut(prev).unwrap().control_flow_next = Some(current);
                    prev = current;
                }
                Ok(Token::Float(f)) => {
                    let const_id = self.program.add_constant(ConstantValue::Float(f));
                    current = self.program.add_term(TermOp::Constant(const_id));
                    self.program.get_term_mut(prev).unwrap().control_flow_next = Some(current);
                    prev = current;
                }
                Ok(Token::Plus) => {
                    let left = prev;

                    if let Some(Ok(Token::Int(i))) = lexer.next() {
                        let const_id = self.program.add_constant(ConstantValue::Int(i));
                        let right = self.program.add_term(TermOp::Constant(const_id));

                        let add_term = self.program.add_term(TermOp::Add);
                        self.program.get_term_mut(add_term).unwrap().add_input(left);
                        self.program.get_term_mut(add_term).unwrap().add_input(right);

                        current = add_term;
                        prev = add_term;
                    }
                }
                Ok(Token::EOL) => continue,
                _ => {}
            }
        }

        // Set entry point
        self.program.entry = current;

        Ok(self.program.clone())
    }

    fn resolve_variable(&self, name: &str) -> Option<TermId> {
        for scope in self.scope.iter().rev() {
            if let Some(id) = scope.get(name) {
                return Some(*id);
            }
        }
        None
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn test_parse_integer() {
//         let mut parser = Parser::new("42");
//         let program = parser.parse().unwrap();
//         assert!(program.terms.len() > 0);
//     }

//     #[test]
//     fn test_parse_addition() {
//         let mut parser = Parser::new("1 + 2");
//         let program = parser.parse().unwrap();
//         assert!(program.terms.len() > 0);
//     }
// }
