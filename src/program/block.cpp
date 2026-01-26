#include <cassert>
#include <stdexcept>
#include "program/block.h"
#include "program/term.h"
#include "program/term_id.h"
#include "program/program.h"

Block::Block(Program* parent_program) : 
    parent_program(parent_program),
    parent_term(TermId::None()) {}

Block::~Block() {
    for (Term* term : terms.items) {
        delete term;
    }
}

Term* Block::add_term() {
    Term* term = new Term(this);
    terms.add(term);

    term->term_id = TermId(block_id, terms.last_id());
    term->block_position = terms.last_id();
    
    return term;
}

Term* Block::get_term(const TermId& term_id) {
    if (term_id.block_id != block_id) {
        throw std::runtime_error("Block.get_term: Term ID does not match this block ID");
    }

    return terms[term_id.term_local_id];
}

Term* Block::get_term_by_local_id(TermLocalId local_id) {
    return terms[local_id];
}

Term* Block::get_parent_term(Program* program) {
    return program->get_term(parent_term);
}

void Block::set_term_name(Term* term, SymbolId name_id) {
    term->name_id = name_id;
    name_id_to_term[name_id] = term->block_position;
}

Term* Block::get_last_term_by_name(SymbolId name_id) {
    auto it = name_id_to_term.find(name_id);
    if (it == name_id_to_term.end()) {
        return nullptr;
    }
    return terms[it->second];
}