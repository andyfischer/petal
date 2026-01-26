#include "runtime/vm.h"
#include "variant/variant.h"
#include "bytecode/bytecode.h"
#include "bytecode/bytecode_encoding.h"
#include "bytecode/bytecode_helpers.h"
#include "standard_headers.h"
#include "globals/global_state.h"
#include "runtime/heap.h"
#include "runtime/heap_types.h"
#include "runtime/heap_vm_ops.h"
#include "host/host_api.h"
#include "assert.h"
#include <cstring>

struct VM {
    // Register slots / stack
    std::vector<Variant32> slots;
    u32 stack_top; 

    // Program counter
    u32 pc;
    
    // Execution status
    VMExecutionStatus last_run_status;
                   
    std::vector<HeapString*> strings;
    Heap heap;
};

VM* vm_create() {
    VM* vm = new VM();
    vm->last_run_status = VM_STATUS_SUCCESS;
    heap_init(&vm->heap, 64 * 1024);  // Start with 64KB heap
    return vm;
}

void vm_destroy(VM* vm) {
    heap_cleanup(&vm->heap);
    delete vm;
}

u32 vm_get_slot_array_size(VM* vm) {
    return vm->slots.size();
}

// Add a string constant to the VM's string table
// Returns the string ID that can be used to reference it
u32 vm_add_string_constant(VM* vm, const char* str) {
    HeapString* heap_str = heap_string_create(vm, str, strlen(str));
    u32 string_id = vm->strings.size();
    vm->strings.push_back(heap_str);
    return string_id;
}

// Heap access functions
void* vm_heap_alloc(VM* vm, u32 size, u32 type_id) {
    return heap_alloc(&vm->heap, size, type_id);
}

void vm_heap_free(VM* vm, void* ptr) {
    heap_free(&vm->heap, ptr);
}

void vm_heap_gc(VM* vm) {
    // Mark phase: mark all reachable objects from VM slots
    for (const auto& variant : vm->slots) {
        if (variant.type == VariantType::HeapPtr) {
            heap_gc_mark(&vm->heap, variant.get_heap_ptr());
        }
        // TODO: Add recursive marking for composite objects
    }
    
    heap_gc_run(&vm->heap);
}

// String convenience functions
HeapString* vm_create_heap_string(VM* vm, const char* str) {
    if (!str) return nullptr;
    u32 len = strlen(str);
    return heap_string_create(vm, str, len);
}

const char* vm_get_heap_string_data(void* heap_ptr) {
    if (!heap_ptr) return nullptr;
    HeapString* str = (HeapString*)heap_ptr;
    return str->data;
}

u32 vm_get_heap_string_length(void* heap_ptr) {
    if (!heap_ptr) return 0;
    HeapString* str = (HeapString*)heap_ptr;
    return str->length;
}

// VM heap operations
void vm_op_heap_alloc_string(VM* vm, u8 result_slot, u8 data_slot, u8 length_slot) {
    // Get the string ID from the data slot
    u32 string_id = vm->slots[data_slot].get_string_id();
    
    // Check if string_id is valid
    if (string_id >= vm->strings.size()) {
        vm->slots[result_slot] = Variant32::None();
        return;
    }
    
    // Get the pre-allocated heap string from the strings table
    HeapString* source_str = vm->strings[string_id];
    if (!source_str) {
        vm->slots[result_slot] = Variant32::None();
        return;
    }
    
    // Create a new heap-allocated string copy
    HeapString* heap_str = heap_string_create(vm, source_str->data, source_str->length);
    
    if (heap_str) {
        vm->slots[result_slot] = Variant32::from_heap_ptr(heap_str);
    } else {
        vm->slots[result_slot] = Variant32::None();
    }
}

