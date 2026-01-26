#include "parser.h"
#include <cstdlib>
#include <cstdio>
#include <cstring>

// Forward declarations
static ASTNode* parse_expression(Parser* parser);
static ASTNode* parse_statement(Parser* parser);
static ASTNode* parse_block(Parser* parser);

void parser_init(Parser* parser, const char* source) {
    lexer_init(&parser->lexer, source);
    parser->had_error = false;
    parser->panic_mode = false;
    parser->error_message[0] = '\0';
    parser->current = lexer_next_token(&parser->lexer);
}

static void error_at(Parser* parser, Token* token, const char* message) {
    if (parser->panic_mode) return;
    parser->panic_mode = true;
    parser->had_error = true;

    snprintf(parser->error_message, sizeof(parser->error_message),
             "[line %d, col %d] Error at '%.*s': %s",
             token->line, token->column, token->length, token->start, message);
}

static void error(Parser* parser, const char* message) {
    error_at(parser, &parser->previous, message);
}

static void error_at_current(Parser* parser, const char* message) {
    error_at(parser, &parser->current, message);
}

static void advance(Parser* parser) {
    parser->previous = parser->current;

    for (;;) {
        parser->current = lexer_next_token(&parser->lexer);
        if (parser->current.type != TOK_ERROR) break;
        error_at_current(parser, parser->current.start);
    }
}

static bool check(Parser* parser, TokenType type) {
    return parser->current.type == type;
}

static bool match(Parser* parser, TokenType type) {
    if (!check(parser, type)) return false;
    advance(parser);
    return true;
}

static void consume(Parser* parser, TokenType type, const char* message) {
    if (parser->current.type == type) {
        advance(parser);
        return;
    }
    error_at_current(parser, message);
}

static char* copy_token_string(Token* token) {
    // Handle string literals - strip quotes
    if (token->type == TOK_STRING) {
        const char* start = token->start + 1;
        int length = token->length - 2;

        // Check for triple quotes
        if (token->length >= 6 && token->start[0] == '"' &&
            token->start[1] == '"' && token->start[2] == '"') {
            start = token->start + 3;
            length = token->length - 6;
        }

        char* str = (char*)malloc(length + 1);
        memcpy(str, start, length);
        str[length] = '\0';
        return str;
    }

    // Handle symbols - strip leading colon
    if (token->type == TOK_SYMBOL) {
        const char* start = token->start + 1;
        int length = token->length - 1;
        char* str = (char*)malloc(length + 1);
        memcpy(str, start, length);
        str[length] = '\0';
        return str;
    }

    char* str = (char*)malloc(token->length + 1);
    memcpy(str, token->start, token->length);
    str[token->length] = '\0';
    return str;
}

// Precedence levels (higher = tighter binding)
typedef enum {
    PREC_NONE,
    PREC_ASSIGNMENT,   // =
    PREC_PIPE,         // @
    PREC_OR,           // ||
    PREC_AND,          // &&
    PREC_EQUALITY,     // == !=
    PREC_COMPARISON,   // < > <= >=
    PREC_TERM,         // + -
    PREC_FACTOR,       // * / %
    PREC_UNARY,        // ! -
    PREC_POWER,        // **
    PREC_CALL,         // . () []
    PREC_PRIMARY
} Precedence;

static ASTNode* parse_precedence(Parser* parser, Precedence precedence);

// Primary expressions
static ASTNode* parse_number(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node;

    if (token.type == TOK_INT) {
        node = ast_alloc(AST_INT_LITERAL, token.line, token.column);
        node->as.int_value = token.int_value;
    }
    else {
        node = ast_alloc(AST_FLOAT_LITERAL, token.line, token.column);
        node->as.float_value = token.float_value;
    }
    return node;
}

static ASTNode* parse_string(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_STRING_LITERAL, token.line, token.column);
    node->as.string_value = copy_token_string(&token);
    return node;
}

