#include <string>
#include <optional>
#include <cassert>
#include "parser/tokens.h"
#include "parser/lexer_char_iterator.h" 

bool is_letter(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
}

bool is_number(char c) {
    return c >= '0' && c <= '9';
}

bool is_hexadecimal_digit(char c) {
    return is_number(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

bool is_identifier_first_letter(char c) {
    return is_letter(c) || c == '_';
}

bool is_acceptable_inside_identifier(char c) {
    return is_letter(c) || is_number(c) || c == '_';
}

bool is_whitespace(char c) {
    return c == ' ' || c == '\t';
}

bool is_newline(char c) {
    return c == '\n';
}

// Forward declarations
void consume_whitespace(CharIterator* it, FoundToken* result);
void consume_identifier(CharIterator* it, FoundToken* result);
void consume_hex_number(CharIterator* it, FoundToken* result);
bool match_number(CharIterator* char_iter);
void consume_number(CharIterator* it, FoundToken* result);
void consume_equals(CharIterator* it, FoundToken* result);
void consume_not_equals(CharIterator* it, FoundToken* result);
void consume_dot(CharIterator* it, FoundToken* result);
void consume_star(CharIterator* it, FoundToken* result);
void consume_plus(CharIterator* it, FoundToken* result);
void consume_minus(CharIterator* it, FoundToken* result);
void consume_greater_than(CharIterator* it, FoundToken* result);
void consume_less_than(CharIterator* it, FoundToken* result);
void consume_color_literal(CharIterator* it, FoundToken* result);
void consume_colon(CharIterator* it, FoundToken* result);
void consume_vertical_bar(CharIterator* it, FoundToken* result);
void consume_slash(CharIterator* it, FoundToken* result);
void consume_string_literal(CharIterator* it, FoundToken* result);
void consume_binary_number(CharIterator* it, FoundToken* result);

void lex_next_token(CharIterator* it, FoundToken* result) {
    char c = it->peek(0);

    if (c == '\0') {
        result->tok_match = Token::None;
        return;
    }

    if (is_identifier_first_letter(c)) {
        consume_identifier(it, result);
        return;
    }
    
    if (c == '0' && it->peek(1) == 'x') {
        consume_hex_number(it, result);
        return;
    }
    
    if (c == '0' && it->peek(1) == 'b') {
        consume_binary_number(it, result);
        return;
    }

    if (match_number(it)) {
        consume_number(it, result);
        return;
    }

    switch (c) {
        case '(': return it->consume(Token::LParen, 1, result);
        case ')': return it->consume(Token::RParen, 1, result);
        case '{': return it->consume(Token::LBrace, 1, result);
        case '}': return it->consume(Token::RBrace, 1, result);
        case '[': return it->consume(Token::LSquare, 1, result);
        case ']': return it->consume(Token::RSquare, 1, result);
        case ',': return it->consume(Token::Comma, 1, result);
        case '@': return it->consume(Token::At, 1, result);
        case '=': return consume_equals(it, result);
        case '"':
        case '\'': return consume_string_literal(it, result);
        case '\n': return it->consume(Token::Newline, 1, result);
        case '.': return consume_dot(it, result);
        case '?': return it->consume(Token::Question, 1, result);
        case '*': return consume_star(it, result);
        case '/': return consume_slash(it, result);
        case '!': return consume_not_equals(it, result);
        case ' ': return consume_whitespace(it, result);
        case '\t': return consume_whitespace(it, result);
        case ':': return consume_colon(it, result);
        case '+': return consume_plus(it, result);
        case '-': return consume_minus(it, result);
        case '<': return consume_less_than(it, result);
        case '>': return consume_greater_than(it, result);
        case '%': return it->consume(Token::Percent, 1, result);
        case '|': return consume_vertical_bar(it, result);
        // case '&': return consume_ampersand(it);
        case ';': return it->consume(Token::Semicolon, 1, result);
        case '#': return consume_color_literal(it, result);
        default: return it->consume(Token::Unrecognized, 1, result);
    }
}

void consume_whitespace(CharIterator* it, FoundToken* result) {
    int lookahead = 0;
    while (is_whitespace(it->peek(lookahead))) {
        lookahead++;
    }

    it->consume(Token::Whitespace, lookahead, result);
}

void consume_identifier(CharIterator* it, FoundToken* result) {
    int lookahead = 0;
    while (is_acceptable_inside_identifier(it->peek(lookahead))) {
        lookahead++;
    }
    
    // Check for known keywords
    if (it->next_matches_text("fn")) {
        return it->consume(Token::Fn, lookahead, result);
    }
    if (it->next_matches_text("let")) {
        return it->consume(Token::Let, lookahead, result);
    }
    if (it->next_matches_text("return")) {
        return it->consume(Token::Return, lookahead, result);
    }
    if (it->next_matches_text("if")) {
        return it->consume(Token::If, lookahead, result);
    }
    if (it->next_matches_text("else")) {
        return it->consume(Token::Else, lookahead, result);
    }
    if (it->next_matches_text("while")) {
        return it->consume(Token::While, lookahead, result);
    }
    if (it->next_matches_text("for")) {
        return it->consume(Token::For, lookahead, result);
    }
    if (it->next_matches_text("true")) {
        return it->consume(Token::True, lookahead, result);
    }
    if (it->next_matches_text("false")) {
        return it->consume(Token::False, lookahead, result);
    }
    if (it->next_matches_text("null")) {
        return it->consume(Token::Null, lookahead, result);
    }
    if (it->next_matches_text("struct")) {
        return it->consume(Token::Struct, lookahead, result);
    }

    return it->consume(Token::Identifier, lookahead, result);
}

void consume_hex_number(CharIterator* it, FoundToken* result) {
    int lookahead = 2; // Skip '0x'
    while (is_hexadecimal_digit(it->peek(lookahead))) {
        lookahead++;
    }

    it->consume(Token::HexInteger, lookahead, result);
}

void consume_binary_number(CharIterator* it, FoundToken* result) {
    int lookahead = 2; // Skip '0b'
    while (it->peek(lookahead) == '0' || it->peek(lookahead) == '1') {
        lookahead++;
    }

    it->consume(Token::BinaryInteger, lookahead, result);
}

/*
 match_number

 Lookahead to check if the next characters are a number.
*/
bool match_number(CharIterator* char_iter) {
    int lookahead = 0;

    if (char_iter->peek(lookahead) == '.') {
        lookahead++;
    }

    if (is_number(char_iter->peek(lookahead))) {
        return true;
    }

    return false;
}

void consume_number(CharIterator* it, FoundToken* result) {
    bool dot_encountered = false;
    int lookahead = 0;

    if (it->peek(lookahead) == '-') {
        lookahead++;
    }

    while (!it->finished()) {
        char c = it->peek(lookahead);

        if (is_number(c)) {
            lookahead++;
            continue;
        }

        if (c == '.') {
            if (dot_encountered) {
                break;
            }

            if (it->peek(lookahead + 1) == '.') {
                break;
            }

            lookahead++;
            dot_encountered = true;
            continue;
        }

        break;
    }
    
    // Check for scientific notation (e.g., 1.23e-4, 1e5)
    char c = it->peek(lookahead);
    if (c == 'e' || c == 'E') {
        int saved_lookahead = lookahead;
        lookahead++; // Consume 'e' or 'E'
        
        // Check for optional sign
        char next = it->peek(lookahead);
        if (next == '+' || next == '-') {
            lookahead++;
        }
        
        // Must have at least one digit after 'e'
        if (!is_number(it->peek(lookahead))) {
            lookahead = saved_lookahead; // Back up if no digit follows
        } else {
            // Consume remaining digits in exponent
            while (is_number(it->peek(lookahead))) {
                lookahead++;
            }
            dot_encountered = true; // Scientific notation is always a float
        }
    }

    if (dot_encountered) {
        it->consume(Token::Float, lookahead, result);
    } else {
        it->consume(Token::Integer, lookahead, result);
    }
}

void consume_string_literal(CharIterator* it, FoundToken* result) {
    char quote_type = it->peek(0);
    bool escaped = false;

    int lookahead = 1; // Skip initial quote
    
    while (it->peek(lookahead) != '\0') {
        char c = it->peek(lookahead);
        
        if (c == quote_type && !escaped) {
            lookahead++; // Include the closing quote
            break;
        }

        if (c == '\\' && !escaped) {
            escaped = true;
        } else {
            escaped = false;
        }

        lookahead++;
    }

    it->consume(Token::StringLiteral, lookahead, result);
}

void consume_equals(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '=') {
        it->consume(Token::DoubleEquals, 2, result);
    } else if (it->peek(1) == '>') {
        it->consume(Token::FatArrow, 2, result);
    } else {
        it->consume(Token::Equals, 1, result);
    }
}

void consume_not_equals(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '=') {
        it->consume(Token::NotEquals, 2, result);
    } else {
        // Just '!' by itself - unrecognized for now
        it->consume(Token::Unrecognized, 1, result);
    }
}

