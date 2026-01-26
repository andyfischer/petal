#pragma once

#include "standard_headers.h"
#include "variant/variant.h"
#include "program/name_map.h"

// Forward declarations
struct VM;
struct GlobalState;
struct Program;

extern "C" {

typedef Variant32 (*HostFunctionPtr)(VM* vm);

// Parse definitions from Petal source
Program* petal_parse_defs(const char* source);

// Registration API
void petal_register_host_function(Program* program, const char* name, HostFunctionPtr func);

// Argument extraction helpers - type-safe access to function arguments
void* petal_get_void_ptr(VM* vm);
i32 petal_get_i32(VM* vm);

// Error handling for host functions
void petal_report_error(VM* vm, const char* message);

}
