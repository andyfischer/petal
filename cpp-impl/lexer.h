#pragma once

#include <cstdint>

enum TokenType {
    // Literals
    TOK_INT,
    TOK_FLOAT,
    TOK_STRING,
    TOK_SYMBOL,
    TOK_IDENTIFIER,

    // Keywords
    TOK_FN,
    TOK_LET,
    TOK_RETURN,
    TOK_IF,
    TOK_ELSE,
    TOK_WHILE,
    TOK_FOR,
    TOK_IN,
    TOK_TRUE,
    TOK_FALSE,
    TOK_NULL,
    TOK_STRUCT,
    TOK_ENUM,
    TOK_STATE,
    TOK_MATCH,
    TOK_LOOP,
    TOK_BREAK,
    TOK_CONTINUE,

    // Operators
    TOK_PLUS,
    TOK_MINUS,
    TOK_STAR,
    TOK_SLASH,
    TOK_PERCENT,
    TOK_STAR_STAR,      // **
    TOK_EQ,             // ==
    TOK_NE,             // !=
    TOK_LT,             // <
    TOK_GT,             // >
    TOK_LE,             // <=
    TOK_GE,             // >=
    TOK_AND,            // &&
    TOK_OR,             // ||
    TOK_NOT,            // !
    TOK_ASSIGN,         // =
    TOK_PLUS_EQ,        // +=
    TOK_MINUS_EQ,       // -=
    TOK_STAR_EQ,        // *=
    TOK_SLASH_EQ,       // /=
    TOK_PERCENT_EQ,     // %=
    TOK_AT,             // @
    TOK_DOT,            // .
    TOK_COLON,          // :
    TOK_COLON_COLON,    // ::
    TOK_ARROW,          // ->
    TOK_FAT_ARROW,      // =>
    TOK_QUESTION,       // ?

    // Delimiters
    TOK_LPAREN,
    TOK_RPAREN,
    TOK_LBRACE,
    TOK_RBRACE,
    TOK_LBRACKET,
    TOK_RBRACKET,
    TOK_COMMA,
    TOK_SEMICOLON,

    // Special
    TOK_NEWLINE,
    TOK_EOF,
    TOK_ERROR
};

struct Token {
    TokenType type;
    const char* start;
    int length;
    int line;
    int column;

    // For numeric literals
    int64_t int_value;
    double float_value;
};

struct Lexer {
    const char* source;
    const char* start;
    const char* current;
    int line;
    int column;
};

void lexer_init(Lexer* lexer, const char* source);
Token lexer_next_token(Lexer* lexer);
Token lexer_peek_token(Lexer* lexer);
const char* token_type_name(TokenType type);
char* token_to_string(Token* token);
