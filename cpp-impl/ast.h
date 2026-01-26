#pragma once

#include "lexer.h"
#include <cstdint>

enum ASTNodeType {
    // Literals
    AST_INT_LITERAL,
    AST_FLOAT_LITERAL,
    AST_STRING_LITERAL,
    AST_BOOL_LITERAL,
    AST_NULL_LITERAL,
    AST_SYMBOL_LITERAL,
    AST_ARRAY_LITERAL,
    AST_OBJECT_LITERAL,

    // Expressions
    AST_IDENTIFIER,
    AST_BINARY_OP,
    AST_UNARY_OP,
    AST_CALL,
    AST_INDEX,
    AST_MEMBER,
    AST_LAMBDA,

    // Statements
    AST_PROGRAM,
    AST_BLOCK,
    AST_VAR_DECL,
    AST_ASSIGN,
    AST_COMPOUND_ASSIGN,
    AST_FN_DECL,
    AST_RETURN,
    AST_IF,
    AST_WHILE,
    AST_FOR,
    AST_LOOP,
    AST_BREAK,
    AST_CONTINUE,
    AST_MATCH,
    AST_MATCH_ARM,
    AST_EXPR_STMT
};

enum BinaryOp {
    OP_ADD,
    OP_SUB,
    OP_MUL,
    OP_DIV,
    OP_MOD,
    OP_POW,
    OP_EQ,
    OP_NE,
    OP_LT,
    OP_GT,
    OP_LE,
    OP_GE,
    OP_AND,
    OP_OR,
    OP_PIPE  // @
};

enum UnaryOp {
    OP_NEG,
    OP_NOT
};

enum CompoundOp {
    COMP_ADD,
    COMP_SUB,
    COMP_MUL,
    COMP_DIV,
    COMP_MOD
};

struct ASTNode;

struct ASTNodeList {
    ASTNode* node;
    ASTNodeList* next;
};

struct ObjectFieldNode {
    char* key;
    ASTNode* value;
    ObjectFieldNode* next;
};

struct ParamNode {
    char* name;
    char* type_name;  // Optional type annotation
    ParamNode* next;
};

struct MatchArm {
    ASTNode* pattern;
    ASTNode* guard;      // Optional guard expression (if condition)
    ASTNode* body;
    MatchArm* next;
};

struct ASTNode {
    ASTNodeType type;
    int line;
    int column;

    union {
        // Literals
        int64_t int_value;
        double float_value;
        char* string_value;
        bool bool_value;
        char* symbol_value;

        // Array literal
        struct {
            ASTNodeList* elements;
        } array;

        // Object literal
        struct {
            ObjectFieldNode* fields;
        } object;

        // Identifier
        struct {
            char* name;
        } identifier;

        // Binary operation
        struct {
            BinaryOp op;
            ASTNode* left;
            ASTNode* right;
        } binary;

        // Unary operation
        struct {
            UnaryOp op;
            ASTNode* operand;
        } unary;

        // Function call
        struct {
            ASTNode* callee;
            ASTNodeList* args;
        } call;

        // Array/object index
        struct {
            ASTNode* object;
            ASTNode* index;
        } index;

        // Member access (dot notation)
        struct {
            ASTNode* object;
            char* member;
        } member;

        // Lambda function
        struct {
            ParamNode* params;
            ASTNode* body;
        } lambda;

        // Program
        struct {
            ASTNodeList* statements;
        } program;

        // Block
        struct {
            ASTNodeList* statements;
        } block;

        // Variable declaration
        struct {
            char* name;
            char* type_name;  // Optional type annotation
            ASTNode* initializer;
            bool is_state;
        } var_decl;

        // Assignment
        struct {
            ASTNode* target;
            ASTNode* value;
        } assign;

        // Compound assignment (+=, -=, etc.)
        struct {
            CompoundOp op;
            ASTNode* target;
            ASTNode* value;
        } compound_assign;

        // Function declaration
        struct {
            char* name;
            ParamNode* params;
            char* return_type;  // Optional
            ASTNode* body;
            bool is_single_expr;
        } fn_decl;

        // Return statement
        struct {
            ASTNode* value;
        } return_stmt;

        // If statement
        struct {
            ASTNode* condition;
            ASTNode* then_branch;
            ASTNode* else_branch;
        } if_stmt;

        // While loop
        struct {
            ASTNode* condition;
            ASTNode* body;
        } while_loop;

        // For loop
        struct {
            char* var_name;
            ASTNode* iterable;
            ASTNode* body;
        } for_loop;

        // Loop (infinite)
        struct {
            ASTNode* body;
        } loop;

        // Match expression
        struct {
            ASTNode* value;
            MatchArm* arms;
        } match;

        // Expression statement
        struct {
            ASTNode* expr;
        } expr_stmt;
    } as;
};

// AST construction helpers
ASTNode* ast_alloc(ASTNodeType type, int line, int column);
ASTNodeList* ast_list_append(ASTNodeList* list, ASTNode* node);
ParamNode* param_list_append(ParamNode* list, char* name, char* type_name);
ObjectFieldNode* object_field_append(ObjectFieldNode* list, char* key, ASTNode* value);
MatchArm* match_arm_append(MatchArm* list, ASTNode* pattern, ASTNode* guard, ASTNode* body);

// AST printing (for debugging)
void ast_print(ASTNode* node, int indent);
