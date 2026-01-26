#pragma once

#include "standard_headers.h"
#include "program/term_ref.h"
#include <string>

// Forward declarations
struct ParseContext;

// Error context class to keep track of where we are in parsing
struct ErrorContext {
    std::string context;
    
    ErrorContext() {}
    
    void while_parsing(const char* context_text);
};

// Functions for handling syntax errors
TermRef syntax_error(ParseContext* context, const char* message);
TermRef syntax_error_unexpected_next_token(ParseContext* context, const ErrorContext* error_context);