void consume_dot(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '.') {
        if (it->peek(2) == '.') {
            it->consume(Token::Ellipsis, 3, result);
        } else {
            it->consume(Token::TwoDots, 2, result);
        }
    } else {
        it->consume(Token::Dot, 1, result);
    }
}

void consume_star(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '=') {
        return it->consume(Token::StarEquals, 2, result);
    }

    if (it->peek(1) == '*') {
        return it->consume(Token::DoubleStar, 2, result);
    }

    it->consume(Token::Star, 1, result);
}

void consume_plus(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '=') {
        return it->consume(Token::PlusEquals, 2, result);
    }

    it->consume(Token::Plus, 1, result);
}

void consume_minus(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '>') {
        return it->consume(Token::RightArrow, 2, result);
    }

    if (it->peek(1) == '=') {
        return it->consume(Token::MinusEquals, 2, result);
    }

    it->consume(Token::Minus, 1, result);
}

void consume_comment(CharIterator* it, FoundToken* result) {
    int lookahead = 0;
    while (char c = it->peek(lookahead)) {
        if (c == '\n') {
            break;
        }
        lookahead++;
    }
    
    it->consume(Token::Comment, lookahead, result);
}

void consume_greater_than(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '=') {
        return it->consume(Token::GThanEq, 2, result);
    }

    it->consume(Token::GThan, 1, result);
}

