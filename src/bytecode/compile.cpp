#pragma once

#include "standard_headers.h"
#include "program/block.h"
#include "bytecode/bytecode.h"
#include "bytecode/bytecode_encoding.h"
#include "runtime/native_funcs.h"
#include "program/program.h"
#include "globals/global_state.h"
#include "program/term.h"
#include "program/term_ref.h"
#include "variant/variant.h"
#include "variant/variant_debug.h"
#include "program/find_by_name.h"
#include "bytecode/debug_format_program.h"
#include <sstream>

using Slot = uint32_t;

struct BytecodeBuilder {
    Program* program;
    Block* block;

    Slot locals_count;
    Slot ra_local_idx;
    std::vector<Instruction> isns;
    std::vector<uint8_t> const_data;
    std::unordered_map<TermId, Slot> term_to_output_slot;
    
    BytecodeBuilder(Program* program, Block* block)
        : program(program), block(block), locals_count(0), ra_local_idx(0) {
    }
    
    Slot take_next_local_idx() {
        Slot slot = locals_count;
        locals_count++;
        return slot;
    }
    
    uint16_t add_const_str(const std::string& str) {
        uint16_t offset = const_data.size();
        
        // Store the string in const_data as a null-terminated string
        for (char c : str) {
            const_data.push_back(c);
        }
        const_data.push_back(0); // null terminator
        
        return offset;
    }

    void add_instruction(Instruction ins) {
        isns.push_back(ins);
    }

    void allocate_empty_slot() {
        take_next_local_idx();
    }
    
    Slot allocate_slot_for_term_output(Term* term) {
        Slot slot = take_next_local_idx();
        term_to_output_slot[term->term_id] = slot;
        return slot;
    }

    bool has_output_slot_for_term(Term* term) {
        auto it = term_to_output_slot.find(term->term_id);
        return it != term_to_output_slot.end();
    }
    
    Slot get_term_output_slot(Term* term) {
        if (term->term_id.block_id != this->block->block_id) {
            // TODO: Add proper error handling
            assert(false && "get_term_output_local_idx usage error: Term belongs to a different block");
        }
        
        auto it = term_to_output_slot.find(term->term_id);
        if (it == term_to_output_slot.end()) {
            // TODO: Add proper error handling
            assert(false && "Output slot not found for term ID");
        }
        return it->second;
    }

    Slot get_or_allocate_term_output_slot(Term* term) {
        auto found = term_to_output_slot.find(term->term_id);
        if (found == term_to_output_slot.end()) {
            return allocate_slot_for_term_output(term);
        }
        return found->second;
    }

    Slot get_term_input_slot(Term* term, int index) {
        Term* input_term = term->resolve_input_term(index);
        if (input_term == nullptr) {    
            assert(false && "get_term_input_slot: Input term not found");
        }

        if (!has_output_slot_for_term(input_term)) {
            std::stringstream msg;
            msg << "Failed to find a slot for an input value: ";
            write_term_id_ref(msg, term->term_id, input_term->term_id);
            msg << " (input #" << index << " of term ";
            write_term_id_ref(msg, term->term_id, term->term_id);
            add_compile_error(msg.str());
            return 0;
        }

        return get_term_output_slot(input_term);
    }
    
    void add_compile_error(const std::string& error_message) {
        uint16_t error_idx = add_const_str(error_message);
        add_instruction(pack_op_compile_error(error_idx));
    }
    
    void add_comment(const std::string& comment_text) {
        uint16_t comment_idx = add_const_str(comment_text);
        add_instruction(pack_op_comment(comment_idx));
    }
};

void compile_value_term(BytecodeBuilder* builder, Term* term) {
    if (!term->has_fixed_value()) {
        builder->add_compile_error("push_const_value: usage error: term has no fixed value");
    }
    
    Variant32* value = term->get_fixed_value();
    
    switch (value->type) {
        case VariantType::I32: {
            Slot slot = builder->allocate_slot_for_term_output(term);
            builder->add_instruction(pack_op_const_i16(slot, value->get_int32()));
            break;
        }

        case VariantType::Symbol: {
            Slot slot = builder->allocate_slot_for_term_output(term);
            builder->add_instruction(pack_op_const_u16_sym(slot, value->get_symbol_id()));
            break;
        }

        case VariantType::FunctionDef: {
            // No const value needed, functions are resolved statically.
            break;
        }


        case VariantType::Float32:
        default: {
            std::stringstream err_msg;
            err_msg << "Unhandled type in compile_value_term: ";
            debug_format_variant32(err_msg, *value);
            builder->add_compile_error(err_msg.str());
            break;
        }
    }
}

