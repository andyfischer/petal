#pragma once

#include "standard_headers.h"
#include <ostream>

struct Program;
struct Block;
struct GlobalState;
struct TokenIterator;

// Context for parsing operations
struct ParseContext {
    GlobalState* env;
    Program* program;
    Block* block;
    TokenIterator* it;
    u32 depth;

    // If defined, the parse steps will print a log to this stream.
    std::ostream* trace_output;
    u32 trace_last_printed_token;
    bool trace_needs_newline;

    ParseContext();

    void trace_start(const char* step_name);
    void trace_end(const char* step_name);
    void trace_update_printend_token(u32 position);
};