#include "interpreter.h"
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <cmath>

// Forward declarations
static Value eval_node(Interpreter* interp, ASTNode* node);

// Native functions
static Value native_print(Value* args, int arg_count, Environment* env) {
    (void)env;
    for (int i = 0; i < arg_count; i++) {
        if (i > 0) printf(" ");
        print_value(args[i]);
    }
    printf("\n");
    return make_null();
}

static Value native_len(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_int(0);

    Value val = args[0];
    if (val.type == VAL_ARRAY) {
        return make_int(array_length(&val));
    }
    if (val.type == VAL_STRING) {
        return make_int((int)strlen(val.as.string));
    }
    return make_int(0);
}

static Value native_sqrt(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_float(0.0);

    Value val = args[0];
    double num = 0.0;
    if (val.type == VAL_INT) num = (double)val.as.integer;
    else if (val.type == VAL_FLOAT) num = val.as.floating;

    return make_float(sqrt(num));
}

static Value native_sin(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_float(0.0);

    Value val = args[0];
    double num = 0.0;
    if (val.type == VAL_INT) num = (double)val.as.integer;
    else if (val.type == VAL_FLOAT) num = val.as.floating;

    return make_float(sin(num));
}

static Value native_cos(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_float(0.0);

    Value val = args[0];
    double num = 0.0;
    if (val.type == VAL_INT) num = (double)val.as.integer;
    else if (val.type == VAL_FLOAT) num = val.as.floating;

    return make_float(cos(num));
}

static Value native_abs(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_int(0);

    Value val = args[0];
    if (val.type == VAL_INT) {
        return make_int(val.as.integer < 0 ? -val.as.integer : val.as.integer);
    }
    if (val.type == VAL_FLOAT) {
        return make_float(fabs(val.as.floating));
    }
    return val;
}

static Value native_floor(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_int(0);

    Value val = args[0];
    if (val.type == VAL_INT) return val;
    if (val.type == VAL_FLOAT) return make_int((int64_t)floor(val.as.floating));
    return make_int(0);
}

static Value native_ceil(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_int(0);

    Value val = args[0];
    if (val.type == VAL_INT) return val;
    if (val.type == VAL_FLOAT) return make_int((int64_t)ceil(val.as.floating));
    return make_int(0);
}

static Value native_round(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_int(0);

    Value val = args[0];
    if (val.type == VAL_INT) return val;
    if (val.type == VAL_FLOAT) return make_int((int64_t)round(val.as.floating));
    return make_int(0);
}

static Value native_min(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 2) return make_null();

    Value a = args[0];
    Value b = args[1];

    double va = (a.type == VAL_INT) ? (double)a.as.integer : a.as.floating;
    double vb = (b.type == VAL_INT) ? (double)b.as.integer : b.as.floating;

    if (va < vb) return a;
    return b;
}

static Value native_max(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 2) return make_null();

    Value a = args[0];
    Value b = args[1];

    double va = (a.type == VAL_INT) ? (double)a.as.integer : a.as.floating;
    double vb = (b.type == VAL_INT) ? (double)b.as.integer : b.as.floating;

    if (va > vb) return a;
    return b;
}

static Value native_pow(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 2) return make_float(0.0);

    Value base = args[0];
    Value exp = args[1];

    double b = (base.type == VAL_INT) ? (double)base.as.integer : base.as.floating;
    double e = (exp.type == VAL_INT) ? (double)exp.as.integer : exp.as.floating;

    return make_float(pow(b, e));
}

static Value native_range(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 2) return make_array();

    int64_t start = 0, end = 0, step = 1;

    if (args[0].type == VAL_INT) start = args[0].as.integer;
    if (args[1].type == VAL_INT) end = args[1].as.integer;
    if (arg_count >= 3 && args[2].type == VAL_INT) step = args[2].as.integer;

    if (step == 0) step = 1;

    Value arr = make_array();

    if (step > 0) {
        for (int64_t i = start; i < end; i += step) {
            array_push(&arr, make_int(i));
        }
    }
    else {
        for (int64_t i = start; i > end; i += step) {
            array_push(&arr, make_int(i));
        }
    }

    return arr;
}

