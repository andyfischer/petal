#include "host/host_api.h"
#include "runtime/vm.h"
#include "runtime/heap.h"
#include "globals/global_state.h"
#include "parser/parse_program.h"
#include <cassert>
#include <cstdio>

// Static VM reference for API functions
static VM* s_current_vm = nullptr;

void petal_register_host_function(Program* program, const char* name, HostFunctionPtr func) {
    // TODO: Implement program-specific host function registration
    GlobalState* gs = get_active_global_state();
    assert(gs != nullptr);
    
    SymbolId symbol = gs->get_or_create_symbol(name);
    gs->register_host_function(symbol, func, 0); // Default argc for now
}

VM* petal_get_vm() {
    return s_current_vm;
}

GlobalState* petal_get_global_state() {
    return get_active_global_state();
}

void petal_set_current_vm(VM* vm) {
    s_current_vm = vm;
}

// Parse definitions from Petal source
Program* petal_parse_defs(const char* source) {
    return parse_program(source, ParseProgramOptions{});
}

// Argument extraction helpers - type-safe access to function arguments
void* petal_get_void_ptr(VM* vm) {
    // TODO: Implement void pointer extraction from VM stack
    return nullptr;
}

i32 petal_get_i32(VM* vm) {
    // TODO: Implement i32 extraction from VM stack
    return 0;
}

f32 petal_get_f32(VM* vm) {
    // TODO: Implement f32 extraction from VM stack
    return 0.0f;
}

u32 petal_get_u32(VM* vm) {
    // TODO: Implement u32 extraction from VM stack
    return 0;
}

SymbolId petal_get_symbol(VM* vm) {
    // TODO: Implement symbol extraction from VM stack
    return 0;
}

BlockId petal_get_function_def(VM* vm) {
    // TODO: Implement function def extraction from VM stack
    return 0;
}

u32 petal_get_string_id(VM* vm) {
    // TODO: Implement string ID extraction from VM stack
    return 0;
}

void* petal_get_heap_ptr(VM* vm) {
    // TODO: Implement heap pointer extraction from VM stack
    return nullptr;
}

// Return value creation helpers
Variant32 petal_return_int(i32 value) {
    return Variant32::from_int(value);
}

Variant32 petal_return_float(f32 value) {
    return Variant32::from_float(value);
}

Variant32 petal_return_string(VM* vm, const char* str) {
    // Use VM API to add string constant
    u32 string_index = vm_add_string_constant(vm, str);
    return Variant32::from_string_id(string_index);
}

Variant32 petal_return_symbol(SymbolId symbol) {
    return Variant32::from_symbol(symbol);
}

Variant32 petal_return_none() {
    return Variant32::None();
}

// Error handling for host functions
void petal_report_error(VM* vm, const char* message) {
    printf("Host function error: %s\n", message);
    // TODO: Implement proper error handling mechanism
}