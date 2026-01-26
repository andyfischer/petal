#include "program/program_building.h"
#include "globals/global_state.h"
#include "program/block.h"

// Forward declaration of helper function
GlobalState* get_active_global_state();

Term* create_term(Program* program, Block* block, const TermRef& func, const std::vector<TermRef>& inputs) {
    Term* term = block->add_term();
    term->func = func;
    term->inputs = inputs;
    return term;
}

// Helper function to create a term with a const value
Term* create_value_term(Program* program, Block* block, const Variant32& value) {
    Term* term = block->add_term();

    term->const_value_pos = block->const_data.alloc_variant_32(value);
    term->func = TermRef::from_native_function_id(NativeFunctionId::Value);

    return term;
}

// Helper function to create an input term
Term* create_input_term(Program* program, Block* block) {
    Term* term = block->add_term();

    term->func = TermRef::from_native_function_id(NativeFunctionId::Input);
    
    return term;
}