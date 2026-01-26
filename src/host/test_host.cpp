#include "../third_party/doctest.h"
#include <iostream>
#include <vector>
#include "host_api.h"
#include "../runtime/vm.h"
#include "../globals/global_state.h"
#include "../bytecode/bytecode_encoding.h"

// ============================================================================
// HOST FUNCTION TESTS
// ============================================================================

// Test host functions - simple implementations for testing
static bool g_test_function_called = false;
static int g_test_function_arg = 0;
static int g_test_function_result = 0;

Variant32 test_host_add(VM* vm) {
    g_test_function_called = true;
    i32 a = petal_get_i32(vm);
    i32 b = petal_get_i32(vm);
    g_test_function_arg = a;  // Store first arg for verification
    g_test_function_result = a + b;
    return Variant32::from_int(a + b);
}

Variant32 test_host_multiply(VM* vm) {
    g_test_function_called = true;
    i32 a = petal_get_i32(vm);
    i32 b = petal_get_i32(vm);
    g_test_function_result = a * b;
    return Variant32::from_int(a * b);
}

Variant32 test_host_return_constant(VM* vm) {
    g_test_function_called = true;
    return Variant32::from_int(42);
}

TEST_CASE("host functions - registration") {
    SUBCASE("register host function") {
        // Reset test state
        g_test_function_called = false;
        
        // Register a test function
        Program* program = petal_parse_defs("func test_add(i32, i32) -> i32;");
        petal_register_host_function(program, "test_add", test_host_add);
        
        // Verify the function is registered in global state
        GlobalState* gs = get_active_global_state();
        SymbolId symbol = gs->get_or_create_symbol("test_add");
        HostFunctionEntry* entry = gs->lookup_host_function(symbol);
        
        REQUIRE(entry != nullptr);
        CHECK(entry->func == test_host_add);
        CHECK(entry->expected_argc == 2);
    }
    
    SUBCASE("register multiple host functions") {
        Program* program1 = petal_parse_defs("func test_multiply(i32, i32) -> i32;");
        Program* program2 = petal_parse_defs("func test_constant() -> i32;");
        petal_register_host_function(program1, "test_multiply", test_host_multiply);
        petal_register_host_function(program2, "test_constant", test_host_return_constant);
        
        GlobalState* gs = get_active_global_state();
        
        // Check first function
        SymbolId symbol1 = gs->get_or_create_symbol("test_multiply");
        HostFunctionEntry* entry1 = gs->lookup_host_function(symbol1);
        REQUIRE(entry1 != nullptr);
        CHECK(entry1->func == test_host_multiply);
        CHECK(entry1->expected_argc == 2);
        
        // Check second function
        SymbolId symbol2 = gs->get_or_create_symbol("test_constant");
        HostFunctionEntry* entry2 = gs->lookup_host_function(symbol2);
        REQUIRE(entry2 != nullptr);
        CHECK(entry2->func == test_host_return_constant);
        CHECK(entry2->expected_argc == 0);
    }
    
    SUBCASE("lookup non-existent function") {
        GlobalState* gs = get_active_global_state();
        SymbolId symbol = gs->get_or_create_symbol("non_existent_function");
        HostFunctionEntry* entry = gs->lookup_host_function(symbol);
        CHECK(entry == nullptr);
    }
}