static Value native_push(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 2) return make_null();

    if (args[0].type == VAL_ARRAY) {
        array_push(&args[0], args[1]);
    }
    return args[0];
}

static Value native_type(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_string("null");

    switch (args[0].type) {
        case VAL_NULL: return make_string("null");
        case VAL_BOOL: return make_string("bool");
        case VAL_INT: return make_string("int");
        case VAL_FLOAT: return make_string("float");
        case VAL_STRING: return make_string("string");
        case VAL_SYMBOL: return make_string("symbol");
        case VAL_ARRAY: return make_string("array");
        case VAL_OBJECT: return make_string("object");
        case VAL_FUNCTION: return make_string("function");
        case VAL_NATIVE_FN: return make_string("native_function");
    }
    return make_string("unknown");
}

static Value native_str(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_string("");
    return make_string(value_to_string(args[0]));
}

static Value native_int(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_int(0);

    Value val = args[0];
    if (val.type == VAL_INT) return val;
    if (val.type == VAL_FLOAT) return make_int((int64_t)val.as.floating);
    if (val.type == VAL_STRING) return make_int(atoll(val.as.string));
    if (val.type == VAL_BOOL) return make_int(val.as.boolean ? 1 : 0);
    return make_int(0);
}

static Value native_float(Value* args, int arg_count, Environment* env) {
    (void)env;
    if (arg_count < 1) return make_float(0.0);

    Value val = args[0];
    if (val.type == VAL_FLOAT) return val;
    if (val.type == VAL_INT) return make_float((double)val.as.integer);
    if (val.type == VAL_STRING) return make_float(atof(val.as.string));
    return make_float(0.0);
}

// Environment functions
Environment* env_create(Environment* parent) {
    Environment* env = (Environment*)malloc(sizeof(Environment));
    env->entries = nullptr;
    env->parent = parent;
    return env;
}

void env_define(Environment* env, const char* name, Value value) {
    // Check if exists in current scope
    EnvEntry* entry = env->entries;
    while (entry != nullptr) {
        if (strcmp(entry->name, name) == 0) {
            entry->value = value;
            return;
        }
        entry = entry->next;
    }

    // Create new entry
    EnvEntry* new_entry = (EnvEntry*)malloc(sizeof(EnvEntry));
    new_entry->name = strdup(name);
    new_entry->value = value;
    new_entry->next = env->entries;
    env->entries = new_entry;
}

Value env_get(Environment* env, const char* name) {
    Environment* current = env;
    while (current != nullptr) {
        EnvEntry* entry = current->entries;
        while (entry != nullptr) {
            if (strcmp(entry->name, name) == 0) {
                return entry->value;
            }
            entry = entry->next;
        }
        current = current->parent;
    }
    return make_null();
}

bool env_set(Environment* env, const char* name, Value value) {
    Environment* current = env;
    while (current != nullptr) {
        EnvEntry* entry = current->entries;
        while (entry != nullptr) {
            if (strcmp(entry->name, name) == 0) {
                entry->value = value;
                return true;
            }
            entry = entry->next;
        }
        current = current->parent;
    }
    return false;
}

static bool env_exists(Environment* env, const char* name) {
    Environment* current = env;
    while (current != nullptr) {
        EnvEntry* entry = current->entries;
        while (entry != nullptr) {
            if (strcmp(entry->name, name) == 0) {
                return true;
            }
            entry = entry->next;
        }
        current = current->parent;
    }
    return false;
}

