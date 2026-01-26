#pragma once

#include "standard_headers.h"
#include <unordered_map>
#include <cassert>
#include "utils/lookup_table.h"
#include "program/term_id.h"
#include "runtime/const_data_buffer.h"

struct Term;
struct NameMap;
struct Variant;
struct Program;

struct Block {
    BlockId block_id;
    TermId parent_term;
    Program* parent_program;
    
    LookupTable<Term*> terms;
    ConstDataBuffer const_data;

    std::unordered_map<SymbolId, TermLocalId> name_id_to_term;
    
    Block(Program* parent_program);
    ~Block();
    
    // Add a new term to this block.
    Term* add_term();
    
    // Get a term by id.
    Term* get_term(const TermId& term_id);
    Term* get_term_by_local_id(TermLocalId local_id);

    Term* get_parent_term(Program* program);
    
    // Set a term's name in this block.
    void set_term_name(Term* term, SymbolId name_id);

    // Fetch the last term with this name. Return null if none found.
    Term* get_last_term_by_name(SymbolId name_id);
};
