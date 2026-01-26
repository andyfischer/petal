/*
 *  parse_steps
 *
 *  Implements handler function for each step of parsing.
 */

#include "parser/parse_steps.h"
#include "program/program.h"
#include "globals/global_state.h"
#include "program/term.h"
#include "program/block.h"
#include "variant/variant.h"
#include "parser/parse_error.h"
#include "program/program_building.h"
#include "parser/parse_context.h"
#include "runtime/native_funcs.h"

// Parsing steps:
TermRef statement(ParseContext* context);
TermRef let_statement(ParseContext* context);
TermRef assignment_statement(ParseContext* context);
TermRef expr_statement(ParseContext* context);
bool lookahead_for_identifier_assignment(ParseContext* context);
TermRef expr(ParseContext* context);
TermRef infix_expr(ParseContext* context);
TermRef unary_expr(ParseContext* context);
TermRef atom_with_suffix(ParseContext* context);
TermRef atom(ParseContext* context);
TermRef function_call(ParseContext* context);
TermRef literal_int(ParseContext* context);
TermRef literal_float(ParseContext* context);
TermRef literal_hex(ParseContext* context);
TermRef literal_binary(ParseContext* context);
TermRef literal_string(ParseContext* context);
TermRef literal_bool(ParseContext* context);
TermRef literal_null(ParseContext* context);
TermRef literal_symbol(ParseContext* context);
TermRef function_definition(ParseContext* context);
std::vector<TermRef> function_definition_inputs(ParseContext* context);
TermRef struct_definition(ParseContext* context);
TermRef struct_field_statement(ParseContext* context);
void parse_struct_field_list(ParseContext* context);
TermRef if_statement(ParseContext* context);
TermRef for_statement(ParseContext* context);

// StepWrapper
//
// Helper class to track the current parser step for debugging.
struct StepWrapper {
    ParseContext* context;
    const char* step_name;

    StepWrapper(ParseContext* context, const char* step_name) {
        this->context = context;
        this->step_name = step_name;

        context->trace_start(step_name);
    }

    ~StepWrapper() {
        context->trace_end(step_name);
    }
};

// trace_step
//
// Macro to trace the current step function. Must be added as the first statement in every func.
#define trace_step() StepWrapper step_wrapper(context, __FUNCTION__);

void parse_statement_list(ParseContext* context) {
    trace_step();

    while (!context->it->finished(0)) {
        context->it->skip_whitespace();

        if (context->it->next_is(0, Token::RBrace)) {
            break;
        }

        TermRef result = statement(context);
        if (result.type == TermRefType::None) {
            break;
        }
    }
}

TermRef statement(ParseContext* context) {
    trace_step();

    context->it->skip_whitespace_and_newlines();

    TermRef result = TermRef::None();

    // Look for 'let' statements
    if (context->it->try_consume(Token::Let)) {
        result = let_statement(context);
    } 
    
    // Look for 'if' statements
    else if (context->it->try_consume(Token::If)) {
        result = if_statement(context);
    }
    
    // Look for 'for' statements
    else if (context->it->try_consume(Token::For)) {
        result = for_statement(context);
    }

    // Look for assignment statements (identifier followed by equals)
    else if (context->it->next_is(0, Token::Identifier)) {
        if (lookahead_for_identifier_assignment(context)) {
            result = assignment_statement(context);
        } else {
            result = expr_statement(context);
        }
    } 
    else {
        result = expr_statement(context);
    }

    context->it->try_consume(Token::Semicolon);
    context->it->skip_whitespace_and_newlines();

    return result;
}

