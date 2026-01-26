#ifndef TOKENS_H
#define TOKENS_H

/*
 * Token
 *
 * Represents the different types of tokens that can be found during lexing.
 */
enum class Token {
    // Basic tokens
    None = 0,
    Unrecognized,
    Whitespace,
    Identifier,
    
    // Literals
    Integer = 10,
    Float,
    HexInteger,
    BinaryInteger,
    StringLiteral,
    Symbol,
    Color,
    
    // Parentheses and braces
    LParen = 20,    // (
    RParen,    // )
    LBrace,    // {
    RBrace,    // }
    LSquare,   // [
    RSquare,   // ]
    
    // Operators
    Plus = 30,      // +
    Minus,     // -
    Star,      // *
    DoubleStar, // **
    Slash,     // /
    Percent,   // %
    Equals,    // =
    DoubleEquals, // ==
    NotEquals, // !=
    FatArrow,  // =>
    RightArrow, // ->
    Dot,       // .
    TwoDots,   // ..
    Ellipsis,  // ...
    Comma,     // ,
    Colon,     // :
    DoubleColon, // ::
    Semicolon, // ;
    At,        // @
    Question,  // ?
    VerticalBar, // |
    DoubleVerticalBar, // ||
    
    // Comparison operators
    GThan,     // >
    LThan,     // <
    GThanEq,   // >=
    LThanEq,   // <=
    
    // Assignment operators
    PlusEquals,   // +=
    MinusEquals,  // -=
    StarEquals,   // *=
    
    // Keywords
    Fn,
    Let,
    Return,
    If,
    Else,
    While,
    For,
    True,
    False,
    Null,
    Struct,
    
    // Special tokens
    Newline,
    Comment
};

#endif // TOKENS_H