#include "runtime/heap_types.h"
#include "runtime/vm.h"
#include <cstring>
#include <cassert>

HeapString* heap_string_create(struct VM* vm, const char* str, u32 length) {
    if (!str || length == 0) return nullptr;
    
    u32 total_size = sizeof(HeapString) + length + 1;  // +1 for null terminator
    HeapString* heap_str = (HeapString*)vm_heap_alloc(vm, total_size, HEAP_TYPE_STRING);
    
    if (heap_str) {
        heap_str->length = length;
        heap_str->capacity = length + 1;
        memcpy(heap_str->data, str, length);
        heap_str->data[length] = '\0';
    }
    
    return heap_str;
}

void heap_string_destroy(struct VM* vm, HeapString* str) {
    if (str) {
        vm_heap_free(vm, str);
    }
}

HeapArray* heap_array_create(struct VM* vm, u32 initial_capacity, u32 element_size) {
    if (element_size == 0) return nullptr;
    
    u32 total_size = sizeof(HeapArray) + (initial_capacity * element_size);
    HeapArray* array = (HeapArray*)vm_heap_alloc(vm, total_size, HEAP_TYPE_ARRAY);
    
    if (array) {
        array->length = 0;
        array->capacity = initial_capacity;
        array->element_size = element_size;
        memset(array->data, 0, initial_capacity * element_size);
    }
    
    return array;
}

void heap_array_destroy(struct VM* vm, HeapArray* array) {
    if (array) {
        vm_heap_free(vm, array);
    }
}

void heap_array_resize(struct VM* vm, HeapArray* array, u32 new_capacity) {
    if (!array || new_capacity <= array->capacity) return;
    
    // Allocate new array with larger capacity
    u32 new_total_size = sizeof(HeapArray) + (new_capacity * array->element_size);
    HeapArray* new_array = (HeapArray*)vm_heap_alloc(vm, new_total_size, HEAP_TYPE_ARRAY);
    
    if (new_array) {
        // Copy existing data
        new_array->length = array->length;
        new_array->capacity = new_capacity;
        new_array->element_size = array->element_size;
        
        u32 copy_size = array->length * array->element_size;
        memcpy(new_array->data, array->data, copy_size);
        
        // Clear remaining space
        memset(new_array->data + copy_size, 0, (new_capacity - array->length) * array->element_size);
        
        // Replace old array data in place
        // Note: This is a simplified approach - in a real implementation,
        // we'd need to update all references to this array
        memcpy(array, new_array, sizeof(HeapArray));
        
        vm_heap_free(vm, new_array);
    }
}

void heap_array_push(struct VM* vm, HeapArray* array, void* element) {
    if (!array || !element) return;
    
    // Resize if necessary
    if (array->length >= array->capacity) {
        u32 new_capacity = array->capacity == 0 ? 4 : array->capacity * 2;
        heap_array_resize(vm, array, new_capacity);
    }
    
    // Add element
    u8* dest = array->data + (array->length * array->element_size);
    memcpy(dest, element, array->element_size);
    array->length++;
}

HeapObject_UserDef* heap_object_create(struct VM* vm, u32 field_count) {
    u32 total_size = sizeof(HeapObject_UserDef) + (field_count * sizeof(u32));
    HeapObject_UserDef* obj = (HeapObject_UserDef*)vm_heap_alloc(vm, total_size, HEAP_TYPE_OBJECT);
    
    if (obj) {
        obj->field_count = field_count;
        memset(obj->field_data, 0, field_count * sizeof(u32));
    }
    
    return obj;
}

void heap_object_destroy(struct VM* vm, HeapObject_UserDef* obj) {
    if (obj) {
        vm_heap_free(vm, obj);
    }
}