TEST_CASE("host functions - VM execution") {
    SUBCASE("execute simple host function") {
        // Reset test state
        g_test_function_called = false;
        g_test_function_arg = 0;
        g_test_function_result = 0;
        
        // Register test function
        Program* program = petal_parse_defs("func vm_test_add(i32, i32) -> i32;");
        petal_register_host_function(program, "vm_test_add", test_host_add);
        
        // Create VM and test the OP_CALL_HOST instruction manually
        VM* vm = vm_create();
        
        // Create a minimal bytecode program:
        // 1. Reserve stack space
        // 2. Load constant 5 into slot 0
        // 3. Load constant 3 into slot 1  
        // 4. Load symbol "vm_test_add" into slot 2
        // 5. Call host function with 2 arguments
        std::vector<Instruction> instructions;
        instructions.push_back(pack_op_reserve_slots(10));      // Reserve stack space
        instructions.push_back(pack_op_const_i16(0, 5));        // slot 0 = 5
        instructions.push_back(pack_op_const_i16(1, 3));        // slot 1 = 3
        
        // Get symbol for the function name
        GlobalState* gs = get_active_global_state();
        SymbolId func_symbol = gs->get_or_create_symbol("vm_test_add");
        instructions.push_back(pack_op_const_u16_sym(2, func_symbol));  // slot 2 = symbol
        
        instructions.push_back(pack_op_call_host(2, 2));        // call host function
        instructions.push_back(pack_op_stop());                 // stop execution
        
        // Execute the bytecode
        vm_execute(vm, instructions.data(), instructions.size());
        
        // Verify the host function was called
        CHECK(g_test_function_called);
        CHECK(g_test_function_arg == 5);     // First argument was 5
        CHECK(g_test_function_result == 8);  // 5 + 3 = 8
        
        // Verify the result is on the VM stack
        CHECK(vm_get_slot_array_size(vm) >= 1);
        
        vm_destroy(vm);
    }
    
    SUBCASE("execute host function with no arguments") {
        // Reset test state
        g_test_function_called = false;
        g_test_function_result = 0;
        
        // Register test function that takes no arguments
        Program* program = petal_parse_defs("func vm_test_constant() -> i32;");
        petal_register_host_function(program, "vm_test_constant", test_host_return_constant);
        
        VM* vm = vm_create();
        
        // Create bytecode:
        // 1. Reserve stack space
        // 2. Load symbol "vm_test_constant" into slot 0
        // 3. Call host function with 0 arguments
        std::vector<Instruction> instructions;
        instructions.push_back(pack_op_reserve_slots(5));       // Reserve stack space
        
        GlobalState* gs = get_active_global_state();
        SymbolId func_symbol = gs->get_or_create_symbol("vm_test_constant");
        instructions.push_back(pack_op_const_u16_sym(0, func_symbol));  // slot 0 = symbol
        instructions.push_back(pack_op_call_host(0, 0));               // call with 0 args
        instructions.push_back(pack_op_stop());
        
        vm_execute(vm, instructions.data(), instructions.size());
        
        // Verify the host function was called
        CHECK(g_test_function_called);
        
        vm_destroy(vm);
    }
    
    SUBCASE("call non-existent host function") {
        VM* vm = vm_create();
        
        // Create bytecode that calls a non-existent function
        std::vector<Instruction> instructions;
        instructions.push_back(pack_op_reserve_slots(5));       // Reserve stack space
        
        GlobalState* gs = get_active_global_state();
        SymbolId func_symbol = gs->get_or_create_symbol("non_existent_function");
        instructions.push_back(pack_op_const_u16_sym(0, func_symbol));
        instructions.push_back(pack_op_call_host(0, 0));
        instructions.push_back(pack_op_stop());
        
        // Should not crash - should just push None onto stack
        vm_execute(vm, instructions.data(), instructions.size());
        
        // Verify stack has result (None value)
        CHECK(vm_get_slot_array_size(vm) >= 1);
        
        vm_destroy(vm);
    }
}

TEST_CASE("host functions - argument validation") {
    SUBCASE("wrong argument count") {
        // Reset test state
        g_test_function_called = false;
        
        // Register function expecting 2 arguments
        Program* program = petal_parse_defs("func vm_test_arg_validation(i32, i32) -> i32;");
        petal_register_host_function(program, "vm_test_arg_validation", test_host_add);
        
        VM* vm = vm_create();
        
        // Create bytecode that calls with wrong number of arguments (1 instead of 2)
        std::vector<Instruction> instructions;
        instructions.push_back(pack_op_reserve_slots(5));       // Reserve stack space
        instructions.push_back(pack_op_const_i16(0, 5));        // slot 0 = 5 (only 1 arg)
        
        GlobalState* gs = get_active_global_state();
        SymbolId func_symbol = gs->get_or_create_symbol("vm_test_arg_validation");
        instructions.push_back(pack_op_const_u16_sym(1, func_symbol));
        instructions.push_back(pack_op_call_host(1, 1));        // call with 1 arg (should be 2)
        instructions.push_back(pack_op_stop());
        
        // Should not crash, but function should not be called due to argument mismatch
        vm_execute(vm, instructions.data(), instructions.size());
        
        // Function should not have been called due to argument count mismatch
        CHECK(!g_test_function_called);
        
        vm_destroy(vm);
    }
}