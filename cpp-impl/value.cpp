#include "value.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cmath>

Value make_null() {
    Value v;
    v.type = VAL_NULL;
    return v;
}

Value make_bool(bool b) {
    Value v;
    v.type = VAL_BOOL;
    v.as.boolean = b;
    return v;
}

Value make_int(int64_t i) {
    Value v;
    v.type = VAL_INT;
    v.as.integer = i;
    return v;
}

Value make_float(double f) {
    Value v;
    v.type = VAL_FLOAT;
    v.as.floating = f;
    return v;
}

Value make_string(const char* s) {
    Value v;
    v.type = VAL_STRING;
    v.as.string = strdup(s);
    return v;
}

Value make_symbol(const char* s) {
    Value v;
    v.type = VAL_SYMBOL;
    v.as.symbol = strdup(s);
    return v;
}

Value make_array() {
    Value v;
    v.type = VAL_ARRAY;
    v.as.array = (Array*)malloc(sizeof(Array));
    v.as.array->head = nullptr;
    v.as.array->tail = nullptr;
    v.as.array->length = 0;
    return v;
}

Value make_object() {
    Value v;
    v.type = VAL_OBJECT;
    v.as.object = (Object*)malloc(sizeof(Object));
    v.as.object->fields = nullptr;
    return v;
}

Value make_function(char** params, int param_count, ASTNode* body, Environment* closure) {
    Value v;
    v.type = VAL_FUNCTION;
    v.as.function = (Function*)malloc(sizeof(Function));
    v.as.function->params = params;
    v.as.function->param_count = param_count;
    v.as.function->body = body;
    v.as.function->closure = closure;
    return v;
}

Value make_native_fn(NativeFn fn) {
    Value v;
    v.type = VAL_NATIVE_FN;
    v.as.native_fn = fn;
    return v;
}

void array_push(Value* arr, Value val) {
    if (arr->type != VAL_ARRAY) return;

    ArrayEntry* entry = (ArrayEntry*)malloc(sizeof(ArrayEntry));
    entry->value = val;
    entry->next = nullptr;

    if (arr->as.array->tail == nullptr) {
        arr->as.array->head = entry;
        arr->as.array->tail = entry;
    }
    else {
        arr->as.array->tail->next = entry;
        arr->as.array->tail = entry;
    }
    arr->as.array->length++;
}

Value array_get(Value* arr, int index) {
    if (arr->type != VAL_ARRAY) return make_null();

    int len = arr->as.array->length;
    if (index < 0) index = len + index;
    if (index < 0 || index >= len) return make_null();

    ArrayEntry* entry = arr->as.array->head;
    for (int i = 0; i < index; i++) {
        entry = entry->next;
    }
    return entry->value;
}

int array_length(Value* arr) {
    if (arr->type != VAL_ARRAY) return 0;
    return arr->as.array->length;
}

void object_set(Value* obj, const char* key, Value val) {
    if (obj->type != VAL_OBJECT) return;

    // Check if key exists
    ObjectField* field = obj->as.object->fields;
    while (field) {
        if (strcmp(field->key, key) == 0) {
            field->value = val;
            return;
        }
        field = field->next;
    }

    // Add new field
    ObjectField* new_field = (ObjectField*)malloc(sizeof(ObjectField));
    new_field->key = strdup(key);
    new_field->value = val;
    new_field->next = obj->as.object->fields;
    obj->as.object->fields = new_field;
}

Value object_get(Value* obj, const char* key) {
    if (obj->type != VAL_OBJECT) return make_null();

    ObjectField* field = obj->as.object->fields;
    while (field) {
        if (strcmp(field->key, key) == 0) {
            return field->value;
        }
        field = field->next;
    }
    return make_null();
}

bool object_has(Value* obj, const char* key) {
    if (obj->type != VAL_OBJECT) return false;

    ObjectField* field = obj->as.object->fields;
    while (field) {
        if (strcmp(field->key, key) == 0) {
            return true;
        }
        field = field->next;
    }
    return false;
}