void interpreter_init(Interpreter* interp) {
    interp->global_env = env_create(nullptr);
    interp->current_env = interp->global_env;
    interp->control.is_return = false;
    interp->control.is_break = false;
    interp->control.is_continue = false;
    interp->control.return_value = make_null();
    interp->had_error = false;
    interp->error_message[0] = '\0';

    // Register native functions
    env_define(interp->global_env, "print", make_native_fn(native_print));
    env_define(interp->global_env, "len", make_native_fn(native_len));
    env_define(interp->global_env, "sqrt", make_native_fn(native_sqrt));
    env_define(interp->global_env, "sin", make_native_fn(native_sin));
    env_define(interp->global_env, "cos", make_native_fn(native_cos));
    env_define(interp->global_env, "abs", make_native_fn(native_abs));
    env_define(interp->global_env, "floor", make_native_fn(native_floor));
    env_define(interp->global_env, "ceil", make_native_fn(native_ceil));
    env_define(interp->global_env, "round", make_native_fn(native_round));
    env_define(interp->global_env, "min", make_native_fn(native_min));
    env_define(interp->global_env, "max", make_native_fn(native_max));
    env_define(interp->global_env, "pow", make_native_fn(native_pow));
    env_define(interp->global_env, "range", make_native_fn(native_range));
    env_define(interp->global_env, "push", make_native_fn(native_push));
    env_define(interp->global_env, "type", make_native_fn(native_type));
    env_define(interp->global_env, "str", make_native_fn(native_str));
    env_define(interp->global_env, "int", make_native_fn(native_int));
    env_define(interp->global_env, "float", make_native_fn(native_float));
}

static void runtime_error(Interpreter* interp, const char* message) {
    interp->had_error = true;
    snprintf(interp->error_message, sizeof(interp->error_message), "Runtime error: %s", message);
}

static double to_number(Value val) {
    if (val.type == VAL_INT) return (double)val.as.integer;
    if (val.type == VAL_FLOAT) return val.as.floating;
    return 0.0;
}

static Value eval_binary(Interpreter* interp, ASTNode* node) {
    BinaryOp op = node->as.binary.op;
    Value left = eval_node(interp, node->as.binary.left);
    if (interp->had_error) return make_null();

    // Short-circuit evaluation for && and ||
    if (op == OP_AND) {
        if (!value_is_truthy(left)) return make_bool(false);
        Value right = eval_node(interp, node->as.binary.right);
        return make_bool(value_is_truthy(right));
    }
    if (op == OP_OR) {
        if (value_is_truthy(left)) return make_bool(true);
        Value right = eval_node(interp, node->as.binary.right);
        return make_bool(value_is_truthy(right));
    }

    Value right = eval_node(interp, node->as.binary.right);
    if (interp->had_error) return make_null();

    // String concatenation
    if (op == OP_ADD && (left.type == VAL_STRING || right.type == VAL_STRING)) {
        char* ls = value_to_string(left);
        char* rs = value_to_string(right);
        char* result = (char*)malloc(strlen(ls) + strlen(rs) + 1);
        strcpy(result, ls);
        strcat(result, rs);
        free(ls);
        free(rs);
        Value v = make_string(result);
        free(result);
        return v;
    }

    // Numeric operations
    if ((left.type == VAL_INT || left.type == VAL_FLOAT) &&
        (right.type == VAL_INT || right.type == VAL_FLOAT)) {

        bool use_float = (left.type == VAL_FLOAT || right.type == VAL_FLOAT);
        double l = to_number(left);
        double r = to_number(right);

        switch (op) {
            case OP_ADD:
                if (use_float) return make_float(l + r);
                return make_int(left.as.integer + right.as.integer);
            case OP_SUB:
                if (use_float) return make_float(l - r);
                return make_int(left.as.integer - right.as.integer);
            case OP_MUL:
                if (use_float) return make_float(l * r);
                return make_int(left.as.integer * right.as.integer);
            case OP_DIV:
                if (r == 0) {
                    runtime_error(interp, "Division by zero");
                    return make_null();
                }
                return make_float(l / r);
            case OP_MOD:
                if (right.as.integer == 0) {
                    runtime_error(interp, "Modulo by zero");
                    return make_null();
                }
                return make_int(left.as.integer % right.as.integer);
            case OP_POW:
                return make_float(pow(l, r));
            case OP_EQ:
                return make_bool(l == r);
            case OP_NE:
                return make_bool(l != r);
            case OP_LT:
                return make_bool(l < r);
            case OP_GT:
                return make_bool(l > r);
            case OP_LE:
                return make_bool(l <= r);
            case OP_GE:
                return make_bool(l >= r);
            default:
                break;
        }
    }

    // Equality for other types
    if (op == OP_EQ) return make_bool(values_equal(left, right));
    if (op == OP_NE) return make_bool(!values_equal(left, right));

    // Pipe operator - treat as function call
    if (op == OP_PIPE) {
        // right should be a function call, pass left as first argument
        if (right.type == VAL_FUNCTION || right.type == VAL_NATIVE_FN) {
            Value args[1] = { left };
            if (right.type == VAL_NATIVE_FN) {
                return right.as.native_fn(args, 1, interp->current_env);
            }
            // User function - need to handle differently
            // For simplicity, just error for now
            runtime_error(interp, "Pipe to user function not fully implemented");
            return make_null();
        }
    }

    runtime_error(interp, "Invalid operands for binary operator");
    return make_null();
}

