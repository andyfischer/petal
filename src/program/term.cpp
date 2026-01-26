#include <stdexcept>
#include "program/term.h"
#include "program/program.h"
#include "variant/variant.h"
#include "program/term_id.h"
#include "program/block.h"
#include "program/term_ref.h"
#include "program/find_by_name.h"

Term::Term(Block* parent_block) : 
    parent_block(parent_block),
    nested_block(nullptr),
    term_id(TermId::None()),
    name_id(0),
    block_position(0),
    func(TermRef::None()),
    const_value_pos(0) {}

TermRef Term::get_input(size_t index) const {
    if (index >= inputs.size()) {
        throw std::runtime_error("Input index out of bounds");
    }
    
    return inputs[index];
}

size_t Term::input_count() const {
    return inputs.size();
}

TermRef Term::as_ref() const {
    TermRef ref;
    ref.type = TermRefType::TermIdRef;
    ref.term_id = term_id;
    return ref;
}

TermRef Term::add_input(const TermRef& input) {
    inputs.push_back(input);
    return input;
}

void Term::set_inputs(const std::vector<TermRef>& inputs) {
    this->inputs = inputs;
}

void Term::set_name(SymbolId name_id) {
    parent_block->set_term_name(this, name_id);
}

bool Term::has_fixed_value() const {
    return const_value_pos != 0;
}

Variant32* Term::get_fixed_value() const {
    if (!has_fixed_value()) {
        return nullptr;
    }

    return parent_block->const_data.get_variant_32(const_value_pos);
}

Term* Term::resolve_input_term(int input_index) const {
    Program* program = parent_block->parent_program;
    TermRef input_ref = get_input(input_index);
    return resolve_term_ref(program, input_ref, this);
}

Block* Term::add_nested_block() {
    if (nested_block != nullptr) {
        // Already has a nested block
        return nested_block;
    }
    
    Program* program = parent_block->parent_program;
    nested_block = program->add_block();
    
    // Set up the parent-child relationship
    nested_block->parent_term = this->term_id;
    
    return nested_block;
}