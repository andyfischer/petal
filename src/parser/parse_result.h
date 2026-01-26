
#pragma once

#include "standard_headers.h"
#include "program/term_id.h"
#include "runtime/native_funcs.h"

enum class ParseResultType {
    None,
    TermIdRef,
    NamedTermRef,
};

struct ParseResult {
    ParseResultType type;
    
    TermId term_id;
    SymbolId name;

    ParseResult() : type(ParseResultType::None), term_id(TermId::None()), name(0) {}

    static ParseResult None();
    static ParseResult from_term_id(const TermId& term_id);
    static ParseResult from_named_term_id(const TermId& term_id, SymbolId name_id);

    bool is_none() const;
    bool is_term_id() const;
    bool is_named_term() const;
};