static Value eval_unary(Interpreter* interp, ASTNode* node) {
    Value operand = eval_node(interp, node->as.unary.operand);
    if (interp->had_error) return make_null();

    switch (node->as.unary.op) {
        case OP_NEG:
            if (operand.type == VAL_INT) return make_int(-operand.as.integer);
            if (operand.type == VAL_FLOAT) return make_float(-operand.as.floating);
            break;
        case OP_NOT:
            return make_bool(!value_is_truthy(operand));
    }

    return make_null();
}

static Value eval_call(Interpreter* interp, ASTNode* node) {
    Value callee = eval_node(interp, node->as.call.callee);
    if (interp->had_error) return make_null();

    // Count and evaluate arguments
    int arg_count = 0;
    for (ASTNodeList* arg = node->as.call.args; arg != nullptr; arg = arg->next) {
        arg_count++;
    }

    Value* args = (Value*)malloc(sizeof(Value) * (arg_count > 0 ? arg_count : 1));
    int i = 0;
    for (ASTNodeList* arg = node->as.call.args; arg != nullptr; arg = arg->next) {
        args[i++] = eval_node(interp, arg->node);
        if (interp->had_error) {
            free(args);
            return make_null();
        }
    }

    Value result = make_null();

    if (callee.type == VAL_NATIVE_FN) {
        result = callee.as.native_fn(args, arg_count, interp->current_env);
    }
    else if (callee.type == VAL_FUNCTION) {
        Function* func = callee.as.function;

        // Create new environment
        Environment* func_env = env_create(func->closure);

        // Bind parameters
        int param_limit = (arg_count < func->param_count) ? arg_count : func->param_count;
        for (int j = 0; j < param_limit; j++) {
            env_define(func_env, func->params[j], args[j]);
        }

        // Execute body
        Environment* prev_env = interp->current_env;
        interp->current_env = func_env;

        result = eval_node(interp, func->body);

        if (interp->control.is_return) {
            result = interp->control.return_value;
            interp->control.is_return = false;
            interp->control.return_value = make_null();
        }

        interp->current_env = prev_env;
    }
    else {
        runtime_error(interp, "Cannot call non-function value");
    }

    free(args);
    return result;
}

static Value eval_index(Interpreter* interp, ASTNode* node) {
    Value object = eval_node(interp, node->as.index.object);
    Value index = eval_node(interp, node->as.index.index);

    if (object.type == VAL_ARRAY) {
        if (index.type != VAL_INT) {
            runtime_error(interp, "Array index must be an integer");
            return make_null();
        }
        return array_get(&object, (int)index.as.integer);
    }

    if (object.type == VAL_STRING) {
        if (index.type != VAL_INT) {
            runtime_error(interp, "String index must be an integer");
            return make_null();
        }
        int64_t idx = index.as.integer;
        int len = (int)strlen(object.as.string);
        if (idx < 0) idx = len + idx;
        if (idx < 0 || idx >= len) return make_null();
        char buf[2] = { object.as.string[idx], '\0' };
        return make_string(buf);
    }

    if (object.type == VAL_OBJECT) {
        if (index.type != VAL_STRING) {
            runtime_error(interp, "Object key must be a string");
            return make_null();
        }
        return object_get(&object, index.as.string);
    }

    return make_null();
}