void print_value(Value val) {
    switch (val.type) {
        case VAL_NULL:
            printf("null");
            break;
        case VAL_BOOL:
            printf("%s", val.as.boolean ? "true" : "false");
            break;
        case VAL_INT:
            printf("%lld", (long long)val.as.integer);
            break;
        case VAL_FLOAT:
            printf("%g", val.as.floating);
            break;
        case VAL_STRING:
            printf("%s", val.as.string);
            break;
        case VAL_SYMBOL:
            printf(":%s", val.as.symbol);
            break;
        case VAL_ARRAY: {
            printf("[");
            ArrayEntry* entry = val.as.array->head;
            bool first = true;
            while (entry) {
                if (!first) printf(", ");
                print_value(entry->value);
                first = false;
                entry = entry->next;
            }
            printf("]");
            break;
        }
        case VAL_OBJECT: {
            printf("{");
            ObjectField* field = val.as.object->fields;
            bool first = true;
            while (field) {
                if (!first) printf(", ");
                printf("%s: ", field->key);
                print_value(field->value);
                first = false;
                field = field->next;
            }
            printf("}");
            break;
        }
        case VAL_FUNCTION:
            printf("<function>");
            break;
        case VAL_NATIVE_FN:
            printf("<native function>");
            break;
    }
}

char* value_to_string(Value val) {
    char* buf = (char*)malloc(256);
    switch (val.type) {
        case VAL_NULL:
            strcpy(buf, "null");
            break;
        case VAL_BOOL:
            strcpy(buf, val.as.boolean ? "true" : "false");
            break;
        case VAL_INT:
            snprintf(buf, 256, "%lld", (long long)val.as.integer);
            break;
        case VAL_FLOAT:
            snprintf(buf, 256, "%g", val.as.floating);
            break;
        case VAL_STRING:
            free(buf);
            return strdup(val.as.string);
        case VAL_SYMBOL:
            snprintf(buf, 256, ":%s", val.as.symbol);
            break;
        case VAL_ARRAY:
            strcpy(buf, "[array]");
            break;
        case VAL_OBJECT:
            strcpy(buf, "[object]");
            break;
        case VAL_FUNCTION:
            strcpy(buf, "<function>");
            break;
        case VAL_NATIVE_FN:
            strcpy(buf, "<native function>");
            break;
    }
    return buf;
}

Value value_copy(Value val) {
    switch (val.type) {
        case VAL_STRING:
            return make_string(val.as.string);
        case VAL_SYMBOL:
            return make_symbol(val.as.symbol);
        case VAL_ARRAY: {
            Value new_arr = make_array();
            ArrayEntry* entry = val.as.array->head;
            while (entry) {
                array_push(&new_arr, value_copy(entry->value));
                entry = entry->next;
            }
            return new_arr;
        }
        case VAL_OBJECT: {
            Value new_obj = make_object();
            ObjectField* field = val.as.object->fields;
            while (field) {
                object_set(&new_obj, field->key, value_copy(field->value));
                field = field->next;
            }
            return new_obj;
        }
        default:
            return val;
    }
}

void value_free(Value val) {
    switch (val.type) {
        case VAL_STRING:
            free(val.as.string);
            break;
        case VAL_SYMBOL:
            free(val.as.symbol);
            break;
        case VAL_ARRAY: {
            ArrayEntry* entry = val.as.array->head;
            while (entry) {
                ArrayEntry* next = entry->next;
                value_free(entry->value);
                free(entry);
                entry = next;
            }
            free(val.as.array);
            break;
        }
        case VAL_OBJECT: {
            ObjectField* field = val.as.object->fields;
            while (field) {
                ObjectField* next = field->next;
                free(field->key);
                value_free(field->value);
                free(field);
                field = next;
            }
            free(val.as.object);
            break;
        }
        default:
            break;
    }
}

bool values_equal(Value a, Value b) {
    if (a.type != b.type) return false;

    switch (a.type) {
        case VAL_NULL:
            return true;
        case VAL_BOOL:
            return a.as.boolean == b.as.boolean;
        case VAL_INT:
            return a.as.integer == b.as.integer;
        case VAL_FLOAT:
            return a.as.floating == b.as.floating;
        case VAL_STRING:
            return strcmp(a.as.string, b.as.string) == 0;
        case VAL_SYMBOL:
            return strcmp(a.as.symbol, b.as.symbol) == 0;
        default:
            return false;
    }
}

bool value_is_truthy(Value val) {
    switch (val.type) {
        case VAL_NULL:
            return false;
        case VAL_BOOL:
            return val.as.boolean;
        case VAL_INT:
            return val.as.integer != 0;
        case VAL_FLOAT:
            return val.as.floating != 0.0;
        case VAL_STRING:
            return val.as.string[0] != '\0';
        default:
            return true;
    }
}
