#include "parser/parse_context.h"
#include "parser/parse_token_iterator.h"

ParseContext::ParseContext() {
    this->depth = 0;
    this->trace_output = nullptr;
    this->trace_last_printed_token = 0;
    this->trace_needs_newline = true;
}

void ParseContext::trace_start(const char* step_name) {
    if (this->trace_output) {
        if (this->trace_needs_newline) {
            *this->trace_output << std::endl;
            this->trace_needs_newline = false;
        }
        *this->trace_output << std::string(this->depth, ' ') << "<" << step_name << ">";
        this->trace_update_printend_token(this->it->position);
        this->trace_needs_newline = true;
    }
    this->depth++;
}

void ParseContext::trace_end(const char* step_name) {
    this->depth--;
    if (this->trace_output) {
        this->trace_update_printend_token(this->it->position);
        if (this->trace_needs_newline) {
            *this->trace_output << std::endl;
            this->trace_needs_newline = false;
        }
        *this->trace_output << std::string(this->depth, ' ') << "</" << step_name << ">" << std::endl;
    }
}

void ParseContext::trace_update_printend_token(u32 position) {
    for (u32 i = this->trace_last_printed_token; i < position; i++) {
        *this->trace_output << this->it->token_text(&this->it->tokens[i]);
    }
    this->trace_last_printed_token = position;
}