#pragma once

#include "standard_headers.h"
#include "runtime/heap_types.h"

struct VM;

// VM heap operation opcodes
const u8 OP_HEAP_ALLOC_STRING = 0x70;
const u8 OP_HEAP_ALLOC_ARRAY = 0x71;
const u8 OP_HEAP_FREE = 0x72;
const u8 OP_HEAP_GC = 0x73;

// String operations
void vm_op_heap_alloc_string(VM* vm, u8 result_slot, u8 data_slot, u8 length_slot);
void vm_op_heap_free(VM* vm, u8 ptr_slot);
void vm_op_heap_gc(VM* vm);

// Array operations  
void vm_op_heap_alloc_array(VM* vm, u8 result_slot, u8 capacity_slot, u8 element_size_slot);