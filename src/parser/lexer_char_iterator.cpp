#include <string>
#include <cassert>
#include "parser/tokens.h"
#include "parser/lexer_char_iterator.h"

/*
 * FoundToken implementation
 */
FoundToken::FoundToken() : 
    tok_match(Token::None),
    char_start(0),
    char_end(0),
    line_start(0),
    line_end(0),
    col_start(0),
    col_end(0),
    preceding_indent(0) {}

/*
 * CharIterator implementation
 *
 * Helper object that advances character-by-character through the source text.
 * Keeps track of the current line and column position, as well as the
 * preceding indentation level.
 * Used during source code lexing.
 */
CharIterator::CharIterator(const char* input) : 
    text(input),
    next_index(0),
    line_position(1),
    char_position(0),
    preceding_indent(-1) {}

char CharIterator::peek(size_t lookahead) const {

    // Make sure we don't go out of bounds
    for (size_t check_index = next_index; check_index < next_index + lookahead; check_index++) {
        if (text[check_index] == '\0') {
            throw std::runtime_error("CharIterator::peek() called with lookahead out of bounds");
        }
    }

    return text[next_index + lookahead];
}

char CharIterator::advance_char() {
    if (finished()) {
        return '\0';
    }

    char c = peek(0);
    next_index++;

    if (c == '\n') {
        line_position++;
        char_position = 0;
    } else {
        char_position++;
    }

    return c;
}

bool CharIterator::finished() const {
    return peek(0) == '\0';
}

void CharIterator::consume(Token tok_match, size_t len, FoundToken* result) {
    if (finished()) {
        throw std::runtime_error("CharIterator::consume() called but iterator is finished");
    }

    result->tok_match = tok_match;
    result->char_start = next_index;
    result->char_end = 0;
    result->line_start = line_position;
    result->line_end = 0;
    result->col_start = char_position;
    result->col_end = 0;
    result->preceding_indent = 0;

    for (size_t i = 0; i < len; i++) {
        advance_char();
    }

    result->char_end = next_index;
    result->line_end = line_position;
    result->col_end = char_position;

    // Update preceding_indent if this is the first whitespace on a line
    if (preceding_indent == -1) {
        if (result->tok_match == Token::Whitespace) {
            preceding_indent = static_cast<int>(len);
        } else {
            preceding_indent = 0;
        }
    }

    // Update line_end if this is a newline token.
    if (result->tok_match == Token::Newline) {
        result->line_end = result->line_start;
        result->col_end = result->col_start + 1;
        preceding_indent = -1;
    }

    result->preceding_indent = preceding_indent >= 0 ? static_cast<size_t>(preceding_indent) : 0;

    assert(result->line_start > 0);
    assert(result->line_end > 0);
    assert(result->line_start <= result->line_end);
    assert((result->col_end > result->col_start) || (result->line_start < result->line_end));
}

bool CharIterator::next_matches_text(const char* text) const {
    for (size_t i = 0;; i++) {
        if (text[i] == '\0') {
            return true;
        }

        if (peek(i) != text[i]) {
            return false;
        }
    }
    return true;
}