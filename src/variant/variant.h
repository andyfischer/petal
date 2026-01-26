#pragma once

#include "standard_headers.h"
#include "runtime/native_funcs.h"
#include <cassert>

enum class VariantSize {
    None,
    Variant32,
};

enum class VariantType {
    None,
    I32,
    Float32,
    Symbol,
    FunctionDef,
    String,
    HeapPtr,
    U32,
};

struct Variant32 {
    VariantType type;

private:
    union {
        i32 int32;
        f32 float32;
        SymbolId symbol_id;
        BlockId block_id;
        u32 string_id;
        void* heap_ptr;     // Pointer to heap-allocated object
        u32 u32_value;
    };

public:
    Variant32() : type(VariantType::None) {}

    Variant32& operator=(const Variant32& other);

    // Getter methods with type assertions
    i32 get_int32() const;
    f32 get_float32() const;
    SymbolId get_symbol_id() const;
    BlockId get_block_id() const;
    u32 get_string_id() const;
    void* get_heap_ptr() const;
    u32 get_u32() const;

    // Setter methods with type validation
    void set_int32(i32 value);
    void set_float32(f32 value);
    void set_symbol_id(SymbolId value);
    void set_block_id(BlockId value);
    void set_string_id(u32 value);
    void set_heap_ptr(void* ptr);
    void set_u32(u32 value);
    void set_none();

    static Variant32 None();
    static Variant32 from_int(int32_t value);
    static Variant32 from_float(float value);
    static Variant32 from_symbol(SymbolId value);
    static Variant32 from_string_id(u32 value);
    static Variant32 from_heap_ptr(void* ptr);
    static Variant32 from_u32(u32 value);
    static Variant32 function_def(BlockId value);
    bool has_value() const;

    // Type checker methods
    bool is_none() const;
    bool is_i32() const;
    bool is_float32() const;
    bool is_symbol() const;
    bool is_function_def() const;
    bool is_string() const;
    bool is_heap_ptr() const;
};