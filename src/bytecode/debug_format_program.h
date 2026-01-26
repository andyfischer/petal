#pragma once

#include "standard_headers.h"
#include <string>

// Forward declarations
struct GlobalState;
struct Program;
struct Block;
struct TermId;
struct TermRef;
struct Variant32;

void write_term_id_ref(std::ostream& os, const TermId& location, const TermId& term_id);

// Format all blocks in the program for debugging
void debug_format_program(std::ostream& os, Program* program);
