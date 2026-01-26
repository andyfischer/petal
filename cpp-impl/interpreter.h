#pragma once

#include "ast.h"
#include "value.h"

struct EnvEntry {
    char* name;
    Value value;
    EnvEntry* next;
};

struct Environment {
    EnvEntry* entries;
    Environment* parent;
};

struct ControlFlow {
    bool is_return;
    bool is_break;
    bool is_continue;
    Value return_value;
};

struct Interpreter {
    Environment* global_env;
    Environment* current_env;
    ControlFlow control;
    bool had_error;
    char error_message[512];
};

void interpreter_init(Interpreter* interp);
Value interpreter_eval(Interpreter* interp, ASTNode* node);
void interpreter_run(Interpreter* interp, ASTNode* program);

Environment* env_create(Environment* parent);
void env_define(Environment* env, const char* name, Value value);
Value env_get(Environment* env, const char* name);
bool env_set(Environment* env, const char* name, Value value);
