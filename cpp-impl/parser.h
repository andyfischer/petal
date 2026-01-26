#pragma once

#include "lexer.h"
#include "ast.h"

struct Parser {
    Lexer lexer;
    Token current;
    Token previous;
    bool had_error;
    bool panic_mode;
    char error_message[256];
};

void parser_init(Parser* parser, const char* source);
ASTNode* parser_parse(Parser* parser);
bool parser_had_error(Parser* parser);
const char* parser_error_message(Parser* parser);
