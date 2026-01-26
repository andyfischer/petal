#pragma once

#include "standard_headers.h"

// Heap memory management for dynamic objects in Petal VM
// This provides malloc/free style allocation with garbage collection support

struct HeapObject {
    u32 size;           // Size of the allocated object in bytes
    u32 type_id;        // Type identifier for GC and debugging
    bool marked;        // Mark bit for garbage collection
    HeapObject* next;   // Next object in free list (when freed)
};

struct FreeBlock {
    u32 size;
    FreeBlock* next;
};

struct Heap {
    u8* memory;             // Base address of heap memory
    u32 size;               // Total heap size in bytes
    u32 used;               // Currently used bytes
    u32 next_alloc;         // Next allocation pointer (bump allocator)
    
    // Free list management - segregated by size classes
    FreeBlock* free_lists[16];  // Free lists for different size classes
    
    // GC state
    HeapObject* all_objects;    // Linked list of all allocated objects
    u32 gc_threshold;           // Trigger GC when used > threshold
    
    // Statistics
    u32 total_allocations;
    u32 total_deallocations;
    u32 gc_runs;
};

// Heap initialization and cleanup
void heap_init(Heap* heap, u32 initial_size);
void heap_cleanup(Heap* heap);

// Memory allocation
void* heap_alloc(Heap* heap, u32 size, u32 type_id);
void heap_free(Heap* heap, void* ptr);

// Garbage collection
void heap_gc_mark(Heap* heap, void* root_ptr);
void heap_gc_sweep(Heap* heap);
void heap_gc_run(Heap* heap);

// Heap management
void heap_grow(Heap* heap, u32 new_size);
u32 heap_get_object_size(void* ptr);
u32 heap_get_object_type(void* ptr);

// Statistics and debugging
void heap_print_stats(Heap* heap);
bool heap_validate(Heap* heap);

// Size class management for free lists
u32 size_class_for_size(u32 size);
u32 size_for_size_class(u32 size_class);