static ASTNode* parse_symbol(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_SYMBOL_LITERAL, token.line, token.column);
    node->as.symbol_value = copy_token_string(&token);
    return node;
}

static ASTNode* parse_true(Parser* parser) {
    ASTNode* node = ast_alloc(AST_BOOL_LITERAL, parser->previous.line, parser->previous.column);
    node->as.bool_value = true;
    return node;
}

static ASTNode* parse_false(Parser* parser) {
    ASTNode* node = ast_alloc(AST_BOOL_LITERAL, parser->previous.line, parser->previous.column);
    node->as.bool_value = false;
    return node;
}

static ASTNode* parse_null(Parser* parser) {
    return ast_alloc(AST_NULL_LITERAL, parser->previous.line, parser->previous.column);
}

static ASTNode* parse_identifier(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_IDENTIFIER, token.line, token.column);
    node->as.identifier.name = copy_token_string(&token);
    return node;
}

static ASTNode* parse_grouping(Parser* parser) {
    ASTNode* expr = parse_expression(parser);
    consume(parser, TOK_RPAREN, "Expected ')' after expression");
    return expr;
}

static ASTNode* parse_array(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_ARRAY_LITERAL, token.line, token.column);
    node->as.array.elements = nullptr;

    if (!check(parser, TOK_RBRACKET)) {
        do {
            if (check(parser, TOK_RBRACKET)) break;
            ASTNode* element = parse_expression(parser);
            node->as.array.elements = ast_list_append(node->as.array.elements, element);
        } while (match(parser, TOK_COMMA) || (!check(parser, TOK_RBRACKET) && !check(parser, TOK_EOF)));
    }

    consume(parser, TOK_RBRACKET, "Expected ']' after array elements");
    return node;
}

static ASTNode* parse_object(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_OBJECT_LITERAL, token.line, token.column);
    node->as.object.fields = nullptr;

    if (!check(parser, TOK_RBRACE)) {
        do {
            if (check(parser, TOK_RBRACE)) break;

            // Key
            consume(parser, TOK_IDENTIFIER, "Expected property name");
            char* key = copy_token_string(&parser->previous);

            consume(parser, TOK_COLON, "Expected ':' after property name");

            // Value
            ASTNode* value = parse_expression(parser);

            node->as.object.fields = object_field_append(node->as.object.fields, key, value);
        } while (match(parser, TOK_COMMA) || (!check(parser, TOK_RBRACE) && !check(parser, TOK_EOF)));
    }

    consume(parser, TOK_RBRACE, "Expected '}' after object properties");
    return node;
}

static ParamNode* parse_params(Parser* parser) {
    ParamNode* params = nullptr;

    consume(parser, TOK_LPAREN, "Expected '(' before parameters");

    if (!check(parser, TOK_RPAREN)) {
        do {
            if (check(parser, TOK_RPAREN)) break;

            consume(parser, TOK_IDENTIFIER, "Expected parameter name");
            char* name = copy_token_string(&parser->previous);
            char* type_name = nullptr;

            // Optional type annotation
            if (match(parser, TOK_COLON)) {
                consume(parser, TOK_IDENTIFIER, "Expected type name");
                type_name = copy_token_string(&parser->previous);
            }

            params = param_list_append(params, name, type_name);
        } while (match(parser, TOK_COMMA) || (!check(parser, TOK_RPAREN) && !check(parser, TOK_EOF)));
    }

    consume(parser, TOK_RPAREN, "Expected ')' after parameters");
    return params;
}

static ASTNode* parse_lambda(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_LAMBDA, token.line, token.column);

    node->as.lambda.params = parse_params(parser);

    consume(parser, TOK_FAT_ARROW, "Expected '=>' after lambda parameters");

    if (check(parser, TOK_LBRACE)) {
        advance(parser);
        node->as.lambda.body = parse_block(parser);
    }
    else {
        node->as.lambda.body = parse_expression(parser);
    }

    return node;
}

