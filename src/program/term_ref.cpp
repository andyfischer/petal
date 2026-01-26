#include "program/term_ref.h"

TermRef TermRef::None() {
    TermRef ref;
    ref.type = TermRefType::None;
    return ref;
}

TermRef TermRef::from_term_id(const TermId& term_id) {
    TermRef ref;
    ref.type = TermRefType::TermIdRef;
    ref.term_id = term_id;
    return ref;
}

TermRef TermRef::from_name_id(SymbolId name_id) {
    TermRef ref;
    ref.type = TermRefType::NameRef;
    ref.name_id = name_id;
    return ref;
}

TermRef TermRef::from_native_function_id(NativeFunctionId native_function_id) {
    TermRef ref;
    ref.type = TermRefType::NativeFunctionRef;
    ref.native_function_id = native_function_id;
    return ref;
}

bool TermRef::is_term_id() const {
    return type == TermRefType::TermIdRef;
}

bool TermRef::is_name() const {
    return type == TermRefType::NameRef;
}

bool TermRef::is_native_function() const {
    return type == TermRefType::NativeFunctionRef;
} 

std::string TermRef::to_debug_str() const {
    switch (type) {
    case TermRefType::None:
        return "(none)";
        
    case TermRefType::TermIdRef:
        return "term_id(" + std::to_string(term_id.block_id) + "," + std::to_string(term_id.term_local_id) + ")";
        
    case TermRefType::NameRef: {
        GlobalState* global_state = get_active_global_state();
        return "name(" + std::string(global_state->symbols.get_name_string(name_id)) + ")";
    }
        
    case TermRefType::NativeFunctionRef:
        return "native_func(" + std::to_string(static_cast<int>(native_function_id)) + ")";
        
    default:
        return "(unknown)";
    }
}
