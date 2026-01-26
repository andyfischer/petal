#pragma once

#include "standard_headers.h"
#include "program/block.h"
#include "utils/lookup_table.h"

// Forward declarations
struct GlobalState;
struct Term;

struct Program {
    LookupTable<Block*> blocks;

    Program();
    ~Program();

    // Add a new block to the program
    Block* add_block();

    // Get a block by ID
    Block* get_block(BlockId block_id);
    const Block* get_block(BlockId block_id) const;

    // Get a term by ID
    Term* get_term(const TermId& term_id);
};

