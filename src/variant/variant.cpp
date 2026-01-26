#include "variant.h"
#include <sstream>
#include <stdexcept>

Variant32 Variant32::None() {
    Variant32 v;
    v.type = VariantType::None;
    return v;
}

Variant32 Variant32::from_int(int32_t value) {
    Variant32 v;
    v.type = VariantType::I32;
    v.int32 = value;
    return v;
}

Variant32 Variant32::from_float(float value) {
    Variant32 v;
    v.type = VariantType::Float32;
    v.float32 = value;
    return v;
}

Variant32 Variant32::from_symbol(SymbolId value) {
    Variant32 v;
    v.type = VariantType::Symbol;
    v.symbol_id = value;
    return v;
}

Variant32 Variant32::from_string_id(u32 value) {
    Variant32 v;
    v.type = VariantType::String;
    v.string_id = value;
    return v;
}

Variant32 Variant32::function_def(BlockId value) {
    Variant32 v;
    v.type = VariantType::FunctionDef;
    v.block_id = value;
    return v;
}

Variant32 Variant32::from_heap_ptr(void* ptr) {
    Variant32 v;
    v.type = VariantType::HeapPtr;
    v.heap_ptr = ptr;
    return v;
}

Variant32 Variant32::from_u32(u32 value) {
    Variant32 v;
    v.type = VariantType::U32;
    v.u32_value = value;
    return v;
}

Variant32& Variant32::operator=(const Variant32& other) {
    type = other.type;
    switch (type) {
        case VariantType::I32:
            int32 = other.int32;
            break;
        case VariantType::Float32:
            float32 = other.float32;
            break;
        case VariantType::Symbol:
            symbol_id = other.symbol_id;
            break;
        case VariantType::String:
            string_id = other.string_id;
            break;
        case VariantType::FunctionDef:
            block_id = other.block_id;
            break;
        case VariantType::HeapPtr:
            heap_ptr = other.heap_ptr;
            break;
        case VariantType::U32:
            u32_value = other.u32_value;
            break;
        case VariantType::None:
            break;
    }
    return *this;
}

bool Variant32::has_value() const {
    return type != VariantType::None;
}

bool Variant32::is_none() const {
    return type == VariantType::None;
}

bool Variant32::is_i32() const {
    return type == VariantType::I32;
}

bool Variant32::is_float32() const {
    return type == VariantType::Float32;
}

bool Variant32::is_symbol() const {
    return type == VariantType::Symbol;
}

bool Variant32::is_function_def() const {
    return type == VariantType::FunctionDef;
}

bool Variant32::is_string() const {
    return type == VariantType::String;
}

bool Variant32::is_heap_ptr() const {
    return type == VariantType::HeapPtr;
}

// Getter method implementations
i32 Variant32::get_int32() const {
    assert(type == VariantType::I32);
    return int32;
}

f32 Variant32::get_float32() const {
    assert(type == VariantType::Float32);
    return float32;
}

SymbolId Variant32::get_symbol_id() const {
    assert(type == VariantType::Symbol);
    return symbol_id;
}

BlockId Variant32::get_block_id() const {
    assert(type == VariantType::FunctionDef);
    return block_id;
}

u32 Variant32::get_string_id() const {
    assert(type == VariantType::String);
    return string_id;
}

void* Variant32::get_heap_ptr() const {
    assert(type == VariantType::HeapPtr);
    return heap_ptr;
}

u32 Variant32::get_u32() const {
    assert(type == VariantType::U32);
    return u32_value;
}

// Setter method implementations
void Variant32::set_int32(i32 value) {
    type = VariantType::I32;
    int32 = value;
}

void Variant32::set_float32(f32 value) {
    type = VariantType::Float32;
    float32 = value;
}

void Variant32::set_symbol_id(SymbolId value) {
    type = VariantType::Symbol;
    symbol_id = value;
}

void Variant32::set_block_id(BlockId value) {
    type = VariantType::FunctionDef;
    block_id = value;
}

void Variant32::set_string_id(u32 value) {
    type = VariantType::String;
    string_id = value;
}

void Variant32::set_heap_ptr(void* ptr) {
    type = VariantType::HeapPtr;
    heap_ptr = ptr;
}

void Variant32::set_u32(u32 value) {
    type = VariantType::U32;
    u32_value = value;
}

void Variant32::set_none() {
    type = VariantType::None;
}

/*
std::string Variant::format_to_string() const {
    std::ostringstream oss;
    
    switch (type) {
        case VariantType::String:
            return *string_value;
            
        case VariantType::Int:
            oss << int_value;
            return oss.str();
            
        case VariantType::Float:
            oss << float_value;
            return oss.str();
            
        case VariantType::Symbol:
            oss << symbol_id;
            return oss.str();
            
        case VariantType::NativeFunction:
            oss << "NativeFunction(" << native_function_id << ")";
            return oss.str();
            
        case VariantType::FunctionDef:
            oss << "FunctionDef";
            return oss.str();
            
        case VariantType::Vec4f:
            oss << "(" << vec4f_value.a << ", " 
                << vec4f_value.b << ", " 
                << vec4f_value.c << ", " 
                << vec4f_value.d << ")";
            return oss.str();
            
        case VariantType::Vec4i:
            oss << "(" << vec4i_value.a << ", " 
                << vec4i_value.b << ", " 
                << vec4i_value.c << ", " 
                << vec4i_value.d << ")";
            return oss.str();
            
        default:
            throw std::runtime_error("Cannot convert to string");
    }
}
*/