#pragma once

#include <vector>
#include <string>
#include "standard_headers.h"
#include "parser/tokens.h"
#include "parser/lexer_char_iterator.h"

struct NameMap;

// A helper class for iterating through tokens in the source code
struct TokenIterator {
    const char* source;
    std::vector<FoundToken> tokens;
    size_t position;
    
    TokenIterator(const char* source_text);
    
    // Initialize the token iterator by tokenizing the entire source
    void initialize();

    std::string token_text(FoundToken* token);
    
    // Get the next token at the given lookahead position without advancing
    const FoundToken* next(size_t lookahead = 0) const;
    
    // Get the text of the next token as a std::string
    std::string next_text(size_t lookahead = 0) const;
    
    // Check if the next token at the given position matches the expected token type
    bool next_is(size_t lookahead, Token expected) const;
    
    // Look at the next identifier and save it as a name id in the given map.
    u32 next_ident_to_name_id(NameMap* names);
    
    // Consume the next token and advance the iterator
    void consume();
    
    // Try to consume a token of a specific type, return true if successful
    bool try_consume(Token expected);
    
    // Skip any whitespace tokens
    void skip_whitespace();
    
    // Skip any whitespace or newline tokens
    void skip_whitespace_and_newlines();
    
    // Check if we've reached the end of the token stream
    bool finished(size_t lookahead = 0) const;
};