static Value eval_member(Interpreter* interp, ASTNode* node) {
    Value object = eval_node(interp, node->as.member.object);
    const char* member = node->as.member.member;

    if (object.type == VAL_OBJECT) {
        return object_get(&object, member);
    }

    // Built-in properties
    if (object.type == VAL_ARRAY && strcmp(member, "length") == 0) {
        return make_int(array_length(&object));
    }

    if (object.type == VAL_STRING && strcmp(member, "length") == 0) {
        return make_int((int)strlen(object.as.string));
    }

    return make_null();
}

static Value eval_array(Interpreter* interp, ASTNode* node) {
    Value arr = make_array();

    for (ASTNodeList* elem = node->as.array.elements; elem != nullptr; elem = elem->next) {
        Value val = eval_node(interp, elem->node);
        if (interp->had_error) return make_null();
        array_push(&arr, val);
    }

    return arr;
}

static Value eval_object(Interpreter* interp, ASTNode* node) {
    Value obj = make_object();

    for (ObjectFieldNode* field = node->as.object.fields; field != nullptr; field = field->next) {
        Value val = eval_node(interp, field->value);
        if (interp->had_error) return make_null();
        object_set(&obj, field->key, val);
    }

    return obj;
}

static Value eval_block(Interpreter* interp, ASTNode* node) {
    Value result = make_null();

    for (ASTNodeList* stmt = node->as.block.statements; stmt != nullptr; stmt = stmt->next) {
        result = eval_node(interp, stmt->node);

        if (interp->control.is_return || interp->control.is_break ||
            interp->control.is_continue || interp->had_error) {
            break;
        }
    }

    return result;
}

static Value eval_var_decl(Interpreter* interp, ASTNode* node) {
    Value value = make_null();

    if (node->as.var_decl.initializer != nullptr) {
        value = eval_node(interp, node->as.var_decl.initializer);
        if (interp->had_error) return make_null();
    }

    // For 'state' variables, only initialize if not already defined
    if (node->as.var_decl.is_state) {
        if (!env_exists(interp->current_env, node->as.var_decl.name)) {
            env_define(interp->current_env, node->as.var_decl.name, value);
        }
    }
    else {
        env_define(interp->current_env, node->as.var_decl.name, value);
    }

    return make_null();
}

static Value eval_fn_decl(Interpreter* interp, ASTNode* node) {
    // Convert params
    int param_count = 0;
    for (ParamNode* p = node->as.fn_decl.params; p != nullptr; p = p->next) {
        param_count++;
    }

    char** params = (char**)malloc(sizeof(char*) * (param_count > 0 ? param_count : 1));
    int i = 0;
    for (ParamNode* p = node->as.fn_decl.params; p != nullptr; p = p->next) {
        params[i++] = strdup(p->name);
    }

    Value func = make_function(params, param_count, node->as.fn_decl.body, interp->current_env);
    env_define(interp->current_env, node->as.fn_decl.name, func);

    return make_null();
}

static Value eval_assign(Interpreter* interp, ASTNode* node) {
    Value value = eval_node(interp, node->as.assign.value);
    if (interp->had_error) return make_null();

    ASTNode* target = node->as.assign.target;

    if (target->type == AST_IDENTIFIER) {
        if (!env_set(interp->current_env, target->as.identifier.name, value)) {
            // Define if not exists (for compatibility)
            env_define(interp->current_env, target->as.identifier.name, value);
        }
    }
    else if (target->type == AST_INDEX) {
        Value obj = eval_node(interp, target->as.index.object);
        Value idx = eval_node(interp, target->as.index.index);

        if (obj.type == VAL_ARRAY && idx.type == VAL_INT) {
            // Find and update array element
            int index = (int)idx.as.integer;
            if (index < 0) index = obj.as.array->length + index;

            ArrayEntry* entry = obj.as.array->head;
            for (int j = 0; j < index && entry != nullptr; j++) {
                entry = entry->next;
            }
            if (entry != nullptr) {
                entry->value = value;
            }
        }
        else if (obj.type == VAL_OBJECT && idx.type == VAL_STRING) {
            object_set(&obj, idx.as.string, value);
        }
    }
    else if (target->type == AST_MEMBER) {
        Value obj = eval_node(interp, target->as.member.object);
        if (obj.type == VAL_OBJECT) {
            object_set(&obj, target->as.member.member, value);
        }
    }

    return value;
}

