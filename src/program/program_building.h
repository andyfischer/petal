#pragma once

#include "standard_headers.h"
#include "program/term.h"
#include "program/term_ref.h"
#include "variant/variant.h"
#include "program/program.h"
#include <vector>
#include <string>

// Forward declarations
struct GlobalState;

// Helper function to create a term with function and inputs
Term* create_term(Program* program, Block* block, const TermRef& func, const std::vector<TermRef>& inputs);

// Helper function to create a term with a fixed value
Term* create_value_term(Program* program, Block* block, const Variant32& value);

// Helper function to create an input term
Term* create_input_term(Program* program, Block* block);