TermRef let_statement(ParseContext* context) {
    trace_step();

    context->it->skip_whitespace();
    if (!context->it->next_is(0, Token::Identifier)) {
        return syntax_error(context, "Expected identifier after 'let'");
    }
    
    std::string ident = context->it->next_text(0);
    context->it->consume();

    context->it->skip_whitespace();
    
    if (!context->it->try_consume(Token::Equals)) {
        return syntax_error(context, "Expected '=' after identifier");
    }
    
    context->it->skip_whitespace();
    
    TermRef expression_term = expr_statement(context);
    
    if (expression_term.type == TermRefType::None) {
        return syntax_error(context, "Expected expression after '='");
    }

    if (expression_term.type != TermRefType::TermIdRef) {
        // bug - this will probably fail on `let a = b'
        ErrorContext error_context;
        error_context.while_parsing("let = statement");
        return syntax_error_unexpected_next_token(context, &error_context);
    }

    // Assign this name to the expression term
    u32 name_id = context->env->get_or_create_symbol(ident.c_str());
    Term* term = context->program->get_term(expression_term.term_id);
    term->set_name(name_id);

    return expression_term;
}

TermRef assignment_statement(ParseContext* context) {
    trace_step();

    // We should only be called when lookahead confirmed this is an assignment
    if (!context->it->next_is(0, Token::Identifier)) {
        return syntax_error(context, "Expected identifier in assignment");
    }
    
    std::string ident = context->it->next_text(0);
    context->it->consume();

    context->it->skip_whitespace();
    
    if (!context->it->try_consume(Token::Equals)) {
        return syntax_error(context, "Expected '=' in assignment");
    }
    
    context->it->skip_whitespace();
    
    TermRef expression_term = expr_statement(context);
    
    if (expression_term.type == TermRefType::None) {
        return syntax_error(context, "Expected expression after '='");
    }

    // Assign this name to the expression term
    u32 name_id = context->env->get_or_create_symbol(ident.c_str());
    Term* term = context->program->get_term(expression_term.term_id);
    term->set_name(name_id);
    
    return expression_term;
}

bool lookahead_for_identifier_assignment(ParseContext* context) {
    // This function checks if the current position has the pattern: identifier = ...
    // It assumes position 0 is already confirmed to be an identifier
    
    size_t lookahead = 1;
    
    // Skip any whitespace tokens
    while (!context->it->finished(lookahead) && context->it->next_is(lookahead, Token::Whitespace)) {
        lookahead++;
    }
    
    // Check if we found an equals sign
    return !context->it->finished(lookahead) && context->it->next_is(lookahead, Token::Equals);
}

TermRef expr_statement(ParseContext* context) {
    trace_step();

    auto result = expr(context);
    context->it->skip_whitespace();
    return result;
}

TermRef expr(ParseContext* context) {
    trace_step();

    // TODO: Look for other types of expressions
    return infix_expr(context);
}

TermRef infix_expr(ParseContext* context) {
    trace_step();

    TermRef lhs = unary_expr(context);
    
    // Check for infix operators
    context->it->skip_whitespace();
    
    if (context->it->finished(0)) {
        return lhs;
    }
    
    Token op_token = context->it->next(0)->tok_match;
    NativeFunctionId native_func = NativeFunctionId::None;
    
    // Map tokens to native functions
    switch (op_token) {
        case Token::DoubleEquals:
            native_func = NativeFunctionId::Eq;
            break;
        case Token::NotEquals:
            native_func = NativeFunctionId::Ne;
            break;
        case Token::LThan:
            native_func = NativeFunctionId::Lt;
            break;
        case Token::GThan:
            native_func = NativeFunctionId::Gt;
            break;
        case Token::LThanEq:
            native_func = NativeFunctionId::Le;
            break;
        case Token::GThanEq:
            native_func = NativeFunctionId::Ge;
            break;
        case Token::Plus:
            native_func = NativeFunctionId::Add;
            break;
        case Token::Minus:
            native_func = NativeFunctionId::Sub;
            break;
        case Token::Star:
            native_func = NativeFunctionId::Mult;
            break;
        case Token::Slash:
            native_func = NativeFunctionId::Div;
            break;
        default:
            // No infix operator found
            return lhs;
    }
    
    // Consume the operator token
    context->it->consume();
    context->it->skip_whitespace();
    
    // Parse the right-hand side
    TermRef rhs = unary_expr(context);
    if (rhs.type == TermRefType::None) {
        return syntax_error(context, "Expected expression after infix operator");
    }
    
    // Create the infix operation term
    TermRef func_ref = TermRef::from_native_function_id(native_func);
    std::vector<TermRef> inputs = {lhs, rhs};
    Term* infix_term = create_term(context->program, context->block, func_ref, inputs);
    
    return infix_term->as_ref();
}

