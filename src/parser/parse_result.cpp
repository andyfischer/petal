#include "parse_result.h"

ParseResult ParseResult::None() {
    return ParseResult();
}

ParseResult ParseResult::from_term_id(const TermId& term_id) {
    ParseResult result;
    result.type = ParseResultType::TermIdRef;
    result.term_id = term_id;
    return result;
}

ParseResult ParseResult::from_named_term_id(const TermId& term_id, SymbolId name_id) {
    ParseResult result;
    result.type = ParseResultType::NamedTermRef;
    result.term_id = term_id;
    result.name = name_id;
    return result;
}

bool ParseResult::is_none() const {
    return type == ParseResultType::None;
}

bool ParseResult::is_term_id() const {
    return type == ParseResultType::TermIdRef;
}

bool ParseResult::is_named_term() const {
    return type == ParseResultType::NamedTermRef;
}