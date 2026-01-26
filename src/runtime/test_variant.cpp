#include "third_party/doctest.h"
#include "variant/variant.h"

TEST_SUITE("Variant32") {
    TEST_CASE("Default construction") {
        Variant32 v;
        CHECK(v.type == VariantType::None);
    }
    
    TEST_CASE("Integer operations") {
        Variant32 v = Variant32::from_int(42);
        CHECK(v.type == VariantType::I32);
        CHECK(v.get_int32() == 42);
        
        SUBCASE("Negative values") {
            Variant32 neg = Variant32::from_int(-100);
            CHECK(neg.get_int32() == -100);
        }
    }
    
    TEST_CASE("Float operations") {
        Variant32 v = Variant32::from_float(3.14f);
        CHECK(v.type == VariantType::Float32);
        CHECK(v.get_float32() == doctest::Approx(3.14f));
    }
    
    TEST_CASE("Symbol operations") {
        SymbolId sym = 12345;
        Variant32 v = Variant32::from_symbol(sym);
        CHECK(v.type == VariantType::Symbol);
        CHECK(v.get_symbol_id() == sym);
    }
    
    TEST_CASE("String ID operations") {
        u32 string_id = 789;
        Variant32 v = Variant32::from_string_id(string_id);
        CHECK(v.type == VariantType::String);
        CHECK(v.get_string_id() == string_id);
    }
    
    TEST_CASE("Heap pointer operations") {
        void* ptr = reinterpret_cast<void*>(0x1234);
        Variant32 v = Variant32::from_heap_ptr(ptr);
        CHECK(v.type == VariantType::HeapPtr);
        CHECK(v.get_heap_ptr() == ptr);
    }
    
    TEST_CASE("None value") {
        Variant32 v = Variant32::None();
        CHECK(v.type == VariantType::None);
    }
}