static ASTNode* parse_unary(Parser* parser) {
    Token token = parser->previous;
    UnaryOp op;

    if (token.type == TOK_MINUS) {
        op = OP_NEG;
    }
    else {
        op = OP_NOT;
    }

    ASTNode* operand = parse_precedence(parser, PREC_UNARY);

    ASTNode* node = ast_alloc(AST_UNARY_OP, token.line, token.column);
    node->as.unary.op = op;
    node->as.unary.operand = operand;
    return node;
}

// Infix parsers
static ASTNode* parse_binary(Parser* parser, ASTNode* left, Precedence prec) {
    Token token = parser->previous;
    BinaryOp op;

    switch (token.type) {
        case TOK_PLUS:      op = OP_ADD; break;
        case TOK_MINUS:     op = OP_SUB; break;
        case TOK_STAR:      op = OP_MUL; break;
        case TOK_SLASH:     op = OP_DIV; break;
        case TOK_PERCENT:   op = OP_MOD; break;
        case TOK_STAR_STAR: op = OP_POW; break;
        case TOK_EQ:        op = OP_EQ;  break;
        case TOK_NE:        op = OP_NE;  break;
        case TOK_LT:        op = OP_LT;  break;
        case TOK_GT:        op = OP_GT;  break;
        case TOK_LE:        op = OP_LE;  break;
        case TOK_GE:        op = OP_GE;  break;
        case TOK_AND:       op = OP_AND; break;
        case TOK_OR:        op = OP_OR;  break;
        case TOK_AT:        op = OP_PIPE; break;
        default:
            error(parser, "Unknown binary operator");
            return left;
    }

    ASTNode* right = parse_precedence(parser, (Precedence)(prec + 1));

    ASTNode* node = ast_alloc(AST_BINARY_OP, token.line, token.column);
    node->as.binary.op = op;
    node->as.binary.left = left;
    node->as.binary.right = right;
    return node;
}

static ASTNode* parse_call(Parser* parser, ASTNode* callee) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_CALL, token.line, token.column);
    node->as.call.callee = callee;
    node->as.call.args = nullptr;

    if (!check(parser, TOK_RPAREN)) {
        do {
            if (check(parser, TOK_RPAREN)) break;
            ASTNode* arg = parse_expression(parser);
            node->as.call.args = ast_list_append(node->as.call.args, arg);
        } while (match(parser, TOK_COMMA) || (!check(parser, TOK_RPAREN) && !check(parser, TOK_EOF)));
    }

    consume(parser, TOK_RPAREN, "Expected ')' after arguments");
    return node;
}

static ASTNode* parse_index(Parser* parser, ASTNode* object) {
    Token token = parser->previous;
    ASTNode* index_expr = parse_expression(parser);
    consume(parser, TOK_RBRACKET, "Expected ']' after index");

    ASTNode* node = ast_alloc(AST_INDEX, token.line, token.column);
    node->as.index.object = object;
    node->as.index.index = index_expr;
    return node;
}

static ASTNode* parse_member(Parser* parser, ASTNode* object) {
    Token token = parser->previous;
    consume(parser, TOK_IDENTIFIER, "Expected property name after '.'");
    char* member = copy_token_string(&parser->previous);

    ASTNode* node = ast_alloc(AST_MEMBER, token.line, token.column);
    node->as.member.object = object;
    node->as.member.member = member;
    return node;
}

static Precedence get_precedence(TokenType type) {
    switch (type) {
        case TOK_ASSIGN:
        case TOK_PLUS_EQ:
        case TOK_MINUS_EQ:
        case TOK_STAR_EQ:
        case TOK_SLASH_EQ:
        case TOK_PERCENT_EQ:
            return PREC_ASSIGNMENT;
        case TOK_AT:
            return PREC_PIPE;
        case TOK_OR:
            return PREC_OR;
        case TOK_AND:
            return PREC_AND;
        case TOK_EQ:
        case TOK_NE:
            return PREC_EQUALITY;
        case TOK_LT:
        case TOK_GT:
        case TOK_LE:
        case TOK_GE:
            return PREC_COMPARISON;
        case TOK_PLUS:
        case TOK_MINUS:
            return PREC_TERM;
        case TOK_STAR:
        case TOK_SLASH:
        case TOK_PERCENT:
            return PREC_FACTOR;
        case TOK_STAR_STAR:
            return PREC_POWER;
        case TOK_LPAREN:
        case TOK_LBRACKET:
        case TOK_DOT:
            return PREC_CALL;
        default:
            return PREC_NONE;
    }
}