TermRef unary_expr(ParseContext* context) {
    trace_step();

    // Check for unary minus operator
    if (context->it->next_is(0, Token::Minus)) {
        context->it->consume(); // consume the minus sign
        
        TermRef operand = atom_with_suffix(context);
        if (operand.type == TermRefType::None) {
            return TermRef::None();
        }
        
        // TODO: Create unary minus operation term
        // For now, if the operand is a numeric literal, negate it directly
        Term* operand_term = context->program->get_term(operand.term_id);
        if (operand_term && operand_term->has_fixed_value()) {
            Variant32* value_ptr = operand_term->get_fixed_value();
            if (value_ptr && value_ptr->type == VariantType::I32) {
                int value = value_ptr->get_int32();
                Variant32 negated_variant = Variant32::from_int(-value);
                Term* negated_term = create_value_term(context->program, context->block, negated_variant);
                return negated_term->as_ref();
            } else if (value_ptr && value_ptr->type == VariantType::Float32) {
                float value = value_ptr->get_float32();
                Variant32 negated_variant = Variant32::from_float(-value);
                Term* negated_term = create_value_term(context->program, context->block, negated_variant);
                return negated_term->as_ref();
            }
        }
        
        return operand; // Fallback - return original operand
    }
    
    return atom_with_suffix(context);
}

TermRef atom_with_suffix(ParseContext* context) {
    trace_step();

    TermRef result = atom(context);
    
    // TODO: Suffixes like []
    
    return result;
}

TermRef atom(ParseContext* context) {
    trace_step();

    if (context->it->finished(0)) {
        return TermRef::None();
    }

    // Check for function call (identifier followed by left parenthesis)
    if (context->it->next_is(0, Token::Identifier) && context->it->next_is(1, Token::LParen)) {
        return function_call(context);
    }
    
    const FoundToken* token = context->it->next(0);
    if (!token) {
        return TermRef::None();
    }
    
    switch (token->tok_match) {
        case Token::Integer:
            return literal_int(context);
            
        case Token::Float:
            return literal_float(context);
            
        case Token::HexInteger:
            return literal_hex(context);
            
        case Token::BinaryInteger:
            return literal_binary(context);
            
        case Token::StringLiteral:
            return literal_string(context);
            
        case Token::True:
        case Token::False:
            return literal_bool(context);
            
        case Token::Null:
            return literal_null(context);
            
        case Token::Symbol:
            return literal_symbol(context);
            
        case Token::Fn:
            return function_definition(context);
            
        case Token::Struct:
            return struct_definition(context);
            
        case Token::Identifier: {
            auto name_id = context->it->next_ident_to_name_id(&context->env->symbols);
            context->it->consume();
            
            return TermRef::from_name_id(name_id);
        }
        default:
            return TermRef::None();
    }
}

TermRef function_call(ParseContext* context) {
    trace_step();

    // Get function name
    auto name_id = context->it->next_ident_to_name_id(&context->env->symbols);
    context->it->consume();
    auto func_ref = TermRef::from_name_id(name_id);
    
    // Left paren
    context->it->consume(); // consume the left parenthesis

    std::vector<TermRef> inputs;

    // Parse inputs
    while (true) {
        context->it->skip_whitespace();

        if (context->it->next_is(0, Token::RParen) || context->it->finished(0)) {
            break;
        }

        // Parse input
        TermRef input = expr(context);
        if (input.type == TermRefType::None) {
            break;
        }

        inputs.push_back(input);

        context->it->skip_whitespace();
        context->it->try_consume(Token::Comma);
    }

    context->it->try_consume(Token::RParen);

    // Create the function call term
    Term* function_call = create_term(context->program, context->block, func_ref, {});
    function_call->set_inputs(inputs);
    
    return function_call->as_ref();
}

