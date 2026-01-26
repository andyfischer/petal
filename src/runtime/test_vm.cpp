#include "third_party/doctest.h"
#include "runtime/vm.h"
#include "variant/variant.h"
#include "runtime/heap_types.h"
#include "bytecode/bytecode_encoding.h"
#include <vector>

TEST_SUITE("VM Operations") {
    TEST_CASE("VM creation and destruction") {
        VM* vm = vm_create();
        REQUIRE(vm != nullptr);
        CHECK(vm_get_stack_size(vm) == 0);
        vm_destroy(vm);
    }
    
    TEST_CASE("String constant management") {
        VM* vm = vm_create();
        
        SUBCASE("Add single string constant") {
            u32 id = vm_add_string_constant(vm, "Hello, World!");
            CHECK(id == 0);
        }
        
        SUBCASE("Add multiple string constants") {
            u32 id1 = vm_add_string_constant(vm, "First");
            u32 id2 = vm_add_string_constant(vm, "Second");
            u32 id3 = vm_add_string_constant(vm, "Third");
            
            CHECK(id1 == 0);
            CHECK(id2 == 1);
            CHECK(id3 == 2);
        }
        
        SUBCASE("Add empty string") {
            u32 id = vm_add_string_constant(vm, "");
            CHECK(id == 0);
        }
        
        vm_destroy(vm);
    }
    
    TEST_CASE("Heap string operations") {
        VM* vm = vm_create();
        
        HeapString* str = vm_create_heap_string(vm, "Test String");
        REQUIRE(str != nullptr);
        
        CHECK(vm_get_heap_string_length(str) == 11);
        CHECK(strcmp(vm_get_heap_string_data(str), "Test String") == 0);
        
        vm_destroy(vm);
    }
    
    TEST_CASE("VM function calls") {
        SUBCASE("execute OP_CALL function call") {
            VM* vm = vm_create();
            
            // Create a simple test function that adds two numbers
            // Function bytecode: param0 + param1, then return result
            std::vector<Instruction> add_function;
            add_function.push_back(pack_op_i32_add(0, 1));     // slot 0 = slot 0 + slot 1
            add_function.push_back(pack_op_return(0));         // return slot 0
            
            // Register function in VM's function table
            BlockId func_id = 42;  // Use a test block ID
            vm_add_function_bytecode(vm, func_id, add_function.data(), add_function.size());
            
            // Main program bytecode that calls the function
            std::vector<Instruction> instructions;
            instructions.push_back(pack_op_reserve_stack(10));              // Reserve stack space
            instructions.push_back(pack_op_const_i16(3, 5));               // slot 3 = 5 (first arg)
            instructions.push_back(pack_op_const_i16(4, 3));               // slot 4 = 3 (second arg)
            
            // Create function definition and store in slot 2
            Variant32 func_def = Variant32::function_def(func_id);
            vm_resize_stack(vm, 10);  // Make sure we have slots
            vm_set_stack_slot(vm, 2, func_def);                           // slot 2 = function
            
            // Call sequence: prepare, push args, call
            instructions.push_back(pack_op_prepare_call(2, 3, 2));         // prepare call with 3 locals, 2 args
            instructions.push_back(pack_op_push(3, 0));                    // copy slot 3 to param 0
            instructions.push_back(pack_op_push(4, 1));                    // copy slot 4 to param 1
            instructions.push_back(pack_op_call(0));                       // execute call
            instructions.push_back(pack_op_stop());                        // stop execution
            
            // Execute the bytecode
            vm_execute(vm, instructions.data(), instructions.size());
            
            // Verify the function was called and returned the correct result
            CHECK(vm_get_stack_size(vm) >= 1);
            // The result (5 + 3 = 8) should be at the top of the stack
            u32 stack_size = vm_get_stack_size(vm);
            printf("Stack size after call: %u\n", stack_size);
            
            if (stack_size > 0) {
                Variant32 result = vm_get_slot(vm, stack_size - 1);
                printf("Result type: %d\n", (int)result.type);
                CHECK(result.type == VariantType::I32);
                if (result.type == VariantType::I32) {
                    CHECK(result.get_int32() == 8);
                }
            }
            
            vm_destroy(vm);
        }
    }
    
    TEST_CASE("Control flow operations") {
        SUBCASE("Jump operations") {
            VM* vm = vm_create();
            vm_resize_slot_array(vm, 10);
            
            // Set up frame header at slot 0 (required for VM execution)
            u32 frame_header = vm_pack_frame_header(0, 0);
            vm_set_slot_slot(vm, 0, Variant32::from_u32(frame_header));
            
            // Test unconditional jump
            std::vector<Instruction> instructions;
            instructions.push_back(pack_op_const_i16(1, 42));   // slot 1 = 42
            instructions.push_back(pack_op_jump(4));            // jump to instruction 4
            instructions.push_back(pack_op_const_i16(1, 99));   // should be skipped
            instructions.push_back(pack_op_const_i16(1, 88));   // should be skipped
            instructions.push_back(pack_op_const_i16(2, 100));  // slot 2 = 100 (jump target)
            instructions.push_back(pack_op_stop());             // stop execution
            
            vm_execute(vm, instructions.data(), instructions.size());
            
            CHECK(vm_get_slot_i32(vm, 1) == 42);  // Should be set before jump
            CHECK(vm_get_slot_i32(vm, 2) == 100); // Should be set after jump
            
            vm_destroy(vm);
        }
        
        SUBCASE("Conditional jump operations") {
            VM* vm = vm_create();
            vm_resize_slot_array(vm, 10);
            
            // Set up frame header at slot 0
            u32 frame_header = vm_pack_frame_header(0, 0);
            vm_set_slot_slot(vm, 0, Variant32::from_u32(frame_header));
            
            // Test jump_if_true with true condition
            std::vector<Instruction> instructions;
            instructions.push_back(pack_op_const_i16(1, 1));     // slot 1 = 1 (true)
            instructions.push_back(pack_op_jump_if_true(1, 4));  // jump if slot 1 is true
            instructions.push_back(pack_op_const_i16(2, 99));    // should be skipped
            instructions.push_back(pack_op_const_i16(2, 88));    // should be skipped
            instructions.push_back(pack_op_const_i16(2, 200));   // slot 2 = 200 (jump target)
            instructions.push_back(pack_op_stop());
            
            vm_execute(vm, instructions.data(), instructions.size());
            
            CHECK(vm_get_slot_i32(vm, 1) == 1);   // Condition
            CHECK(vm_get_slot_i32(vm, 2) == 200); // Should be set after jump
            
            vm_destroy(vm);
        }
        
        SUBCASE("Comparison operations") {
            VM* vm = vm_create();
            vm_resize_slot_array(vm, 10);
            
            // Set up frame header at slot 0
            u32 frame_header = vm_pack_frame_header(0, 0);
            vm_set_slot_slot(vm, 0, Variant32::from_u32(frame_header));
            
            std::vector<Instruction> instructions;
            instructions.push_back(pack_op_const_i16(1, 5));      // slot 1 = 5
            instructions.push_back(pack_op_const_i16(2, 3));      // slot 2 = 3
            instructions.push_back(pack_op_i32_lt(1, 2, 3));      // slot 3 = (5 < 3) = 0
            instructions.push_back(pack_op_i32_gt(1, 2, 4));      // slot 4 = (5 > 3) = 1
            instructions.push_back(pack_op_i32_eq(1, 1, 5));      // slot 5 = (5 == 5) = 1
            instructions.push_back(pack_op_stop());
            
            vm_execute(vm, instructions.data(), instructions.size());
            
            CHECK(vm_get_slot_i32(vm, 1) == 5);   // Original value
            CHECK(vm_get_slot_i32(vm, 2) == 3);   // Original value
            CHECK(vm_get_slot_i32(vm, 3) == 0);   // 5 < 3 = false
            CHECK(vm_get_slot_i32(vm, 4) == 1);   // 5 > 3 = true
            CHECK(vm_get_slot_i32(vm, 5) == 1);   // 5 == 5 = true
            
            vm_destroy(vm);
        }
    }
}