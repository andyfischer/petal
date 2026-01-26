#pragma once

#include "standard_headers.h"
#include "program/name_map.h"
#include "utils/lookup_table.h"
#include "runtime/native_funcs.h"
#include <unordered_map>

// Forward declarations
struct Bytecode;
struct VM;
struct RetainedBuffer;
struct Variant32;
enum class NativeFunction;

using BlockId = u32;
using BufferId = u32;

// Host function type for user-registered functions
typedef Variant32 (*HostFunctionPtr)(VM* vm);

// Host function registry entry
struct HostFunctionEntry {
    HostFunctionPtr func;
    u32 expected_argc;
};

struct GlobalState {

    // Symbols
    NameMap symbols;
    SymbolId get_or_create_symbol(const char* name);

    // Native functions
    std::unordered_map<SymbolId, NativeFunctionId> native_func_dict;
    NativeFunctionId get_native_function_by_name(SymbolId name_id);

    // Host functions (user-registered callbacks)
    std::unordered_map<SymbolId, HostFunctionEntry> host_functions;
    
    GlobalState();
    ~GlobalState();

    // Host function registration
    void register_host_function(SymbolId symbol, HostFunctionPtr func, u32 expected_argc);
    HostFunctionEntry* lookup_host_function(SymbolId symbol);

};

GlobalState* get_active_global_state();
void reset_active_global_state();
