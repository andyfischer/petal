#include "program/find_by_name.h"
#include <functional>

IteratorControl iterate_preceding_terms(Program* program, const Term* position, std::function<IteratorControl(Term*)> callback);

void iterate_visible_terms(Program* program, Term* position, std::function<IteratorControl(Term*)> callback) {
    if (position->parent_block == nullptr) {
        return;
    }

    Block* block = position->parent_block;

    IteratorControl control = iterate_preceding_terms(program, position, callback);
    if (control == iterator_break) {
        return;
    }

    // Iterate through the parent block.
    Term* parent_of_block = position->parent_block->get_parent_term(program);
    if (parent_of_block == nullptr) {
        return;
    }

    iterate_visible_terms(program, parent_of_block, callback);
}

// iterate_preceding_terms - Iterates through terms in the same block that are before 'position'.
IteratorControl iterate_preceding_terms(Program* program, const Term* position, std::function<IteratorControl(Term*)> callback) {
    if (position->parent_block == nullptr) {
        return iterator_break;
    }
    
    Block* block = position->parent_block;
    auto term_position = position->term_id.term_local_id;

    // Iterate through the preceding terms in the block.
    // Note: LookupTable uses 1-based indexing, so we iterate from term_position-1 down to 1
    for (int i = term_position - 1; i >= 1; i--) {
        Term* term = block->get_term_by_local_id(i);
        IteratorControl control = callback(term);
        if (control == iterator_break) {
            return control;
        }
    }

    return 0;
}

Term* find_term_by_name(Program* program, SymbolId name_id, const Term* position) {
    Term* block_term_with_name = position->parent_block->get_last_term_by_name(name_id);

    if (block_term_with_name == nullptr) {
        // Failed to find the name in the current block.

        Term* parent_of_block = position->parent_block->get_parent_term(program);
        if (parent_of_block == nullptr) {
            // Nothing left to search.
            return nullptr;
        }

        // Recursively search for the name in the parent blocks.
        return find_term_by_name(program, name_id, parent_of_block);
    }

    // Check if this name is actually visible from thi position.
    if (block_term_with_name->term_id.term_local_id < position->block_position) {
        // This name is visible from the position.
        return block_term_with_name;
    }

    // Slower search - Check if this block has another term with the same name
    // which is visible.
    Term* found_term = nullptr;
    iterate_preceding_terms(program, position, [&](Term* term) -> u32 {
        if (term->name_id == name_id) {
            found_term = term;
            return iterator_break;
        }
        return 0;
    });

    return found_term;
}

Term* resolve_term_ref(Program* program, TermRef ref, const Term* position) {
    switch (ref.type) {
    case TermRefType::NameRef:
        return find_term_by_name(program, ref.name_id, position);

    case TermRefType::TermIdRef:
        return program->get_term(ref.term_id);

    case TermRefType::NativeFunctionRef:
        throw std::runtime_error("resolve_term_ref: does not support native function refs");

    default:
        throw std::runtime_error("resolve_term_ref: unhandled case");
    }
}