void vm_op_heap_alloc_array(VM* vm, u8 result_slot, u8 capacity_slot, u8 element_size_slot) {
    u32 capacity = vm->slots[capacity_slot].get_int32();
    u32 element_size = vm->slots[element_size_slot].get_int32();
    
    HeapArray* heap_array = heap_array_create(vm, capacity, element_size);
    
    if (heap_array) {
        vm->slots[result_slot] = Variant32::from_heap_ptr(heap_array);
    } else {
        vm->slots[result_slot] = Variant32::None();
    }
}

// Helper function to send effect
// Convert Variant32 to string representation as a HeapString
HeapString* variant_to_heap_string(VM* vm, Variant32 value) {
    char buffer[256]; // Temporary buffer for formatting
    
    switch (value.type) {
        case VariantType::None:
            return vm_create_heap_string(vm, "none");
        case VariantType::I32:
            snprintf(buffer, sizeof(buffer), "%d", value.get_int32());
            return vm_create_heap_string(vm, buffer);
        case VariantType::Float32:
            snprintf(buffer, sizeof(buffer), "%f", value.get_float32());
            return vm_create_heap_string(vm, buffer);
        case VariantType::Symbol: {
            GlobalState* state = get_active_global_state();
            const char* symbol_str = state->symbols.get_name_string(value.get_symbol_id());
            snprintf(buffer, sizeof(buffer), ":%s", symbol_str);
            return vm_create_heap_string(vm, buffer);
        }
        case VariantType::String: {
            u32 string_id = value.get_string_id();
            if (string_id < vm->strings.size()) {
                HeapString* heap_str = vm->strings[string_id];
                return heap_string_create(vm, heap_str->data, heap_str->length);
            }
            return vm_create_heap_string(vm, "invalid_string");
        }
        case VariantType::FunctionDef:
            snprintf(buffer, sizeof(buffer), "function_def(%u)", value.get_block_id());
            return vm_create_heap_string(vm, buffer);
        case VariantType::HeapPtr:
            return vm_create_heap_string(vm, "heap_ptr");
        default:
            return vm_create_heap_string(vm, "unknown_variant");
    }
}

void vm_print(VM* vm, Variant32 value) {
    HeapString* message_str = variant_to_heap_string(vm, value);
    printf("%s\n", message_str->data);
}

void vm_reset(VM* vm) {
    vm->pc = 0;
    vm->stack_top = 0;
    vm->last_run_status = VM_STATUS_SUCCESS;
}

// Frame header operations
u32 vm_pack_frame_header(u16 return_address, u8 prev_frame_size) {
    return ((u32)return_address << 8) | (u32)prev_frame_size;
}

void vm_unpack_frame_header(u32 frame_header, u16* return_address, u8* prev_frame_size) {
    *return_address = (frame_header >> 8) & 0xFFFF;
    *prev_frame_size = frame_header & 0xFF;
}

// VM execution status
VMExecutionStatus vm_get_last_run_status(VM* vm) {
    return vm->last_run_status;
}

void vm_set_execution_failed(VM* vm, const char* error_message) {
    vm->last_run_status = VM_STATUS_FAILED;
    if (vm->slots.size() > 0) {
        vm_io_slot(vm, 0)->set_heap_ptr(vm_create_heap_string(vm, error_message));
    }
}