static ASTNode* parse_precedence(Parser* parser, Precedence precedence) {
    advance(parser);

    // Prefix
    ASTNode* left = nullptr;
    switch (parser->previous.type) {
        case TOK_INT:
        case TOK_FLOAT:
            left = parse_number(parser);
            break;
        case TOK_STRING:
            left = parse_string(parser);
            break;
        case TOK_SYMBOL:
            left = parse_symbol(parser);
            break;
        case TOK_TRUE:
            left = parse_true(parser);
            break;
        case TOK_FALSE:
            left = parse_false(parser);
            break;
        case TOK_NULL:
            left = parse_null(parser);
            break;
        case TOK_IDENTIFIER:
            left = parse_identifier(parser);
            break;
        case TOK_LPAREN:
            left = parse_grouping(parser);
            break;
        case TOK_LBRACKET:
            left = parse_array(parser);
            break;
        case TOK_LBRACE:
            left = parse_object(parser);
            break;
        case TOK_FN:
            left = parse_lambda(parser);
            break;
        case TOK_MINUS:
        case TOK_NOT:
            left = parse_unary(parser);
            break;
        default:
            error(parser, "Expected expression");
            return nullptr;
    }

    // Infix
    while (precedence <= get_precedence(parser->current.type)) {
        advance(parser);
        Token op_token = parser->previous;

        switch (op_token.type) {
            case TOK_PLUS:
            case TOK_MINUS:
            case TOK_STAR:
            case TOK_SLASH:
            case TOK_PERCENT:
            case TOK_STAR_STAR:
            case TOK_EQ:
            case TOK_NE:
            case TOK_LT:
            case TOK_GT:
            case TOK_LE:
            case TOK_GE:
            case TOK_AND:
            case TOK_OR:
            case TOK_AT:
                left = parse_binary(parser, left, get_precedence(op_token.type));
                break;
            case TOK_LPAREN:
                left = parse_call(parser, left);
                break;
            case TOK_LBRACKET:
                left = parse_index(parser, left);
                break;
            case TOK_DOT:
                left = parse_member(parser, left);
                break;
            default:
                return left;
        }
    }

    return left;
}

static ASTNode* parse_expression(Parser* parser) {
    return parse_precedence(parser, PREC_PIPE);
}

// Statements
static ASTNode* parse_var_decl(Parser* parser, bool is_state) {
    Token token = parser->previous;

    consume(parser, TOK_IDENTIFIER, "Expected variable name");
    char* name = copy_token_string(&parser->previous);
    char* type_name = nullptr;

    // Optional type annotation
    if (match(parser, TOK_COLON)) {
        consume(parser, TOK_IDENTIFIER, "Expected type name");
        type_name = copy_token_string(&parser->previous);
    }

    ASTNode* initializer = nullptr;
    if (match(parser, TOK_ASSIGN)) {
        initializer = parse_expression(parser);
    }

    ASTNode* node = ast_alloc(AST_VAR_DECL, token.line, token.column);
    node->as.var_decl.name = name;
    node->as.var_decl.type_name = type_name;
    node->as.var_decl.initializer = initializer;
    node->as.var_decl.is_state = is_state;
    return node;
}

