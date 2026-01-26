#include <cassert>
#include <stdexcept>
#include "program/program.h"
#include "globals/global_state.h"
#include "program/term.h"

Program::Program() {
}

Program::~Program() {
    for (Block* block : blocks.items) {
        delete block;
    }
}

Block* Program::add_block() {
    Block* block = new Block(this);
    blocks.add(block);
    block->block_id = blocks.last_id();
    return block;
}

Block* Program::get_block(BlockId block_id) {
    return blocks[block_id];
}

Term* Program::get_term(const TermId& term_id){
    if (term_id.block_id == 0) {
        return nullptr;
    }

    Block* block = get_block(term_id.block_id);
    return block->get_term(term_id);
}