void vm_execute(VM* vm, Instruction* isns, u32 num_isns) {
    // Validate VM state before execution
    if (vm->slots.size() == 0) {
        vm_set_execution_failed(vm, "VM execution failed: No slots allocated");
        return;
    }
    
    if (vm->slots[0].type != VariantType::U32) {
        vm_set_execution_failed(vm, "VM execution failed: Missing frame header at slot 0");
        return;
    }
    
    u32 budget = 1000;

    Instruction* current_isns = isns;
    Instruction* end_isns = isns + num_isns;

    while (budget > 0 && current_isns < end_isns) {
        u8 opcode = unpack_opcode(*current_isns);

        switch (opcode) {

            case OP_NOPE: {
                // No operation - do nothing
                break;
            }

            case OP_STOP: {
                vm->pc = (u32)(current_isns - isns);
                return;
            }

            case OP_RESERVE_SLOTS: {
                u16 count = unpack_op_reserve_slots__count(*current_isns);
                vm->slots.resize(vm->slots.size() + count);
                break;
            }

            case OP_CONST_I16: {
                u8 slot = unpack_op_const_i16__slot(*current_isns);
                i16 value = unpack_op_const_i16__value(*current_isns);
                vm->slots[vm->stack_top + slot] = Variant32::from_int(value);
                break;
            }

            case OP_CONST_U16: {
                u8 slot = unpack_op_const_u16__slot(*current_isns);
                u16 value = unpack_op_const_u16__value(*current_isns);
                vm->slots[vm->stack_top + slot] = Variant32::from_int(value);
                break;
            }

            case OP_CONST_U16_SYM: {
                u8 slot = unpack_op_const_u16_sym__slot(*current_isns);
                u16 value = unpack_op_const_u16_sym__value(*current_isns);
                vm->slots[vm->stack_top + slot] = Variant32::from_symbol(value);
                break;
            }


            case OP_CALL: {
                u16 func_address = unpack_op_call__func_address(*current_isns);
                u8 stack_size = unpack_op_call__stack_size(*current_isns);
                
                // Pack frame header: u16 return_address + u8 prev_frame_size
                u32 frame_header = vm_pack_frame_header(vm->pc + 1, vm->stack_top);
                
                // Save frame header in new local:0 slot
                u32 new_stack_top = vm->stack_top + stack_size;
                vm->slots[new_stack_top] = Variant32::from_u32(frame_header);
                
                // Update stack_top to new frame
                vm->stack_top = new_stack_top;
                
                // Jump to function address
                vm->pc = func_address;
                break;
            }

            case OP_RETURN: {
                u8 return_slot = unpack_op_return__return_slot(*current_isns);
                
                // Get return value from current frame
                Variant32 return_value = vm->slots[vm->stack_top + return_slot];
                
                // Get frame header from local:0
                u32 frame_header = vm->slots[vm->stack_top].get_u32();
                u16 return_address;
                u8 prev_frame_size;
                vm_unpack_frame_header(frame_header, &return_address, &prev_frame_size);
                
                // If return_address is 0, we're returning from the top level - just exit
                if (return_address == 0) {
                    return;
                }
                
                // Restore previous stack_top
                vm->stack_top = prev_frame_size;
                
                // Store return value in the caller's "new frame" area
                // (The slot right after the old local frame)
                vm->slots[vm->stack_top + 1] = return_value;
                
                // Jump back to return address
                vm->pc = return_address;
                break;
            }

            case OP_CALL_HOST: {
                u8 symbol_slot = unpack_op_call_host__symbol_slot(*current_isns);
                u8 argc = unpack_op_call_host__argc(*current_isns);
                
                // Get the function symbol from the stack
                SymbolId func_symbol = vm_get_slot_sym(vm, symbol_slot);
                
                // Look up the host function
                GlobalState* gs = get_active_global_state();
                HostFunctionEntry* entry = gs->lookup_host_function(func_symbol);
                
                if (entry) {
                    // Validate argument count
                    if (argc != entry->expected_argc) {
                        // TODO: Better error handling
                        break;
                    }
                    
                    // Set up arguments array (arguments are in slots 0, 1, 2, ... argc-1)
                    Variant32* args = nullptr;
                    if (argc > 0) {
                        // Ensure we have enough slots in the stack
                        if (vm->slots.size() < vm->stack_top + argc) {
                            break;
                        }
                        args = &vm->slots[vm->stack_top];  // Arguments start at current frame
                    }
                    
                    // Set current VM context for host API
                    petal_set_current_vm(vm);
                    
                    // Call the host function
                    Variant32 result = entry->func(vm);
                    
                    // Push result onto stack (don't pop arguments since they're in specific slots)
                    // TODO: Store result in appropriate slot for host call return
                } else {
                    // Host function not found - push None as result
                    // TODO: Store None result in appropriate slot for host call return
                }
                break;
            }

            // Claude added:
            case OP_MOVE: {
                u8 from_slot = unpack_op_move__from_slot(*current_isns);
                u8 to_slot = unpack_op_move__to_slot(*current_isns);
                vm->slots[vm->stack_top + to_slot] = vm->slots[vm->stack_top + from_slot];
                break;
            }

            // Claude added:
            case OP_I32_ADD: {
                u8 slot_a = unpack_op_i32_add__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_add__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_add__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a + b);
                break;
            }

            // Claude added:
            case OP_I32_SUB: {
                u8 slot_a = unpack_op_i32_sub__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_sub__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_sub__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a - b);
                break;
            }

            // Claude added:
            case OP_I32_MULT: {
                u8 slot_a = unpack_op_i32_mult__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_mult__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_mult__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a * b);
                break;
            }

            // Claude added:
            case OP_I32_DIV_S: {
                u8 slot_a = unpack_op_i32_div_s__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_div_s__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_div_s__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                assert(b != 0 && "Division by zero");
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a / b);
                break;
            }

            // Claude added:
            case OP_I32_DIV_U: {
                // TODO: Add parameters for unsigned division
                break;
            }

            case OP_COPY: {
                u8 from_slot = unpack_op_copy__from_slot(*current_isns);
                u8 to_slot = unpack_op_copy__to_slot(*current_isns);
                *vm_io_slot(vm, to_slot) = *vm_io_slot(vm, from_slot);
                break;
            }

            case OP_COMPILE_ERROR: {
                vm_set_execution_failed(vm, "error: reached compile error");
                return;
            }

            case OP_UNREACHABLE: {
                vm_set_execution_failed(vm, "error: op_unreachable");
                return;
            }

            case OP_COMMENT: {
                // Comment instruction - just skip it
                break;
            }

            // Control flow operations
            case OP_JUMP: {
                u16 address = unpack_op_jump__address(*current_isns);
                current_isns = isns + address;
                continue; // Skip normal increment
            }

            case OP_JUMP_IF_TRUE: {
                u8 condition_slot = unpack_op_jump_if_true__condition_slot(*current_isns);
                u16 address = unpack_op_jump_if_true__address(*current_isns);
                i32 condition = vm_get_slot_i32(vm, condition_slot);
                if (condition != 0) {
                    current_isns = isns + address;
                    continue; // Skip normal increment
                }
                break;
            }

            case OP_JUMP_IF_FALSE: {
                u8 condition_slot = unpack_op_jump_if_false__condition_slot(*current_isns);
                u16 address = unpack_op_jump_if_false__address(*current_isns);
                i32 condition = vm_get_slot_i32(vm, condition_slot);
                if (condition == 0) {
                    current_isns = isns + address;
                    continue; // Skip normal increment
                }
                break;
            }

            // Comparison operations
            case OP_I32_EQ: {
                u8 slot_a = unpack_op_i32_eq__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_eq__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_eq__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a == b ? 1 : 0);
                break;
            }

            case OP_I32_LT: {
                u8 slot_a = unpack_op_i32_lt__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_lt__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_lt__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a < b ? 1 : 0);
                break;
            }

            case OP_I32_GT: {
                u8 slot_a = unpack_op_i32_gt__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_gt__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_gt__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a > b ? 1 : 0);
                break;
            }

            case OP_I32_LE: {
                u8 slot_a = unpack_op_i32_le__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_le__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_le__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a <= b ? 1 : 0);
                break;
            }

            case OP_I32_GE: {
                u8 slot_a = unpack_op_i32_ge__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_ge__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_ge__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a >= b ? 1 : 0);
                break;
            }

            case OP_I32_NE: {
                u8 slot_a = unpack_op_i32_ne__slot_a(*current_isns);
                u8 slot_b = unpack_op_i32_ne__slot_b(*current_isns);
                u8 slot_out = unpack_op_i32_ne__slot_out(*current_isns);
                i32 a = vm_get_slot_i32(vm, slot_a);
                i32 b = vm_get_slot_i32(vm, slot_b);
                vm->slots[vm->stack_top + slot_out] = Variant32::from_int(a != b ? 1 : 0);
                break;
            }

            case OP_I32_INC: {
                u8 slot = unpack_op_i32_inc__slot(*current_isns);
                i32 current_value = vm_get_slot_i32(vm, slot);
                vm->slots[vm->stack_top + slot] = Variant32::from_int(current_value + 1);
                break;
            }

            default: {
                std::string msg;
                msg += "vm_execute failed, unknown opcode:";
                msg += bytecode_opcode_to_string(opcode);
                vm_set_execution_failed(vm, msg.c_str());
                return;
            }
        }

        current_isns++;
    }
    
    // Finished
    vm->pc = (u32)(current_isns - isns);
}