static ASTNode* parse_fn_decl(Parser* parser) {
    Token token = parser->previous;

    consume(parser, TOK_IDENTIFIER, "Expected function name");
    char* name = copy_token_string(&parser->previous);

    ParamNode* params = parse_params(parser);

    char* return_type = nullptr;
    if (match(parser, TOK_ARROW)) {
        // Could be return type or single-expression body
        if (check(parser, TOK_IDENTIFIER)) {
            Token peek = parser->current;
            advance(parser);
            if (check(parser, TOK_LBRACE)) {
                // It was a return type
                return_type = copy_token_string(&peek);
            }
            else {
                // It's actually a single-expression body starting with identifier
                // Rewind - actually we can't easily rewind, so handle differently
                // Create identifier node
                ASTNode* body = ast_alloc(AST_IDENTIFIER, peek.line, peek.column);
                body->as.identifier.name = copy_token_string(&peek);

                // Continue parsing the expression
                while (get_precedence(parser->current.type) > PREC_NONE) {
                    advance(parser);
                    Token op_token = parser->previous;

                    switch (op_token.type) {
                        case TOK_PLUS:
                        case TOK_MINUS:
                        case TOK_STAR:
                        case TOK_SLASH:
                        case TOK_PERCENT:
                        case TOK_STAR_STAR:
                        case TOK_EQ:
                        case TOK_NE:
                        case TOK_LT:
                        case TOK_GT:
                        case TOK_LE:
                        case TOK_GE:
                        case TOK_AND:
                        case TOK_OR:
                        case TOK_AT:
                            body = parse_binary(parser, body, get_precedence(op_token.type));
                            break;
                        case TOK_LPAREN:
                            body = parse_call(parser, body);
                            break;
                        case TOK_LBRACKET:
                            body = parse_index(parser, body);
                            break;
                        case TOK_DOT:
                            body = parse_member(parser, body);
                            break;
                        default:
                            goto done;
                    }
                }
                done:

                ASTNode* node = ast_alloc(AST_FN_DECL, token.line, token.column);
                node->as.fn_decl.name = name;
                node->as.fn_decl.params = params;
                node->as.fn_decl.return_type = nullptr;
                node->as.fn_decl.body = body;
                node->as.fn_decl.is_single_expr = true;
                return node;
            }
        }
        else if (!check(parser, TOK_LBRACE)) {
            // Single-expression body
            ASTNode* body = parse_expression(parser);
            ASTNode* node = ast_alloc(AST_FN_DECL, token.line, token.column);
            node->as.fn_decl.name = name;
            node->as.fn_decl.params = params;
            node->as.fn_decl.return_type = nullptr;
            node->as.fn_decl.body = body;
            node->as.fn_decl.is_single_expr = true;
            return node;
        }
    }

    consume(parser, TOK_LBRACE, "Expected '{' before function body");
    ASTNode* body = parse_block(parser);

    ASTNode* node = ast_alloc(AST_FN_DECL, token.line, token.column);
    node->as.fn_decl.name = name;
    node->as.fn_decl.params = params;
    node->as.fn_decl.return_type = return_type;
    node->as.fn_decl.body = body;
    node->as.fn_decl.is_single_expr = false;
    return node;
}

static ASTNode* parse_return(Parser* parser) {
    Token token = parser->previous;
    ASTNode* value = nullptr;

    if (!check(parser, TOK_RBRACE) && !check(parser, TOK_EOF)) {
        value = parse_expression(parser);
    }

    ASTNode* node = ast_alloc(AST_RETURN, token.line, token.column);
    node->as.return_stmt.value = value;
    return node;
}

static ASTNode* parse_if(Parser* parser) {
    Token token = parser->previous;

    ASTNode* condition = parse_expression(parser);

    consume(parser, TOK_LBRACE, "Expected '{' after if condition");
    ASTNode* then_branch = parse_block(parser);

    ASTNode* else_branch = nullptr;
    if (match(parser, TOK_ELSE)) {
        if (match(parser, TOK_IF)) {
            else_branch = parse_if(parser);
        }
        else {
            consume(parser, TOK_LBRACE, "Expected '{' after else");
            else_branch = parse_block(parser);
        }
    }

    ASTNode* node = ast_alloc(AST_IF, token.line, token.column);
    node->as.if_stmt.condition = condition;
    node->as.if_stmt.then_branch = then_branch;
    node->as.if_stmt.else_branch = else_branch;
    return node;
}