TermRef literal_int(ParseContext* context) {
    trace_step();

    std::string text = context->it->next_text(0);
    
    // Parse the integer value from the text
    int value = std::stoi(text);
    
    context->it->consume();
    
    // Create a term with the integer value
    Variant32 variant = Variant32::from_int(value);
    
    Term* term = create_value_term(context->program, context->block, variant);
    return term->as_ref();
}

TermRef literal_symbol(ParseContext* context) {
    trace_step();

    std::string text = context->it->next_text(0);
    
    // Remove the leading colon
    size_t colon_pos = text.find(':');
    if (colon_pos != std::string::npos) {
        text = text.substr(colon_pos + 1);
    }
    
    // Create a symbol from the text
    SymbolId symbol_id = context->env->get_or_create_symbol(text.c_str());
    
    context->it->consume();
    
    // Create a term with the symbol value
    Variant32 variant = Variant32::from_symbol(symbol_id);
    
    Term* term = create_value_term(context->program, context->block, variant);
    
    // Return a reference to the created term
    return term->as_ref();
}

TermRef literal_float(ParseContext* context) {
    trace_step();

    std::string text = context->it->next_text(0);
    
    // Parse the float value from the text
    float value = std::stof(text);
    
    context->it->consume();
    
    // Create a term with the float value
    Variant32 variant = Variant32::from_float(value);
    
    Term* term = create_value_term(context->program, context->block, variant);
    return term->as_ref();
}

TermRef literal_hex(ParseContext* context) {
    trace_step();

    std::string text = context->it->next_text(0);
    
    // Parse the hex value from the text (remove 0x prefix)
    int value = std::stoi(text, nullptr, 16);
    
    context->it->consume();
    
    // Create a term with the integer value
    Variant32 variant = Variant32::from_int(value);
    
    Term* term = create_value_term(context->program, context->block, variant);
    return term->as_ref();
}

TermRef literal_binary(ParseContext* context) {
    trace_step();

    std::string text = context->it->next_text(0);
    
    // Parse the binary value from the text (remove 0b prefix)
    int value = std::stoi(text.substr(2), nullptr, 2);
    
    context->it->consume();
    
    // Create a term with the integer value
    Variant32 variant = Variant32::from_int(value);
    
    Term* term = create_value_term(context->program, context->block, variant);
    return term->as_ref();
}

TermRef literal_string(ParseContext* context) {
    trace_step();

    std::string text = context->it->next_text(0);
    
    // Remove the quotes from the string literal
    if (text.length() >= 2 && (text[0] == '"' || text[0] == '\'')) {
        text = text.substr(1, text.length() - 2);
    }
    
    // For now, store strings as symbols (temporary solution)
    // TODO: Implement proper string table management
    SymbolId string_symbol_id = context->env->get_or_create_symbol(text.c_str());
    
    context->it->consume();
    
    // Create a term with the symbol value (representing the string)
    Variant32 variant = Variant32::from_symbol(string_symbol_id);
    
    Term* term = create_value_term(context->program, context->block, variant);
    return term->as_ref();
}

TermRef literal_bool(ParseContext* context) {
    trace_step();

    std::string text = context->it->next_text(0);
    
    // Parse the boolean value
    bool value = (text == "true");
    
    context->it->consume();
    
    // Create a term with the boolean value (as integer)
    Variant32 variant = Variant32::from_int(value ? 1 : 0);
    
    Term* term = create_value_term(context->program, context->block, variant);
    return term->as_ref();
}