void vm_resize_stack(VM* vm, u32 size) {
    vm->slots.resize(size);
}

// Accessing input/output values
Variant32* vm_io_slot(VM* vm, u32 locals_idx) {
    u32 slot_index = vm->stack_top + locals_idx + 1;
    assert(slot_index < vm->slots.size());
    return &vm->slots[slot_index];
}

void vm_io_set_u32(VM* vm, u32 locals_idx, u32 value) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::from_u32(value);
}

void vm_io_set_i32(VM* vm, u32 locals_idx, i32 value) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::from_int(value);
}

void vm_io_set_f32(VM* vm, u32 locals_idx, f32 value) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::from_float(value);
}

void vm_io_set_symbol(VM* vm, u32 locals_idx, SymbolId value) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::from_symbol(value);
}

void vm_io_set_string_id(VM* vm, u32 locals_idx, u32 string_id) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::from_string_id(string_id);
}

void vm_io_set_heap_ptr(VM* vm, u32 locals_idx, void* ptr) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::from_heap_ptr(ptr);
}

void vm_io_set_function_def(VM* vm, u32 locals_idx, BlockId block_id) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::function_def(block_id);
}

void vm_io_set_none(VM* vm, u32 locals_idx) {
    Variant32* slot = vm_io_slot(vm, locals_idx);
    *slot = Variant32::None();
}