void consume_less_than(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '=') {
        return it->consume(Token::LThanEq, 2, result);
    }

    it->consume(Token::LThan, 1, result);
}

void consume_color_literal(CharIterator* it, FoundToken* result) {
    int lookahead = 1; // Start after the '#'

    while (is_hexadecimal_digit(it->peek(lookahead))) {
        lookahead++;
    }

    int hex_digits = lookahead - 1; // Exclude the '#'

    // Acceptable lengths are 3, 4, 6, or 8 characters (not including '#')
    if (hex_digits == 3 || hex_digits == 4 || hex_digits == 6 || hex_digits == 8) {
        return it->consume(Token::Color, lookahead, result);
    } else {
        return it->consume(Token::Unrecognized, lookahead, result);
    }
}

void consume_colon(CharIterator* it, FoundToken* result) {
    if (is_acceptable_inside_identifier(it->peek(1))) {
        // Parse as a symbol.
        int lookahead = 1;

        while (is_acceptable_inside_identifier(it->peek(lookahead))) {
            lookahead++;
        }

        return it->consume(Token::Symbol, lookahead, result);
    }

    if (it->peek(1) == ':') {
        return it->consume(Token::DoubleColon, 2, result);
    }

    it->consume(Token::Colon, 1, result);
}

void consume_vertical_bar(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '|') {
        return it->consume(Token::DoubleVerticalBar, 2, result);
    }

    it->consume(Token::VerticalBar, 1, result);
}

void consume_slash(CharIterator* it, FoundToken* result) {
    if (it->peek(1) == '/') {
        // Line comment
        return consume_comment(it, result);
    }
    // TODO: Handle division and /= operators
    it->consume(Token::Slash, 1, result);
}