TermRef literal_null(ParseContext* context) {
    trace_step();

    context->it->consume();
    
    // Create a term with null value
    Variant32 variant = Variant32::None();
    
    Term* term = create_value_term(context->program, context->block, variant);
    return term->as_ref();
}

TermRef function_definition(ParseContext* context) {
    trace_step();

    context->it->consume(); // consume the 'fn' keyword
    context->it->skip_whitespace();
    
    if (!context->it->next_is(0, Token::Identifier)) {
        return syntax_error(context, "Expected function name after 'fn'");
    }
    
    auto name_id = context->it->next_ident_to_name_id(&context->env->symbols);
    context->it->consume();
    
    if (!context->it->try_consume(Token::LParen)) {
        return syntax_error(context, "Expected '(' after function name");
    }
    
    std::vector<TermRef> inputs = function_definition_inputs(context);
    
    // Create the function definition term first
    Term* function_def_term = context->block->add_term();
    function_def_term->set_name(name_id);
    
    // Create a nested block for the function body using the new method
    Block* function_block = function_def_term->add_nested_block();
    
    // Set the function definition term's value to reference the function block
    Variant32 variant = Variant32::function_def(function_block->block_id);
    function_def_term->const_value_pos = function_def_term->parent_block->const_data.alloc_variant_32(variant);
    function_def_term->func = TermRef::from_native_function_id(NativeFunctionId::Value);
    
    // Create input terms for each parameter
    for (const auto& input : inputs) {
        Term* input_term = create_input_term(context->program, function_block);
        input_term->set_name(input.name_id);
    }
    
    context->it->skip_whitespace();
    
    // Check for optional return type annotation (-> Type)
    if (context->it->try_consume(Token::RightArrow)) {
        context->it->skip_whitespace();
        
        // TODO: Parse return type - for now just skip the identifier
        if (context->it->next_is(0, Token::Identifier)) {
            context->it->consume();
        }
        
        context->it->skip_whitespace();
    }
    
    // Check if this is a function declaration (ends with semicolon) or definition (has body)
    if (context->it->try_consume(Token::Semicolon)) {
        // Function declaration - no body, just return the function term
        return function_def_term->as_ref();
    } else if (context->it->try_consume(Token::LBrace)) {
        // Function definition with body
        context->it->skip_whitespace();
        
        // Parse the function body
        Block* preserve_block = context->block;
        context->block = function_block;
        parse_statement_list(context);
        context->block = preserve_block;
        
        if (!context->it->try_consume(Token::RBrace)) {
            return syntax_error(context, "Expected '}'");
        }
    } else {
        return syntax_error(context, "Expected ';' for function declaration or '{' for function definition");
    }
    
    // Return a reference to the function definition term
    return function_def_term->as_ref();
}

std::vector<TermRef> function_definition_inputs(ParseContext* context) {
    trace_step();

    std::vector<TermRef> result_inputs;
    
    while (true) {
        context->it->skip_whitespace();
        
        // Finish consuming inputs if the next token is ')' or we've reached the end
        if (context->it->try_consume(Token::RParen) || context->it->finished(0)) {
            break;
        }
        
        if (!context->it->next_is(0, Token::Identifier)) {
            std::string next_text = context->it->next_text(0);
            syntax_error(context, ("Expected identifier for function parameter, got: " + next_text).c_str());
            return result_inputs;
        }
        
        context->it->skip_whitespace();

        auto name_id = context->it->next_ident_to_name_id(&context->env->symbols);
        context->it->consume();
        
        // future: type annotations

        result_inputs.push_back(TermRef::from_name_id(name_id));
        
        context->it->skip_whitespace();
        context->it->try_consume(Token::Comma);
        context->it->skip_whitespace();
    }
    
    return result_inputs;
}