void compile_native_function_call(BytecodeBuilder* builder, Term* term, NativeFunctionId func) {

    switch (func) {
        case NativeFunctionId::Value:
            compile_value_term(builder, term);
            return;

        case NativeFunctionId::Input:
            // Input terms are handled in the compile_input_saves function.
            return;

        case NativeFunctionId::Add: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_add(a, b, out));
            break;
        }

        case NativeFunctionId::Sub: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_sub(a, b, out));
            break;
        }

        case NativeFunctionId::Mult: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_mult(a, b, out));
            break;
        }

        case NativeFunctionId::Div: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_div_s(a, b, out));
            break;
        }

        // Comparison operations
        case NativeFunctionId::Eq: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_eq(a, b, out));
            break;
        }

        case NativeFunctionId::Ne: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_ne(a, b, out));
            break;
        }

        case NativeFunctionId::Lt: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_lt(a, b, out));
            break;
        }

        case NativeFunctionId::Gt: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_gt(a, b, out));
            break;
        }

        case NativeFunctionId::Le: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_le(a, b, out));
            break;
        }

        case NativeFunctionId::Ge: {
            Slot a = builder->get_term_input_slot(term, 0);
            Slot b = builder->get_term_input_slot(term, 1);
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_i32_ge(a, b, out));
            break;
        }

        // Control flow operations
        case NativeFunctionId::Jump: {
            // Jump takes an address as input (typically from a constant)
            Slot address_slot = builder->get_term_input_slot(term, 0);
            // TODO: For now, generate a placeholder jump. In practice, this will need
            // special handling during compilation to resolve jump addresses.
            builder->add_compile_error("Jump operation needs special compilation handling");
            break;
        }

        case NativeFunctionId::JumpIfTrue: {
            // JumpIfTrue takes condition and address as inputs
            Slot condition_slot = builder->get_term_input_slot(term, 0);
            Slot address_slot = builder->get_term_input_slot(term, 1);
            // TODO: Like Jump, this needs special handling to resolve addresses
            builder->add_compile_error("JumpIfTrue operation needs special compilation handling");
            break;
        }

        case NativeFunctionId::JumpIfFalse: {
            // JumpIfFalse takes condition and address as inputs
            Slot condition_slot = builder->get_term_input_slot(term, 0);
            Slot address_slot = builder->get_term_input_slot(term, 1);
            // TODO: Like Jump, this needs special handling to resolve addresses
            builder->add_compile_error("JumpIfFalse operation needs special compilation handling");
            break;
        }

        // For loop support
        case NativeFunctionId::Inc: {
            Slot slot = builder->get_term_input_slot(term, 0);
            builder->add_instruction(pack_op_i32_inc(slot));
            // Inc modifies the slot in place, but we might want to also output the result
            Slot out = builder->get_or_allocate_term_output_slot(term);
            builder->add_instruction(pack_op_copy(slot, out));
            break;
        }

        default: {
            builder->add_compile_error("Unhandled native function");
            return;
        }
    }
}

void compile_function_call(BytecodeBuilder* builder, Term* call_term, Term* func_def_term) {
    if (!func_def_term->has_fixed_value() || !func_def_term->get_fixed_value()->is_function_def()) {
        std::string msg = "Not a function: " + func_def_term->func.to_debug_str();
        builder->add_compile_error(msg);
        return;
    }

    BlockId function_def_block = func_def_term->get_fixed_value()->get_block_id();
    
    // Count the number of input arguments
    // TODO: Allocate enough space for outputs too.
    size_t arg_count = call_term->input_count();
    
    // Calculate the stack frame size needed (at least for arguments)
    // Add 1 for the frame header that will be stored at local:0
    u8 frame_size = arg_count + 1;
    
    // Reserve slots for the new stack frame
    // The frame will start after the current local variables
    builder->add_instruction(pack_op_reserve_slots(frame_size));
    
    // Copy input arguments to the new frame slots
    // New frame slots start at (current locals_count)
    // Slot 0 in new frame will be the frame header, so args start at slot 1
    for (size_t i = 0; i < arg_count; i++) {
        Slot input_slot = builder->get_term_input_slot(call_term, i);
        Slot new_frame_slot = builder->locals_count + 1 + i;  // +1 to skip frame header slot
        builder->add_instruction(pack_op_copy(input_slot, new_frame_slot));
    }
    
    // Trigger the function call
    // func_address is the block_id of the function definition
    u16 func_address = static_cast<u16>(function_def_block);
    builder->add_instruction(pack_op_call(func_address, frame_size));
    
    // After the call returns, the output will be in the first slot of what was the new frame
    // Allocate an output slot for this call term's result
    Slot output_slot = builder->allocate_slot_for_term_output(call_term);
    
    // Copy the return value from the new frame to our output slot
    Slot return_value_slot = builder->locals_count;  // First slot of the returned frame
    builder->add_instruction(pack_op_copy(return_value_slot, output_slot));
}