static Value eval_compound_assign(Interpreter* interp, ASTNode* node) {
    ASTNode* target = node->as.compound_assign.target;
    Value old_val = make_null();

    if (target->type == AST_IDENTIFIER) {
        old_val = env_get(interp->current_env, target->as.identifier.name);
    }
    else if (target->type == AST_INDEX) {
        Value obj = eval_node(interp, target->as.index.object);
        Value idx = eval_node(interp, target->as.index.index);
        if (obj.type == VAL_ARRAY) {
            old_val = array_get(&obj, (int)idx.as.integer);
        }
    }
    else if (target->type == AST_MEMBER) {
        Value obj = eval_node(interp, target->as.member.object);
        if (obj.type == VAL_OBJECT) {
            old_val = object_get(&obj, target->as.member.member);
        }
    }

    Value delta = eval_node(interp, node->as.compound_assign.value);
    if (interp->had_error) return make_null();

    Value new_val = make_null();
    double old_num = to_number(old_val);
    double delta_num = to_number(delta);

    bool use_float = (old_val.type == VAL_FLOAT || delta.type == VAL_FLOAT);

    switch (node->as.compound_assign.op) {
        case COMP_ADD:
            if (old_val.type == VAL_STRING || delta.type == VAL_STRING) {
                // String concatenation
                char* os = value_to_string(old_val);
                char* ds = value_to_string(delta);
                char* result = (char*)malloc(strlen(os) + strlen(ds) + 1);
                strcpy(result, os);
                strcat(result, ds);
                new_val = make_string(result);
                free(os);
                free(ds);
                free(result);
            }
            else if (use_float) {
                new_val = make_float(old_num + delta_num);
            }
            else {
                new_val = make_int(old_val.as.integer + delta.as.integer);
            }
            break;
        case COMP_SUB:
            if (use_float) new_val = make_float(old_num - delta_num);
            else new_val = make_int(old_val.as.integer - delta.as.integer);
            break;
        case COMP_MUL:
            if (use_float) new_val = make_float(old_num * delta_num);
            else new_val = make_int(old_val.as.integer * delta.as.integer);
            break;
        case COMP_DIV:
            new_val = make_float(old_num / delta_num);
            break;
        case COMP_MOD:
            new_val = make_int(old_val.as.integer % delta.as.integer);
            break;
    }

    // Assign back
    if (target->type == AST_IDENTIFIER) {
        env_set(interp->current_env, target->as.identifier.name, new_val);
    }
    else if (target->type == AST_INDEX) {
        Value obj = eval_node(interp, target->as.index.object);
        Value idx = eval_node(interp, target->as.index.index);
        if (obj.type == VAL_ARRAY && idx.type == VAL_INT) {
            int index = (int)idx.as.integer;
            if (index < 0) index = obj.as.array->length + index;
            ArrayEntry* entry = obj.as.array->head;
            for (int j = 0; j < index && entry != nullptr; j++) {
                entry = entry->next;
            }
            if (entry != nullptr) {
                entry->value = new_val;
            }
        }
    }
    else if (target->type == AST_MEMBER) {
        Value obj = eval_node(interp, target->as.member.object);
        if (obj.type == VAL_OBJECT) {
            object_set(&obj, target->as.member.member, new_val);
        }
    }

    return new_val;
}

