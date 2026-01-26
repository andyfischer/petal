#include "ast.h"
#include <cstdlib>
#include <cstdio>
#include <cstring>

ASTNode* ast_alloc(ASTNodeType type, int line, int column) {
    ASTNode* node = (ASTNode*)calloc(1, sizeof(ASTNode));
    node->type = type;
    node->line = line;
    node->column = column;
    return node;
}

ASTNodeList* ast_list_append(ASTNodeList* list, ASTNode* node) {
    ASTNodeList* item = (ASTNodeList*)malloc(sizeof(ASTNodeList));
    item->node = node;
    item->next = nullptr;

    if (list == nullptr) {
        return item;
    }

    ASTNodeList* current = list;
    while (current->next != nullptr) {
        current = current->next;
    }
    current->next = item;
    return list;
}

ParamNode* param_list_append(ParamNode* list, char* name, char* type_name) {
    ParamNode* item = (ParamNode*)malloc(sizeof(ParamNode));
    item->name = name;
    item->type_name = type_name;
    item->next = nullptr;

    if (list == nullptr) {
        return item;
    }

    ParamNode* current = list;
    while (current->next != nullptr) {
        current = current->next;
    }
    current->next = item;
    return list;
}

ObjectFieldNode* object_field_append(ObjectFieldNode* list, char* key, ASTNode* value) {
    ObjectFieldNode* item = (ObjectFieldNode*)malloc(sizeof(ObjectFieldNode));
    item->key = key;
    item->value = value;
    item->next = nullptr;

    if (list == nullptr) {
        return item;
    }

    ObjectFieldNode* current = list;
    while (current->next != nullptr) {
        current = current->next;
    }
    current->next = item;
    return list;
}

MatchArm* match_arm_append(MatchArm* list, ASTNode* pattern, ASTNode* guard, ASTNode* body) {
    MatchArm* item = (MatchArm*)malloc(sizeof(MatchArm));
    item->pattern = pattern;
    item->guard = guard;
    item->body = body;
    item->next = nullptr;

    if (list == nullptr) {
        return item;
    }

    MatchArm* current = list;
    while (current->next != nullptr) {
        current = current->next;
    }
    current->next = item;
    return list;
}

static void print_indent(int indent) {
    for (int i = 0; i < indent; i++) {
        printf("  ");
    }
}

static const char* binary_op_name(BinaryOp op) {
    switch (op) {
        case OP_ADD: return "+";
        case OP_SUB: return "-";
        case OP_MUL: return "*";
        case OP_DIV: return "/";
        case OP_MOD: return "%";
        case OP_POW: return "**";
        case OP_EQ: return "==";
        case OP_NE: return "!=";
        case OP_LT: return "<";
        case OP_GT: return ">";
        case OP_LE: return "<=";
        case OP_GE: return ">=";
        case OP_AND: return "&&";
        case OP_OR: return "||";
        case OP_PIPE: return "@";
    }
    return "?";
}

static const char* unary_op_name(UnaryOp op) {
    switch (op) {
        case OP_NEG: return "-";
        case OP_NOT: return "!";
    }
    return "?";
}

