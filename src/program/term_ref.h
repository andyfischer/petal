#pragma once

#include <string>
#include "standard_headers.h"
#include "program/term_id.h"
#include "runtime/native_funcs.h"

enum class TermRefType {
    None,
    TermIdRef,
    NameRef,
    NativeFunctionRef,
};

struct TermRef {
    TermRefType type;
    
    union {
        TermId term_id;
        SymbolId name_id;
        NativeFunctionId native_function_id;
    };

    TermRef() : type(TermRefType::None) {}

    static TermRef None();
    static TermRef from_term_id(const TermId& term_id);
    static TermRef from_name_id(SymbolId name_id);
    static TermRef from_native_function_id(NativeFunctionId native_function_id);

    bool is_term_id() const;
    bool is_name() const;
    bool is_native_function() const;

    std::string to_debug_str() const;
};
