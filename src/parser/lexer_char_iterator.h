#pragma once

#include "parser/tokens.h"

/*
 * FoundToken
 * 
 * Data for a single token found during tokenization. Includes position information.
 */
struct FoundToken {
    // tok_match: The Token that was matched.
    Token tok_match;

    // char_start and char_end: The character start & end indexes in the original string.
    size_t char_start;
    size_t char_end;

    // line_start and line_end: The line numbers for this token match. Line numbers start at 1.
    // Usually, line_start == line_end, but for multi-line tokens, they can differ.
    size_t line_start;
    size_t line_end;
    
    // col_start and col_end: The column numbers for this token match (when the original
    // string is split into lines). Column numbers start at 0.
    size_t col_start;
    size_t col_end;

    // preceding_indent: The number of spaces that was used on the current line. All tokens
    // on the same line will have the same preceding_indent value.
    size_t preceding_indent;

    FoundToken();
};

/*
 * CharIterator
 *
 * Helper object that advances character-by-character through the source text.
 * Keeps track of the current line and column position, as well as the
 * preceding indentation level.
 * Used during source code lexing.
 */
struct CharIterator {
    const char* text;
    size_t next_index;
    size_t line_position;
    size_t char_position;
    int preceding_indent;

    CharIterator(const char* input);

    char peek(size_t lookahead) const;
    char advance_char();
    bool finished() const;
    bool within_range(size_t lookahead) const;
    void consume(Token tok_match, size_t len, FoundToken* result);
    bool next_matches_text(const char* text) const;
};