static Value eval_return(Interpreter* interp, ASTNode* node) {
    Value value = make_null();
    if (node->as.return_stmt.value != nullptr) {
        value = eval_node(interp, node->as.return_stmt.value);
    }
    interp->control.is_return = true;
    interp->control.return_value = value;
    return value;
}

static Value eval_if(Interpreter* interp, ASTNode* node) {
    Value condition = eval_node(interp, node->as.if_stmt.condition);
    if (interp->had_error) return make_null();

    if (value_is_truthy(condition)) {
        return eval_node(interp, node->as.if_stmt.then_branch);
    }
    else if (node->as.if_stmt.else_branch != nullptr) {
        return eval_node(interp, node->as.if_stmt.else_branch);
    }

    return make_null();
}

static Value eval_while(Interpreter* interp, ASTNode* node) {
    Value result = make_null();

    while (true) {
        Value condition = eval_node(interp, node->as.while_loop.condition);
        if (interp->had_error) return make_null();

        if (!value_is_truthy(condition)) break;

        result = eval_node(interp, node->as.while_loop.body);

        if (interp->control.is_return || interp->had_error) break;
        if (interp->control.is_break) {
            interp->control.is_break = false;
            break;
        }
        if (interp->control.is_continue) {
            interp->control.is_continue = false;
        }
    }

    return result;
}

static Value eval_for(Interpreter* interp, ASTNode* node) {
    Value iterable = eval_node(interp, node->as.for_loop.iterable);
    if (interp->had_error) return make_null();

    Value result = make_null();
    Environment* loop_env = env_create(interp->current_env);
    Environment* prev_env = interp->current_env;
    interp->current_env = loop_env;

    if (iterable.type == VAL_ARRAY) {
        ArrayEntry* entry = iterable.as.array->head;
        while (entry != nullptr) {
            env_define(loop_env, node->as.for_loop.var_name, entry->value);

            result = eval_node(interp, node->as.for_loop.body);

            if (interp->control.is_return || interp->had_error) break;
            if (interp->control.is_break) {
                interp->control.is_break = false;
                break;
            }
            if (interp->control.is_continue) {
                interp->control.is_continue = false;
            }

            entry = entry->next;
        }
    }

    interp->current_env = prev_env;
    return result;
}

static Value eval_loop(Interpreter* interp, ASTNode* node) {
    Value result = make_null();

    while (true) {
        result = eval_node(interp, node->as.loop.body);

        if (interp->control.is_return || interp->had_error) break;
        if (interp->control.is_break) {
            interp->control.is_break = false;
            break;
        }
        if (interp->control.is_continue) {
            interp->control.is_continue = false;
        }
    }

    return result;
}

static bool pattern_matches(Interpreter* interp, ASTNode* pattern, Value val, Environment* env) {
    if (pattern->type == AST_IDENTIFIER) {
        const char* name = pattern->as.identifier.name;
        if (strcmp(name, "_") == 0) {
            // Wildcard - always matches
            return true;
        }
        // Bind variable
        env_define(env, name, val);
        return true;
    }

    if (pattern->type == AST_INT_LITERAL) {
        return val.type == VAL_INT && val.as.integer == pattern->as.int_value;
    }

    if (pattern->type == AST_FLOAT_LITERAL) {
        return val.type == VAL_FLOAT && val.as.floating == pattern->as.float_value;
    }

    if (pattern->type == AST_STRING_LITERAL) {
        return val.type == VAL_STRING && strcmp(val.as.string, pattern->as.string_value) == 0;
    }

    if (pattern->type == AST_BOOL_LITERAL) {
        return val.type == VAL_BOOL && val.as.boolean == pattern->as.bool_value;
    }

    if (pattern->type == AST_NULL_LITERAL) {
        return val.type == VAL_NULL;
    }

    if (pattern->type == AST_SYMBOL_LITERAL) {
        return val.type == VAL_SYMBOL && strcmp(val.as.symbol, pattern->as.symbol_value) == 0;
    }

    // For other patterns, evaluate and compare
    Value pattern_val = eval_node(interp, pattern);
    return values_equal(pattern_val, val);
}