TermRef struct_definition(ParseContext* context) {
    trace_step();

    context->it->consume(); // consume the 'struct' keyword
    context->it->skip_whitespace();
    
    if (!context->it->next_is(0, Token::Identifier)) {
        return syntax_error(context, "Expected struct name after 'struct'");
    }
    
    auto name_id = context->it->next_ident_to_name_id(&context->env->symbols);
    context->it->consume();
    
    context->it->skip_whitespace();
    
    if (!context->it->try_consume(Token::LBrace)) {
        return syntax_error(context, "Expected '{' after struct name");
    }
    
    // Create the struct definition term first
    Term* struct_def_term = context->block->add_term();
    struct_def_term->set_name(name_id);
    
    // Create a nested block for the struct fields using the new method
    Block* struct_block = struct_def_term->add_nested_block();
    
    // Set the struct definition term's value to reference the struct block
    Variant32 variant = Variant32::function_def(struct_block->block_id);
    struct_def_term->const_value_pos = struct_def_term->parent_block->const_data.alloc_variant_32(variant);
    struct_def_term->func = TermRef::from_native_function_id(NativeFunctionId::Value);
    
    context->it->skip_whitespace();
    
    // Parse the struct fields
    Block* preserve_block = context->block;
    context->block = struct_block;
    parse_struct_field_list(context);
    context->block = preserve_block;
    
    if (!context->it->try_consume(Token::RBrace)) {
        return syntax_error(context, "Expected '}'");
    }
    
    // Return a reference to the struct definition term
    return struct_def_term->as_ref();
}

void parse_struct_field_list(ParseContext* context) {
    trace_step();

    while (!context->it->finished(0)) {
        context->it->skip_whitespace_and_newlines();

        if (context->it->next_is(0, Token::RBrace)) {
            break;
        }

        TermRef result = struct_field_statement(context);
        if (result.type == TermRefType::None) {
            break;
        }
    }
}

TermRef struct_field_statement(ParseContext* context) {
    trace_step();

    context->it->skip_whitespace_and_newlines();

    if (!context->it->next_is(0, Token::Identifier)) {
        return TermRef::None();
    }
    
    std::string field_name = context->it->next_text(0);
    context->it->consume();

    context->it->skip_whitespace();
    
    // Optional type annotation with colon
    if (context->it->try_consume(Token::Colon)) {
        context->it->skip_whitespace();
        
        // TODO: Parse field type - for now just skip the identifier
        if (context->it->next_is(0, Token::Identifier)) {
            context->it->consume();
        }
        
        context->it->skip_whitespace();
    }

    // Create a field term 
    u32 field_name_id = context->env->get_or_create_symbol(field_name.c_str());
    Term* field_term = context->block->add_term();
    field_term->set_name(field_name_id);
    field_term->func = TermRef::from_native_function_id(NativeFunctionId::Value);
    
    // Set a placeholder value for the field
    Variant32 variant = Variant32::None();
    field_term->const_value_pos = field_term->parent_block->const_data.alloc_variant_32(variant);

    context->it->try_consume(Token::Semicolon);
    context->it->skip_whitespace_and_newlines();

    return field_term->as_ref();
}