void compile_term(BytecodeBuilder* builder, Term* term) {

    // Handle native functions.
    if (term->func.type == TermRefType::NativeFunctionRef) {
        compile_native_function_call(builder, term, term->func.native_function_id);
        return;
    }

    Term* func_term = resolve_term_ref(builder->program, term->func, term);

    if (func_term != nullptr) {
        if (func_term->has_fixed_value() && func_term->get_fixed_value()->is_function_def()) {
            compile_function_call(builder, term, func_term);
        } else {
            std::string msg = "Not a function: " + term->func.to_debug_str();
            builder->add_compile_error(msg);
        }
        return;
    }

    // Try looking up the name as a global function
    if (term->func.type == TermRefType::NameRef) {
        SymbolId name_id = term->func.name_id;
        NativeFunctionId native_func_id = get_active_global_state()->get_native_function_by_name(name_id);

        if (native_func_id != NativeFunctionId::None) {
            compile_native_function_call(builder, term, native_func_id);
            return;
        }
    }

    // Handle calls that are implemented using a host function
    if (term->func.type == TermRefType::NameRef) {
        SymbolId name_id = term->func.name_id;
        GlobalState* gs = get_active_global_state();
        HostFunctionEntry* host_entry = gs->lookup_host_function(name_id);
        
        if (host_entry) {
            // Allocate a slot for the function symbol
            Slot func_symbol_slot = builder->take_next_local_idx();
            
            // Store the function symbol in a slot
            builder->add_instruction(pack_op_const_u16_sym(func_symbol_slot, name_id));
            
            // Call the host function with the expected argument count
            builder->add_instruction(pack_op_call_host(func_symbol_slot, host_entry->expected_argc));
            return;
        }
    }

    // Look up by name.
    Term* found_func = resolve_term_ref(builder->program, term->func, term);

    if (found_func != nullptr) {
        compile_function_call(builder, term, found_func);
        return;
    }

    std::string msg = "Function not found: " + term->func.to_debug_str();
    builder->add_compile_error(msg);
}

void allocate_input_term_slots(BytecodeBuilder* builder, Block* block) {
    for (Term* term : block->terms.items) {
        if (term->func.type == TermRefType::NativeFunctionRef && 
            term->func.native_function_id == NativeFunctionId::Input) {

            builder->allocate_slot_for_term_output(term);
        } else {
            break;
        }
    }
}

void compile_block(BytecodeBuilder* builder, Block* block) {
    std::stringstream debug_comment;
    debug_comment << "start block: " << block->block_id;
    builder->add_comment(debug_comment.str());

    // Allocate one empty slot to hold the frame header.
    builder->allocate_empty_slot();
    
    // Allocate slots for inputs
    // TODO: Allocate enough slots to hold inputs and outputs.
    allocate_input_term_slots(builder, block);
    
    for (Term* term : block->terms.items) {
        compile_term(builder, term);
    }
    
    builder->add_instruction(pack_op_return(0));
}

Bytecode* compile_program(Program* program) {
    Bytecode* program_bytecode = new Bytecode();

    for (Block* block : program->blocks.items) {
        BytecodeBuilder builder(program, block);
        auto const_data_existing_size = program_bytecode->const_data.size();

        // Store entry point for this block
        EntryPoint entry_point;
        entry_point.entry_pc = program_bytecode->isns.size();
        entry_point.frame_size = 0; // Will be updated after compilation

        // Compile the block
        compile_block(&builder, block);
        
        // Update frame size after compilation
        entry_point.frame_size = builder.locals_count;
        program_bytecode->entry_points[block->block_id] = entry_point;

        // Combine into final bytecode
        for (Instruction isn : builder.isns) {
            // Check if this is a compile error or comment instruction that needs const_data remapping
            u8 opcode = unpack_opcode(isn);
            if (opcode == OP_COMPILE_ERROR && const_data_existing_size > 0) {
                // Remap the error index to account for existing const_data
                u16 old_error_idx = unpack_op_compile_error__error_idx(isn);
                u16 new_error_idx = old_error_idx + const_data_existing_size;
                isn = pack_op_compile_error(new_error_idx);
            } else if (opcode == OP_COMMENT && const_data_existing_size > 0) {
                // Remap the comment index to account for existing const_data
                u16 old_comment_idx = unpack_op_comment__comment_idx(isn);
                u16 new_comment_idx = old_comment_idx + const_data_existing_size;
                isn = pack_op_comment(new_comment_idx);
            }
            
            program_bytecode->isns.push_back(isn);
        }

        for (uint8_t const_byte : builder.const_data) {
            program_bytecode->const_data.push_back(const_byte);
        }
    }

    return program_bytecode;
}