static Value eval_match(Interpreter* interp, ASTNode* node) {
    Value val = eval_node(interp, node->as.match.value);
    if (interp->had_error) return make_null();

    for (MatchArm* arm = node->as.match.arms; arm != nullptr; arm = arm->next) {
        Environment* match_env = env_create(interp->current_env);
        Environment* prev_env = interp->current_env;
        interp->current_env = match_env;

        bool matches = pattern_matches(interp, arm->pattern, val, match_env);

        if (matches && arm->guard != nullptr) {
            Value guard_result = eval_node(interp, arm->guard);
            matches = value_is_truthy(guard_result);
        }

        if (matches) {
            Value result = eval_node(interp, arm->body);
            interp->current_env = prev_env;
            return result;
        }

        interp->current_env = prev_env;
    }

    return make_null();
}

static Value eval_lambda(Interpreter* interp, ASTNode* node) {
    int param_count = 0;
    for (ParamNode* p = node->as.lambda.params; p != nullptr; p = p->next) {
        param_count++;
    }

    char** params = (char**)malloc(sizeof(char*) * (param_count > 0 ? param_count : 1));
    int i = 0;
    for (ParamNode* p = node->as.lambda.params; p != nullptr; p = p->next) {
        params[i++] = strdup(p->name);
    }

    return make_function(params, param_count, node->as.lambda.body, interp->current_env);
}

static Value eval_node(Interpreter* interp, ASTNode* node) {
    if (node == nullptr) return make_null();
    if (interp->had_error) return make_null();

    switch (node->type) {
        case AST_INT_LITERAL:
            return make_int(node->as.int_value);

        case AST_FLOAT_LITERAL:
            return make_float(node->as.float_value);

        case AST_STRING_LITERAL:
            return make_string(node->as.string_value);

        case AST_BOOL_LITERAL:
            return make_bool(node->as.bool_value);

        case AST_NULL_LITERAL:
            return make_null();

        case AST_SYMBOL_LITERAL:
            return make_symbol(node->as.symbol_value);

        case AST_ARRAY_LITERAL:
            return eval_array(interp, node);

        case AST_OBJECT_LITERAL:
            return eval_object(interp, node);

        case AST_IDENTIFIER:
            return env_get(interp->current_env, node->as.identifier.name);

        case AST_BINARY_OP:
            return eval_binary(interp, node);

        case AST_UNARY_OP:
            return eval_unary(interp, node);

        case AST_CALL:
            return eval_call(interp, node);

        case AST_INDEX:
            return eval_index(interp, node);

        case AST_MEMBER:
            return eval_member(interp, node);

        case AST_LAMBDA:
            return eval_lambda(interp, node);

        case AST_PROGRAM:
        case AST_BLOCK:
            return eval_block(interp, node);

        case AST_VAR_DECL:
            return eval_var_decl(interp, node);

        case AST_ASSIGN:
            return eval_assign(interp, node);

        case AST_COMPOUND_ASSIGN:
            return eval_compound_assign(interp, node);

        case AST_FN_DECL:
            return eval_fn_decl(interp, node);

        case AST_RETURN:
            return eval_return(interp, node);

        case AST_IF:
            return eval_if(interp, node);

        case AST_WHILE:
            return eval_while(interp, node);

        case AST_FOR:
            return eval_for(interp, node);

        case AST_LOOP:
            return eval_loop(interp, node);

        case AST_BREAK:
            interp->control.is_break = true;
            return make_null();

        case AST_CONTINUE:
            interp->control.is_continue = true;
            return make_null();

        case AST_MATCH:
            return eval_match(interp, node);

        case AST_MATCH_ARM:
            // Should not be evaluated directly
            return make_null();

        case AST_EXPR_STMT:
            return eval_node(interp, node->as.expr_stmt.expr);
    }

    return make_null();
}

Value interpreter_eval(Interpreter* interp, ASTNode* node) {
    return eval_node(interp, node);
}

void interpreter_run(Interpreter* interp, ASTNode* program) {
    eval_node(interp, program);
}
