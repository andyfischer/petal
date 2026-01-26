#include "runtime/heap.h"
#include <cstdlib>
#include <cstring>
#include <cassert>

// Size classes for free list management (powers of 2 from 16 to 32KB)
static const u32 SIZE_CLASS_SIZES[] = {
    16, 32, 64, 128, 256, 512, 1024, 2048,
    4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288
};

void heap_init(Heap* heap, u32 initial_size) {
    // Align size to page boundary
    initial_size = (initial_size + 4095) & ~4095;
    
    heap->memory = (u8*)malloc(initial_size);
    assert(heap->memory != nullptr);
    
    heap->size = initial_size;
    heap->used = 0;
    heap->next_alloc = 0;
    heap->all_objects = nullptr;
    heap->gc_threshold = initial_size / 2;  // Trigger GC at 50% usage
    
    // Initialize free lists
    for (int i = 0; i < 16; i++) {
        heap->free_lists[i] = nullptr;
    }
    
    // Initialize statistics
    heap->total_allocations = 0;
    heap->total_deallocations = 0;
    heap->gc_runs = 0;
}

void heap_cleanup(Heap* heap) {
    if (heap->memory) {
        free(heap->memory);
        heap->memory = nullptr;
    }
}

u32 size_class_for_size(u32 size) {
    // Add header size
    size += sizeof(HeapObject);
    
    // Find smallest size class that fits
    for (u32 i = 0; i < 16; i++) {
        if (size <= SIZE_CLASS_SIZES[i]) {
            return i;
        }
    }
    return 15;  // Largest size class
}

u32 size_for_size_class(u32 size_class) {
    if (size_class >= 16) return SIZE_CLASS_SIZES[15];
    return SIZE_CLASS_SIZES[size_class];
}

void* heap_alloc(Heap* heap, u32 size, u32 type_id) {
    if (size == 0) return nullptr;
    
    // Align size to 8-byte boundary
    size = (size + 7) & ~7;
    
    u32 size_class = size_class_for_size(size);
    u32 alloc_size = size_for_size_class(size_class);
    
    HeapObject* obj = nullptr;
    
    // Try to allocate from free list first
    FreeBlock* free_block = heap->free_lists[size_class];
    if (free_block) {
        heap->free_lists[size_class] = free_block->next;
        obj = (HeapObject*)free_block;
    }
    else {
        // Allocate from bump allocator
        if (heap->next_alloc + alloc_size > heap->size) {
            // Try garbage collection first
            if (heap->used > heap->gc_threshold) {
                heap_gc_run(heap);
            }
            
            // If still no space, grow heap
            if (heap->next_alloc + alloc_size > heap->size) {
                u32 new_size = heap->size * 2;
                if (new_size < heap->next_alloc + alloc_size) {
                    new_size = heap->next_alloc + alloc_size + 4096;
                }
                heap_grow(heap, new_size);
            }
        }
        
        obj = (HeapObject*)(heap->memory + heap->next_alloc);
        heap->next_alloc += alloc_size;
    }
    
    // Initialize object header
    obj->size = size;
    obj->type_id = type_id;
    obj->marked = false;
    obj->next = heap->all_objects;
    heap->all_objects = obj;
    
    heap->used += alloc_size;
    heap->total_allocations++;
    
    // Return pointer to data area (after header)
    return (u8*)obj + sizeof(HeapObject);
}

void heap_free(Heap* heap, void* ptr) {
    if (!ptr) return;
    
    // Get object header
    HeapObject* obj = (HeapObject*)((u8*)ptr - sizeof(HeapObject));
    
    // Remove from all_objects list
    if (heap->all_objects == obj) {
        heap->all_objects = obj->next;
    } else {
        HeapObject* current = heap->all_objects;
        while (current && current->next != obj) {
            current = current->next;
        }
        if (current) {
            current->next = obj->next;
        }
    }
    
    // Add to appropriate free list
    u32 size_class = size_class_for_size(obj->size);
    FreeBlock* free_block = (FreeBlock*)obj;
    free_block->size = size_for_size_class(size_class);
    free_block->next = heap->free_lists[size_class];
    heap->free_lists[size_class] = free_block;
    
    heap->used -= free_block->size;
    heap->total_deallocations++;
}