static ASTNode* parse_while(Parser* parser) {
    Token token = parser->previous;

    ASTNode* condition = parse_expression(parser);

    consume(parser, TOK_LBRACE, "Expected '{' after while condition");
    ASTNode* body = parse_block(parser);

    ASTNode* node = ast_alloc(AST_WHILE, token.line, token.column);
    node->as.while_loop.condition = condition;
    node->as.while_loop.body = body;
    return node;
}

static ASTNode* parse_for(Parser* parser) {
    Token token = parser->previous;

    consume(parser, TOK_IDENTIFIER, "Expected variable name after 'for'");
    char* var_name = copy_token_string(&parser->previous);

    consume(parser, TOK_IN, "Expected 'in' after for variable");

    ASTNode* iterable = parse_expression(parser);

    consume(parser, TOK_LBRACE, "Expected '{' after for iterable");
    ASTNode* body = parse_block(parser);

    ASTNode* node = ast_alloc(AST_FOR, token.line, token.column);
    node->as.for_loop.var_name = var_name;
    node->as.for_loop.iterable = iterable;
    node->as.for_loop.body = body;
    return node;
}

static ASTNode* parse_loop(Parser* parser) {
    Token token = parser->previous;

    consume(parser, TOK_LBRACE, "Expected '{' after loop");
    ASTNode* body = parse_block(parser);

    ASTNode* node = ast_alloc(AST_LOOP, token.line, token.column);
    node->as.loop.body = body;
    return node;
}

static ASTNode* parse_match(Parser* parser) {
    Token token = parser->previous;

    ASTNode* value = parse_expression(parser);

    consume(parser, TOK_LBRACE, "Expected '{' after match value");

    MatchArm* arms = nullptr;

    while (!check(parser, TOK_RBRACE) && !check(parser, TOK_EOF)) {
        // Pattern
        ASTNode* pattern = nullptr;

        if (match(parser, TOK_IDENTIFIER)) {
            // Variable pattern or _ (wildcard)
            char* name = copy_token_string(&parser->previous);
            pattern = ast_alloc(AST_IDENTIFIER, parser->previous.line, parser->previous.column);
            pattern->as.identifier.name = name;
        }
        else {
            pattern = parse_expression(parser);
        }

        // Optional guard
        ASTNode* guard = nullptr;
        if (match(parser, TOK_IF)) {
            guard = parse_expression(parser);
        }

        consume(parser, TOK_ARROW, "Expected '->' after match pattern");

        // Body
        ASTNode* body = nullptr;
        if (check(parser, TOK_LBRACE)) {
            advance(parser);
            body = parse_block(parser);
        }
        else {
            body = parse_expression(parser);
        }

        arms = match_arm_append(arms, pattern, guard, body);

        // Optional comma/newline between arms
        match(parser, TOK_COMMA);
    }

    consume(parser, TOK_RBRACE, "Expected '}' after match arms");

    ASTNode* node = ast_alloc(AST_MATCH, token.line, token.column);
    node->as.match.value = value;
    node->as.match.arms = arms;
    return node;
}

static ASTNode* parse_block(Parser* parser) {
    Token token = parser->previous;
    ASTNode* node = ast_alloc(AST_BLOCK, token.line, token.column);
    node->as.block.statements = nullptr;

    while (!check(parser, TOK_RBRACE) && !check(parser, TOK_EOF)) {
        ASTNode* stmt = parse_statement(parser);
        if (stmt != nullptr) {
            node->as.block.statements = ast_list_append(node->as.block.statements, stmt);
        }
    }

    consume(parser, TOK_RBRACE, "Expected '}' after block");
    return node;
}

