#include "parser/parse_token_iterator.h"
#include "parser/lexer.h"
#include "program/name_map.h"
TokenIterator::TokenIterator(const char* source_text)
    : source(source_text), position(0)
{
    initialize();
}

void TokenIterator::initialize() {
    CharIterator char_iter(source);
    
    while (!char_iter.finished()) {
        FoundToken token;
        lex_next_token(&char_iter, &token);
        
        // Skip comments if needed
        if (token.tok_match != Token::Comment) {
            tokens.push_back(token);
        }
    }
}

std::string TokenIterator::token_text(FoundToken* token) {
    return std::string(source + token->char_start, token->char_end - token->char_start);
}

const FoundToken* TokenIterator::next(size_t lookahead) const {
    if (finished(lookahead)) {
        return nullptr;
    }
    
    return &tokens[position + lookahead];
}

std::string TokenIterator::next_text(size_t lookahead) const {
    const FoundToken* token = next(lookahead);
    if (!token) {
        return "";
    }
    
    size_t length = token->char_end - token->char_start;
    return std::string(source + token->char_start, length);
}

bool TokenIterator::next_is(size_t lookahead, Token expected) const {
    const FoundToken* token = next(lookahead);
    return token && token->tok_match == expected;
}

u32 TokenIterator::next_ident_to_name_id(NameMap* names) {
    const FoundToken* token = next(0);
    if (!token || token->tok_match != Token::Identifier) {
        return 0;
    }

    std::string text = next_text(0);
    return names->get_or_add_name(text.c_str());
}

void TokenIterator::consume() {
    if (!finished(0)) {
        position++;
    }
}

bool TokenIterator::try_consume(Token expected) {
    if (next_is(0, expected)) {
        consume();
        return true;
    }
    return false;
}

void TokenIterator::skip_whitespace() {
    while (next_is(0, Token::Whitespace)) {
        consume();
    }
}

void TokenIterator::skip_whitespace_and_newlines() {
    while (next_is(0, Token::Whitespace) || next_is(0, Token::Newline)) {
        consume();
    }
}

bool TokenIterator::finished(size_t lookahead) const {
    return position + lookahead >= tokens.size();
}