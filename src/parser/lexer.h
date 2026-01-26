#ifndef LEXER_H
#define LEXER_H

#include <string>
#include <optional>
#include "parser/tokens.h"
#include "parser/lexer_char_iterator.h"

// Character classification functions
bool is_letter(char c);
bool is_number(char c);
bool is_hexadecimal_digit(char c);
bool is_identifier_first_letter(char c);
bool is_acceptable_inside_identifier(char c);
bool is_whitespace(char c);
bool is_newline(char c);

// Main lexer function
void lex_next_token(CharIterator* it, FoundToken* result);

// Token consumption functions
void consume_whitespace(CharIterator* it, FoundToken* result);
void consume_identifier(CharIterator* it, FoundToken* result);
void consume_hex_number(CharIterator* it, FoundToken* result);
bool match_number(CharIterator* char_iter);
void consume_number(CharIterator* it, FoundToken* result);
void consume_equals(CharIterator* it, FoundToken* result);
void consume_dot(CharIterator* it, FoundToken* result);
void consume_star(CharIterator* it, FoundToken* result);
void consume_plus(CharIterator* it, FoundToken* result);
void consume_minus(CharIterator* it, FoundToken* result);
void consume_greater_than(CharIterator* it, FoundToken* result);
void consume_less_than(CharIterator* it, FoundToken* result);
void consume_color_literal(CharIterator* it, FoundToken* result);
void consume_colon(CharIterator* it, FoundToken* result);
void consume_vertical_bar(CharIterator* it, FoundToken* result);

#endif // LEXER_H