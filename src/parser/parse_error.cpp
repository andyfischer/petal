#include <iostream>
#include "parser/parse_error.h"
#include "parser/parse_steps.h"
#include "parser/parse_token_iterator.h"
#include "parser/parse_context.h"

void ErrorContext::while_parsing(const char* context_text) {
    context = context_text;
}

TermRef syntax_error(ParseContext* context, const char* message) {
    // Print error message for now - TODO is to improve this reporting.
    std::cerr << "Syntax error: " << message << std::endl;
    
    // If we have a token, show its position
    if (const FoundToken* token = context->it->next(0)) {
        std::cerr << "At line " << token->line_start << ", column " << token->col_start << std::endl;
        std::cerr << "Near token: " << context->it->next_text(0) << std::endl;
    }
    
    return TermRef::None();
}

TermRef syntax_error_unexpected_next_token(ParseContext* context, const ErrorContext* error_context) {
    std::string message = "Unexpected token";
    
    if (!error_context->context.empty()) {
        message += " while parsing " + error_context->context;
    }
    
    if (const FoundToken* token = context->it->next(0)) {
        message += ": '" + context->it->next_text(0) + "'";
    } else {
        message += " (end of input)";
    }
    
    return syntax_error(context, message.c_str());
}
