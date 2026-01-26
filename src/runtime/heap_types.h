#pragma once

#include "standard_headers.h"

// Type IDs for heap-allocated objects
enum HeapObjectType : u32 {
    HEAP_TYPE_STRING = 1,
    HEAP_TYPE_ARRAY = 2,
    HEAP_TYPE_OBJECT = 3,
    HEAP_TYPE_CLOSURE = 4,
    HEAP_TYPE_HASH_TABLE = 5,
};

// Heap-allocated string
struct HeapString {
    u32 length;
    u32 capacity;
    char data[];  // Flexible array member
};

// Heap-allocated array  
struct HeapArray {
    u32 length;
    u32 capacity;
    u32 element_size;
    u8 data[];    // Flexible array member
};

// Heap-allocated object (for user-defined structures)
struct HeapObject_UserDef {
    u32 field_count;
    u32 field_data[];  // Flexible array member - stores field values
};

// Functions for heap object creation
HeapString* heap_string_create(struct VM* vm, const char* str, u32 length);
void heap_string_destroy(struct VM* vm, HeapString* str);

HeapArray* heap_array_create(struct VM* vm, u32 initial_capacity, u32 element_size);
void heap_array_destroy(struct VM* vm, HeapArray* array);
void heap_array_resize(struct VM* vm, HeapArray* array, u32 new_capacity);
void heap_array_push(struct VM* vm, HeapArray* array, void* element);

HeapObject_UserDef* heap_object_create(struct VM* vm, u32 field_count);
void heap_object_destroy(struct VM* vm, HeapObject_UserDef* obj);