TermRef if_statement(ParseContext* context) {
    trace_step();
    
    // 'if' keyword was already consumed by the caller
    context->it->skip_whitespace();
    
    // Parse condition expression
    TermRef condition = expr(context);
    if (condition.type == TermRefType::None) {
        return syntax_error(context, "Expected condition expression after 'if'");
    }
    
    context->it->skip_whitespace();
    
    if (!context->it->try_consume(Token::LBrace)) {
        return syntax_error(context, "Expected '{' after if condition");
    }
    
    // Create the if statement term
    Term* if_term = context->block->add_term();
    if_term->func = TermRef::from_native_function_id(NativeFunctionId::Value); // TODO: Use proper if function
    
    // Create a nested block for the if body
    Block* if_block = if_term->add_nested_block();
    
    // Set the if term's value to reference the if block
    Variant32 variant = Variant32::function_def(if_block->block_id);
    if_term->const_value_pos = if_term->parent_block->const_data.alloc_variant_32(variant);
    
    // Set the condition as input
    std::vector<TermRef> inputs = {condition};
    if_term->set_inputs(inputs);
    
    context->it->skip_whitespace();
    
    // Parse the if body
    Block* preserve_block = context->block;
    context->block = if_block;
    parse_statement_list(context);
    context->block = preserve_block;
    
    if (!context->it->try_consume(Token::RBrace)) {
        return syntax_error(context, "Expected '}' after if body");
    }
    
    context->it->skip_whitespace();
    
    // Check for optional 'else' clause
    if (context->it->try_consume(Token::Else)) {
        context->it->skip_whitespace();
        
        if (!context->it->try_consume(Token::LBrace)) {
            return syntax_error(context, "Expected '{' after 'else'");
        }
        
        // Create a nested block for the else body
        Block* else_block = if_term->add_nested_block();
        
        context->it->skip_whitespace();
        
        // Parse the else body
        context->block = else_block;
        parse_statement_list(context);
        context->block = preserve_block;
        
        if (!context->it->try_consume(Token::RBrace)) {
            return syntax_error(context, "Expected '}' after else body");
        }
    }
    
    return if_term->as_ref();
}

TermRef for_statement(ParseContext* context) {
    trace_step();
    
    // 'for' keyword was already consumed by the caller
    context->it->skip_whitespace();
    
    // Parse loop variable
    if (!context->it->next_is(0, Token::Identifier)) {
        return syntax_error(context, "Expected identifier for loop variable");
    }
    
    std::string loop_var_name = context->it->next_text(0);
    context->it->consume();
    
    context->it->skip_whitespace();
    
    // Expect 'in' keyword (for now, treat as identifier)
    if (!context->it->next_is(0, Token::Identifier) || context->it->next_text(0) != "in") {
        return syntax_error(context, "Expected 'in' after loop variable");
    }
    context->it->consume();
    
    context->it->skip_whitespace();
    
    // Parse iterable expression
    TermRef iterable = expr(context);
    if (iterable.type == TermRefType::None) {
        return syntax_error(context, "Expected iterable expression after 'in'");
    }
    
    context->it->skip_whitespace();
    
    if (!context->it->try_consume(Token::LBrace)) {
        return syntax_error(context, "Expected '{' after for expression");
    }
    
    // Create the for statement term
    Term* for_term = context->block->add_term();
    for_term->func = TermRef::from_native_function_id(NativeFunctionId::Value); // TODO: Use proper for function
    
    // Create a nested block for the for body
    Block* for_block = for_term->add_nested_block();
    
    // Set the for term's value to reference the for block
    Variant32 variant = Variant32::function_def(for_block->block_id);
    for_term->const_value_pos = for_term->parent_block->const_data.alloc_variant_32(variant);
    
    // Set the iterable as input
    std::vector<TermRef> inputs = {iterable};
    for_term->set_inputs(inputs);
    
    // Create loop variable in the for block
    Block* preserve_block = context->block;
    context->block = for_block;
    
    u32 loop_var_name_id = context->env->get_or_create_symbol(loop_var_name.c_str());
    Term* loop_var_term = context->block->add_term();
    loop_var_term->set_name(loop_var_name_id);
    loop_var_term->func = TermRef::from_native_function_id(NativeFunctionId::Value);
    
    // Set a placeholder value for the loop variable
    Variant32 loop_var_variant = Variant32::None();
    loop_var_term->const_value_pos = loop_var_term->parent_block->const_data.alloc_variant_32(loop_var_variant);
    
    context->it->skip_whitespace();
    
    // Parse the for body
    parse_statement_list(context);
    context->block = preserve_block;
    
    if (!context->it->try_consume(Token::RBrace)) {
        return syntax_error(context, "Expected '}' after for body");
    }
    
    return for_term->as_ref();
}
