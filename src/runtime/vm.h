#pragma once

#include "standard_headers.h"

struct Block;
struct VM;
struct Variant32;

// VM execution status
enum VMExecutionStatus {
    VM_STATUS_SUCCESS = 0,
    VM_STATUS_FAILED = 1
};

// VM management
VM* vm_create();
void vm_destroy(VM* vm);
void vm_reset(VM* vm);
u32 vm_get_slot_array_size(VM* vm);

// Execution
struct Bytecode;
void vm_prepare_entry(VM* vm, Bytecode* bytecode, BlockId block_id);
void vm_reserve_frame_size(VM* vm, u32 frame_size);
void vm_execute(VM* vm, Instruction* isns, u32 num_isns);

// Accessing input/output values.
Variant32* vm_io_slot(VM* vm, u32 locals_idx);
void vm_io_set_u32(VM* vm, u32 locals_idx, u32 value);
void vm_io_set_i32(VM* vm, u32 locals_idx, i32 value);
void vm_io_set_f32(VM* vm, u32 locals_idx, f32 value);
void vm_io_set_symbol(VM* vm, u32 locals_idx, SymbolId value);
void vm_io_set_string_id(VM* vm, u32 locals_idx, u32 string_id);
void vm_io_set_heap_ptr(VM* vm, u32 locals_idx, void* ptr);
void vm_io_set_function_def(VM* vm, u32 locals_idx, BlockId block_id);
void vm_io_set_none(VM* vm, u32 locals_idx);

// Heap management
void* vm_heap_alloc(VM* vm, u32 size, u32 type_id);
void vm_heap_free(VM* vm, void* ptr);
void vm_heap_gc(VM* vm);

// String operations
struct HeapString;
HeapString* vm_create_heap_string(VM* vm, const char* str);
const char* vm_get_heap_string_data(void* heap_ptr);
u32 vm_get_heap_string_length(void* heap_ptr);
u32 vm_add_string_constant(VM* vm, const char* str);
HeapString* variant_to_heap_string(VM* vm, Variant32 value);

// Frame header operations
u32 vm_pack_frame_header(u16 return_address, u8 prev_frame_size);
void vm_unpack_frame_header(u32 frame_header, u16* return_address, u8* prev_frame_size);

// VM execution status
VMExecutionStatus vm_get_last_run_status(VM* vm);
void vm_set_execution_failed(VM* vm, const char* error_message);

// VM heap operations
void vm_op_heap_alloc_string(VM* vm, u8 result_slot, u8 data_slot, u8 length_slot);
void vm_op_heap_alloc_array(VM* vm, u8 result_slot, u8 capacity_slot, u8 element_size_slot);

// VM test utilities
void vm_add_function_bytecode(VM* vm, BlockId func_id, Instruction* instructions, u32 count);
void vm_set_slot_slot(VM* vm, u8 slot, Variant32 value);
void vm_resize_slot_array(VM* vm, u32 size);
Variant32 vm_get_slot(VM* vm, u8 slot);
i32 vm_get_slot_i32(VM* vm, u8 slot);
SymbolId vm_get_slot_sym(VM* vm, u8 slot);
