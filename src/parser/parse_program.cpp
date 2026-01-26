#include "parser/parse_program.h"
#include "parser/parse_token_iterator.h"
#include "program/program.h"
#include "parser/parse_steps.h"
#include "globals/global_state.h"
#include "program/block.h"
#include <iostream>
#include "parser/parse_context.h"

Program* parse_program(const char* source, const ParseProgramOptions& options) {
    // Create a token iterator from the source code
    TokenIterator it(source);
    
    Program* program = new Program();
    
    // Create a root block for the program
    Block* root_block = program->add_block();
    
    // Create a parse context with references to the necessary objects
    ParseContext context;
    context.env = get_active_global_state();
    context.program = program;
    context.block = root_block;
    context.it = &it;
    if (options.stdout_trace) {
        context.trace_output = &std::cout;
    }
    parse_statement_list(&context);
    
    return program;
}