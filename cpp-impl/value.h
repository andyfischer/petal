#pragma once

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <cstdio>

// Forward declarations
struct Value;
struct Object;
struct Array;
struct Function;
struct Environment;

enum ValueType {
    VAL_NULL,
    VAL_BOOL,
    VAL_INT,
    VAL_FLOAT,
    VAL_STRING,
    VAL_SYMBOL,
    VAL_ARRAY,
    VAL_OBJECT,
    VAL_FUNCTION,
    VAL_NATIVE_FN
};

typedef Value (*NativeFn)(Value* args, int arg_count, Environment* env);

struct Value {
    ValueType type;
    union {
        bool boolean;
        int64_t integer;
        double floating;
        char* string;
        char* symbol;
        Array* array;
        Object* object;
        Function* function;
        NativeFn native_fn;
    } as;
};

struct ArrayEntry {
    Value value;
    ArrayEntry* next;
};

struct Array {
    ArrayEntry* head;
    ArrayEntry* tail;
    int length;
};

struct ObjectField {
    char* key;
    Value value;
    ObjectField* next;
};

struct Object {
    ObjectField* fields;
};

// AST forward declarations
struct ASTNode;

struct Function {
    char** params;
    int param_count;
    ASTNode* body;
    Environment* closure;
};

// Value constructors
Value make_null();
Value make_bool(bool b);
Value make_int(int64_t i);
Value make_float(double f);
Value make_string(const char* s);
Value make_symbol(const char* s);
Value make_array();
Value make_object();
Value make_function(char** params, int param_count, ASTNode* body, Environment* closure);
Value make_native_fn(NativeFn fn);

// Value operations
void array_push(Value* arr, Value val);
Value array_get(Value* arr, int index);
int array_length(Value* arr);

void object_set(Value* obj, const char* key, Value val);
Value object_get(Value* obj, const char* key);
bool object_has(Value* obj, const char* key);

// Value utilities
void print_value(Value val);
char* value_to_string(Value val);
Value value_copy(Value val);
void value_free(Value val);
bool values_equal(Value a, Value b);
bool value_is_truthy(Value val);