void ast_print(ASTNode* node, int indent) {
    if (node == nullptr) {
        print_indent(indent);
        printf("(null)\n");
        return;
    }

    print_indent(indent);

    switch (node->type) {
        case AST_INT_LITERAL:
            printf("INT: %lld\n", (long long)node->as.int_value);
            break;

        case AST_FLOAT_LITERAL:
            printf("FLOAT: %g\n", node->as.float_value);
            break;

        case AST_STRING_LITERAL:
            printf("STRING: \"%s\"\n", node->as.string_value);
            break;

        case AST_BOOL_LITERAL:
            printf("BOOL: %s\n", node->as.bool_value ? "true" : "false");
            break;

        case AST_NULL_LITERAL:
            printf("NULL\n");
            break;

        case AST_SYMBOL_LITERAL:
            printf("SYMBOL: :%s\n", node->as.symbol_value);
            break;

        case AST_ARRAY_LITERAL:
            printf("ARRAY:\n");
            for (ASTNodeList* item = node->as.array.elements; item; item = item->next) {
                ast_print(item->node, indent + 1);
            }
            break;

        case AST_OBJECT_LITERAL:
            printf("OBJECT:\n");
            for (ObjectFieldNode* field = node->as.object.fields; field; field = field->next) {
                print_indent(indent + 1);
                printf("%s:\n", field->key);
                ast_print(field->value, indent + 2);
            }
            break;

        case AST_IDENTIFIER:
            printf("IDENT: %s\n", node->as.identifier.name);
            break;

        case AST_BINARY_OP:
            printf("BINARY %s:\n", binary_op_name(node->as.binary.op));
            ast_print(node->as.binary.left, indent + 1);
            ast_print(node->as.binary.right, indent + 1);
            break;

        case AST_UNARY_OP:
            printf("UNARY %s:\n", unary_op_name(node->as.unary.op));
            ast_print(node->as.unary.operand, indent + 1);
            break;

        case AST_CALL:
            printf("CALL:\n");
            print_indent(indent + 1);
            printf("callee:\n");
            ast_print(node->as.call.callee, indent + 2);
            print_indent(indent + 1);
            printf("args:\n");
            for (ASTNodeList* arg = node->as.call.args; arg; arg = arg->next) {
                ast_print(arg->node, indent + 2);
            }
            break;

        case AST_INDEX:
            printf("INDEX:\n");
            ast_print(node->as.index.object, indent + 1);
            ast_print(node->as.index.index, indent + 1);
            break;

        case AST_MEMBER:
            printf("MEMBER .%s:\n", node->as.member.member);
            ast_print(node->as.member.object, indent + 1);
            break;

        case AST_LAMBDA:
            printf("LAMBDA:\n");
            print_indent(indent + 1);
            printf("params: ");
            for (ParamNode* p = node->as.lambda.params; p; p = p->next) {
                printf("%s ", p->name);
            }
            printf("\n");
            print_indent(indent + 1);
            printf("body:\n");
            ast_print(node->as.lambda.body, indent + 2);
            break;

        case AST_PROGRAM:
            printf("PROGRAM:\n");
            for (ASTNodeList* stmt = node->as.program.statements; stmt; stmt = stmt->next) {
                ast_print(stmt->node, indent + 1);
            }
            break;

        case AST_BLOCK:
            printf("BLOCK:\n");
            for (ASTNodeList* stmt = node->as.block.statements; stmt; stmt = stmt->next) {
                ast_print(stmt->node, indent + 1);
            }
            break;

        case AST_VAR_DECL:
            printf("VAR_DECL: %s%s\n", node->as.var_decl.is_state ? "state " : "", node->as.var_decl.name);
            if (node->as.var_decl.initializer) {
                ast_print(node->as.var_decl.initializer, indent + 1);
            }
            break;

        case AST_ASSIGN:
            printf("ASSIGN:\n");
            ast_print(node->as.assign.target, indent + 1);
            ast_print(node->as.assign.value, indent + 1);
            break;

        case AST_COMPOUND_ASSIGN:
            printf("COMPOUND_ASSIGN:\n");
            ast_print(node->as.compound_assign.target, indent + 1);
            ast_print(node->as.compound_assign.value, indent + 1);
            break;

        case AST_FN_DECL:
            printf("FN_DECL: %s\n", node->as.fn_decl.name);
            print_indent(indent + 1);
            printf("params: ");
            for (ParamNode* p = node->as.fn_decl.params; p; p = p->next) {
                printf("%s ", p->name);
            }
            printf("\n");
            print_indent(indent + 1);
            printf("body:\n");
            ast_print(node->as.fn_decl.body, indent + 2);
            break;

        case AST_RETURN:
            printf("RETURN:\n");
            if (node->as.return_stmt.value) {
                ast_print(node->as.return_stmt.value, indent + 1);
            }
            break;

        case AST_IF:
            printf("IF:\n");
            print_indent(indent + 1);
            printf("condition:\n");
            ast_print(node->as.if_stmt.condition, indent + 2);
            print_indent(indent + 1);
            printf("then:\n");
            ast_print(node->as.if_stmt.then_branch, indent + 2);
            if (node->as.if_stmt.else_branch) {
                print_indent(indent + 1);
                printf("else:\n");
                ast_print(node->as.if_stmt.else_branch, indent + 2);
            }
            break;

        case AST_WHILE:
            printf("WHILE:\n");
            print_indent(indent + 1);
            printf("condition:\n");
            ast_print(node->as.while_loop.condition, indent + 2);
            print_indent(indent + 1);
            printf("body:\n");
            ast_print(node->as.while_loop.body, indent + 2);
            break;

        case AST_FOR:
            printf("FOR: %s in\n", node->as.for_loop.var_name);
            print_indent(indent + 1);
            printf("iterable:\n");
            ast_print(node->as.for_loop.iterable, indent + 2);
            print_indent(indent + 1);
            printf("body:\n");
            ast_print(node->as.for_loop.body, indent + 2);
            break;

        case AST_LOOP:
            printf("LOOP:\n");
            ast_print(node->as.loop.body, indent + 1);
            break;

        case AST_BREAK:
            printf("BREAK\n");
            break;

        case AST_CONTINUE:
            printf("CONTINUE\n");
            break;

        case AST_MATCH:
            printf("MATCH:\n");
            print_indent(indent + 1);
            printf("value:\n");
            ast_print(node->as.match.value, indent + 2);
            print_indent(indent + 1);
            printf("arms:\n");
            for (MatchArm* arm = node->as.match.arms; arm; arm = arm->next) {
                print_indent(indent + 2);
                printf("ARM:\n");
                print_indent(indent + 3);
                printf("pattern:\n");
                ast_print(arm->pattern, indent + 4);
                if (arm->guard) {
                    print_indent(indent + 3);
                    printf("guard:\n");
                    ast_print(arm->guard, indent + 4);
                }
                print_indent(indent + 3);
                printf("body:\n");
                ast_print(arm->body, indent + 4);
            }
            break;

        case AST_MATCH_ARM:
            printf("MATCH_ARM\n");
            break;

        case AST_EXPR_STMT:
            printf("EXPR_STMT:\n");
            ast_print(node->as.expr_stmt.expr, indent + 1);
            break;
    }
}
