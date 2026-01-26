#include "globals/global_state.h"
#include <cassert>

static GlobalState* g_active_global_state = nullptr;

GlobalState* get_active_global_state() {
    if (g_active_global_state == nullptr) {
        g_active_global_state = new GlobalState();
    }
    return g_active_global_state;
}

void reset_active_global_state() {
    if (g_active_global_state == nullptr) {
        delete g_active_global_state;
        g_active_global_state = nullptr;
    }
}

GlobalState::GlobalState() 
{
    // Set native function dictionary
    native_func_dict.insert({get_or_create_symbol("add"), NativeFunctionId::Add});
    native_func_dict.insert({get_or_create_symbol("sub"), NativeFunctionId::Sub});
    native_func_dict.insert({get_or_create_symbol("mult"), NativeFunctionId::Mult});
    native_func_dict.insert({get_or_create_symbol("div"), NativeFunctionId::Div});
    native_func_dict.insert({get_or_create_symbol("and"), NativeFunctionId::And});
    native_func_dict.insert({get_or_create_symbol("or"), NativeFunctionId::Or});
    native_func_dict.insert({get_or_create_symbol("xor"), NativeFunctionId::Xor});
    native_func_dict.insert({get_or_create_symbol("eq"), NativeFunctionId::Eq});
    native_func_dict.insert({get_or_create_symbol("ne"), NativeFunctionId::Ne});
    native_func_dict.insert({get_or_create_symbol("lt"), NativeFunctionId::Lt});
    native_func_dict.insert({get_or_create_symbol("gt"), NativeFunctionId::Gt});
    native_func_dict.insert({get_or_create_symbol("le"), NativeFunctionId::Le});
    native_func_dict.insert({get_or_create_symbol("ge"), NativeFunctionId::Ge});
    native_func_dict.insert({get_or_create_symbol("inc"), NativeFunctionId::Inc});
}

GlobalState::~GlobalState() {
}

SymbolId GlobalState::get_or_create_symbol(const char* name) {
    return symbols.get_or_add_name(name);
}

NativeFunctionId GlobalState::get_native_function_by_name(SymbolId name_id) {
    return native_func_dict[name_id];
}

void GlobalState::register_host_function(SymbolId symbol, HostFunctionPtr func, u32 expected_argc) {
    HostFunctionEntry entry;
    entry.func = func;
    entry.expected_argc = expected_argc;
    host_functions[symbol] = entry;
}

HostFunctionEntry* GlobalState::lookup_host_function(SymbolId symbol) {
    auto it = host_functions.find(symbol);
    if (it != host_functions.end()) {
        return &it->second;
    }
    return nullptr;
}
