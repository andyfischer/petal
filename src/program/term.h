#pragma once

#include "standard_headers.h"
#include <vector>
#include <string>
#include <optional>
#include "variant/variant.h"
#include "program/term_ref.h"

// Forward declarations
struct Block;
struct Variant;

struct Term {
    TermId term_id;
    SymbolId name_id;
    Block* parent_block;
    Block* nested_block;
    
    // The term's position in the block's execution order.
    size_t block_position;
    
    TermRef func;
    std::vector<TermRef> inputs;
    
    // The position of the term's constant value in the block's constant data buffer (if any)
    u32 const_value_pos;

    Term(Block* parent_block);
    
    TermRef get_input(size_t index) const;
    size_t input_count() const;


    TermRef as_ref() const;
    
    bool has_fixed_value() const;
    Variant32* get_fixed_value() const;

    Term* resolve_input_term(int input_index) const;

    // Building
    void set_name(SymbolId name_id);
    TermRef add_input(const TermRef& input);
    void set_inputs(const std::vector<TermRef>& inputs);
    Block* add_nested_block();
};