static ASTNode* parse_assignment_or_expr(Parser* parser) {
    ASTNode* expr = parse_expression(parser);

    if (match(parser, TOK_ASSIGN)) {
        ASTNode* value = parse_expression(parser);
        ASTNode* node = ast_alloc(AST_ASSIGN, expr->line, expr->column);
        node->as.assign.target = expr;
        node->as.assign.value = value;
        return node;
    }

    if (match(parser, TOK_PLUS_EQ) || match(parser, TOK_MINUS_EQ) ||
        match(parser, TOK_STAR_EQ) || match(parser, TOK_SLASH_EQ) ||
        match(parser, TOK_PERCENT_EQ)) {

        CompoundOp op;
        switch (parser->previous.type) {
            case TOK_PLUS_EQ:    op = COMP_ADD; break;
            case TOK_MINUS_EQ:   op = COMP_SUB; break;
            case TOK_STAR_EQ:    op = COMP_MUL; break;
            case TOK_SLASH_EQ:   op = COMP_DIV; break;
            case TOK_PERCENT_EQ: op = COMP_MOD; break;
            default:             op = COMP_ADD; break;
        }

        ASTNode* value = parse_expression(parser);
        ASTNode* node = ast_alloc(AST_COMPOUND_ASSIGN, expr->line, expr->column);
        node->as.compound_assign.op = op;
        node->as.compound_assign.target = expr;
        node->as.compound_assign.value = value;
        return node;
    }

    ASTNode* node = ast_alloc(AST_EXPR_STMT, expr->line, expr->column);
    node->as.expr_stmt.expr = expr;
    return node;
}

static ASTNode* parse_statement(Parser* parser) {
    if (match(parser, TOK_LET)) {
        return parse_var_decl(parser, false);
    }
    if (match(parser, TOK_STATE)) {
        return parse_var_decl(parser, true);
    }
    if (match(parser, TOK_FN)) {
        return parse_fn_decl(parser);
    }
    if (match(parser, TOK_RETURN)) {
        return parse_return(parser);
    }
    if (match(parser, TOK_IF)) {
        return parse_if(parser);
    }
    if (match(parser, TOK_WHILE)) {
        return parse_while(parser);
    }
    if (match(parser, TOK_FOR)) {
        return parse_for(parser);
    }
    if (match(parser, TOK_LOOP)) {
        return parse_loop(parser);
    }
    if (match(parser, TOK_MATCH)) {
        return parse_match(parser);
    }
    if (match(parser, TOK_BREAK)) {
        return ast_alloc(AST_BREAK, parser->previous.line, parser->previous.column);
    }
    if (match(parser, TOK_CONTINUE)) {
        return ast_alloc(AST_CONTINUE, parser->previous.line, parser->previous.column);
    }

    return parse_assignment_or_expr(parser);
}

ASTNode* parser_parse(Parser* parser) {
    ASTNode* program = ast_alloc(AST_PROGRAM, 1, 1);
    program->as.program.statements = nullptr;

    while (!check(parser, TOK_EOF)) {
        ASTNode* stmt = parse_statement(parser);
        if (stmt != nullptr) {
            program->as.program.statements = ast_list_append(program->as.program.statements, stmt);
        }

        if (parser->panic_mode) {
            // Synchronize on statement boundaries
            parser->panic_mode = false;
            while (!check(parser, TOK_EOF)) {
                if (parser->previous.type == TOK_SEMICOLON) break;
                if (check(parser, TOK_LET) || check(parser, TOK_FN) ||
                    check(parser, TOK_IF) || check(parser, TOK_WHILE) ||
                    check(parser, TOK_FOR) || check(parser, TOK_RETURN)) {
                    break;
                }
                advance(parser);
            }
        }
    }

    return program;
}

bool parser_had_error(Parser* parser) {
    return parser->had_error;
}

const char* parser_error_message(Parser* parser) {
    return parser->error_message;
}