void heap_gc_mark(Heap* heap, void* root_ptr) {
    if (!root_ptr) return;
    
    HeapObject* obj = (HeapObject*)((u8*)root_ptr - sizeof(HeapObject));
    if (obj->marked) return;
    
    obj->marked = true;
    
    // TODO: Add type-specific marking for objects that contain references
    // This would need to be extended based on the object type_id
}

void heap_gc_sweep(Heap* heap) {
    HeapObject* current = heap->all_objects;
    HeapObject* prev = nullptr;
    
    while (current) {
        if (current->marked) {
            // Keep object, clear mark for next GC
            current->marked = false;
            prev = current;
            current = current->next;
        } else {
            // Free unmarked object
            HeapObject* to_free = current;
            current = current->next;
            
            if (prev) {
                prev->next = current;
            } else {
                heap->all_objects = current;
            }
            
            // Add to free list
            u32 size_class = size_class_for_size(to_free->size);
            FreeBlock* free_block = (FreeBlock*)to_free;
            free_block->size = size_for_size_class(size_class);
            free_block->next = heap->free_lists[size_class];
            heap->free_lists[size_class] = free_block;
            
            heap->used -= free_block->size;
            heap->total_deallocations++;
        }
    }
}

void heap_gc_run(Heap* heap) {
    // TODO: Mark phase - walk VM stack and mark reachable objects
    // For now, this is a placeholder that would need VM integration
    
    // Sweep phase
    heap_gc_sweep(heap);
    
    heap->gc_runs++;
}

void heap_grow(Heap* heap, u32 new_size) {
    new_size = (new_size + 4095) & ~4095;  // Align to page boundary
    
    u8* new_memory = (u8*)realloc(heap->memory, new_size);
    assert(new_memory != nullptr);
    
    // Update pointers if memory moved
    if (new_memory != heap->memory) {
        i64 offset = new_memory - heap->memory;
        
        // Update all object pointers
        HeapObject* current = heap->all_objects;
        while (current) {
            current = (HeapObject*)((u8*)current + offset);
            current = current->next;
        }
        
        // Update free list pointers
        for (int i = 0; i < 16; i++) {
            if (heap->free_lists[i]) {
                heap->free_lists[i] = (FreeBlock*)((u8*)heap->free_lists[i] + offset);
            }
        }
        
        heap->memory = new_memory;
    }
    
    heap->size = new_size;
    heap->gc_threshold = new_size / 2;
}

u32 heap_get_object_size(void* ptr) {
    if (!ptr) return 0;
    HeapObject* obj = (HeapObject*)((u8*)ptr - sizeof(HeapObject));
    return obj->size;
}

u32 heap_get_object_type(void* ptr) {
    if (!ptr) return 0;
    HeapObject* obj = (HeapObject*)((u8*)ptr - sizeof(HeapObject));
    return obj->type_id;
}

void heap_print_stats(Heap* heap) {
    printf("Heap Statistics:\n");
    printf("  Total size: %u bytes\n", heap->size);
    printf("  Used: %u bytes (%.1f%%)\n", heap->used, (heap->used * 100.0f) / heap->size);
    printf("  Free: %u bytes\n", heap->size - heap->used);
    printf("  Allocations: %u\n", heap->total_allocations);
    printf("  Deallocations: %u\n", heap->total_deallocations);
    printf("  GC runs: %u\n", heap->gc_runs);
}

bool heap_validate(Heap* heap) {
    // Basic heap validation
    if (!heap->memory) return false;
    if (heap->used > heap->size) return false;
    if (heap->next_alloc > heap->size) return false;
    
    // Count objects and verify consistency
    u32 object_count = 0;
    HeapObject* current = heap->all_objects;
    while (current) {
        // Verify object is within heap bounds
        u8* obj_ptr = (u8*)current;
        if (obj_ptr < heap->memory || obj_ptr >= heap->memory + heap->size) {
            return false;
        }
        
        object_count++;
        current = current->next;
    }
    
    return true;
}