// Helper function to get i32 value from slot
i32 vm_get_slot_i32(VM* vm, u8 slot) {
    return vm->slots[vm->stack_top + slot].get_int32();
}

// Helper function to get Variant32 from slot
Variant32 vm_get_slot(VM* vm, u8 slot) {
    return vm->slots[vm->stack_top + slot];
}

// Helper function to get SymbolId from slot
SymbolId vm_get_slot_sym(VM* vm, u8 slot) {
    return vm->slots[vm->stack_top + slot].get_symbol_id();
}

// Reserve frame size by resizing the VM's slot array
void vm_reserve_frame_size(VM* vm, u32 frame_size) {
    if (vm->slots.size() < frame_size) {
        vm->slots.resize(frame_size);
    }
}

// Prepare VM for entry at a specific block
void vm_prepare_entry(VM* vm, Bytecode* bytecode, BlockId block_id) {
    auto it = bytecode->entry_points.find(block_id);
    if (it != bytecode->entry_points.end()) {
        EntryPoint entry_point = it->second;
        vm->pc = entry_point.entry_pc;
        vm_reserve_frame_size(vm, entry_point.frame_size);
        
        // Set up top-level frame header at slot 0
        // For the top level, return_address = 0 (no caller) and prev_frame_size = 0
        vm->stack_top = 0;
        u32 top_level_frame_header = vm_pack_frame_header(0, 0);
        vm->slots[0] = Variant32::from_u32(top_level_frame_header);
    }
}
