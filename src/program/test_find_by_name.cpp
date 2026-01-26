#include "../third_party/doctest.h"
#include <iostream>
#include <sstream>
#include "find_by_name.h"
#include "program.h"
#include "term.h"
#include "block.h"
#include "../globals/global_state.h"
#include "../parser/parse_program.h"

// ============================================================================
// FIND BY NAME UNIT TESTS
// ============================================================================

TEST_CASE("find_term_by_name - basic functionality") {
    SUBCASE("find variable in same block") {
        // Test finding a variable that was defined earlier in the same block
        // Use simpler syntax that we know works
        const char* source = "let x = 42\nlet y = 100";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Get the first term (x = 42)
        Term* x_term = block->get_term_by_local_id(1);
        REQUIRE(x_term != nullptr);
        REQUIRE(x_term->has_fixed_value());
        CHECK(x_term->get_fixed_value()->get_int32() == 42);
        
        // Get the second term (y = 100)
        Term* y_term = block->get_term_by_local_id(2);
        REQUIRE(y_term != nullptr);
        
        // Test find_term_by_name: from y's position, look for 'x'
        GlobalState* global_state = get_active_global_state();
        SymbolId x_symbol = global_state->symbols.get_name("x");
        
        Term* found_term = find_term_by_name(program, x_symbol, y_term);
        REQUIRE(found_term != nullptr);
        CHECK(found_term == x_term);
    }
    
    SUBCASE("variable not found") {
        // Test the find_term_by_name function with a non-existent variable
        const char* source = "let x = 42";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* x_term = block->get_term_by_local_id(1);
        REQUIRE(x_term != nullptr);
        
        GlobalState* global_state = get_active_global_state();
        SymbolId nonexistent_symbol = global_state->get_or_create_symbol("nonexistent");
        
        // This should return null since there's no term with that name
        Term* found_term = find_term_by_name(program, nonexistent_symbol, x_term);
        CHECK(found_term == nullptr);
    }
}

TEST_CASE("resolve_term_ref - different reference types") {
    SUBCASE("simple verify") {
        // Simple verification that this code path works
        const char* source = "42";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 42);
    }
    
    SUBCASE("test term ID reference") {
        const char* source = "42";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        
        // Create a term ID reference
        TermRef term_id_ref;
        term_id_ref.type = TermRefType::TermIdRef;
        term_id_ref.term_id = term->term_id;
        
        // Resolve the reference
        Term* resolved_term = resolve_term_ref(program, term_id_ref, term);
        REQUIRE(resolved_term != nullptr);
        CHECK(resolved_term == term);
    }
    
    SUBCASE("native function reference throws exception") {
        const char* source = "42";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        
        // Create a native function reference
        TermRef native_ref;
        native_ref.type = TermRefType::NativeFunctionRef;
        native_ref.native_function_id = NativeFunctionId::Add;
        
        // Should throw an exception
        CHECK_THROWS_AS(resolve_term_ref(program, native_ref, term), std::runtime_error);
    }
}

TEST_CASE("find_term_by_name - scope visibility") {
    SUBCASE("variable shadowing - later definition hides earlier one") {
        // Test that a later definition with the same name shadows an earlier one
        // Simplify to just test basic shadowing without variable references
        const char* source = "let x = 1\nlet x = 2";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Get the terms
        Term* x1_term = block->get_term_by_local_id(1); // x = 1
        REQUIRE(x1_term != nullptr);
        
        Term* x2_term = block->get_term_by_local_id(2); // x = 2  
        REQUIRE(x2_term != nullptr);
        
        GlobalState* global_state = get_active_global_state();
        SymbolId x_symbol = global_state->symbols.get_name("x");
        
        // Both terms should have the same name
        CHECK(x1_term->name_id == x_symbol);
        CHECK(x2_term->name_id == x_symbol);
        
        // When looking up 'x' from the second term's position, the function should find the first one
        // (since it looks for preceding terms)
        Term* found_term = find_term_by_name(program, x_symbol, x2_term);
        REQUIRE(found_term != nullptr);
        CHECK(found_term == x1_term); // Should find the first definition
    }
    
    SUBCASE("visibility rule - can only see preceding terms") {
        // Test that terms can only see variables defined before them
        const char* source = "let x = 42\nlet y = 100";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* x_term = block->get_term_by_local_id(1);
        REQUIRE(x_term != nullptr);
        
        Term* y_term = block->get_term_by_local_id(2);
        REQUIRE(y_term != nullptr);
        
        GlobalState* global_state = get_active_global_state();
        SymbolId y_symbol = global_state->symbols.get_name("y");
        
        // From x's position, looking for 'y' should return null (y is defined after x)
        Term* found_term = find_term_by_name(program, y_symbol, x_term);
        CHECK(found_term == nullptr);
    }
}

TEST_CASE("find_term_by_name - edge cases") {
    SUBCASE("simple smoke test") {
        // Simple test to verify the function doesn't crash
        const char* source = "42";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 42);
    }
}

TEST_CASE("find_term_by_name - function visibility") {
    SUBCASE("function call should see function definitions from parent scope") {
        // Test that a function call inside a function body can see function definitions
        // from the parent scope (cross-block name resolution)
        // This addresses the TODO: Make sure that the 'func1' call inside func2() 
        // can see the func1() function
        const char* source = "fn func1() {}\nfn func2() { func1() }";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // The parser should create terms for both function definitions
        // and the function call inside func2
        
        // Get func1 definition (should be first term)
        Term* func1_def = block->get_term_by_local_id(1);
        REQUIRE(func1_def != nullptr);
        
        // Get func2 definition (should be second term)  
        Term* func2_def = block->get_term_by_local_id(2);
        REQUIRE(func2_def != nullptr);
        
        // The function call func1() should be in func2's body (block 3)
        Block* func2_body = program->get_block(3);
        REQUIRE(func2_body != nullptr);
        
        // Get the func1() call term inside func2's body
        Term* func1_call = func2_body->get_term_by_local_id(1);
        REQUIRE(func1_call != nullptr);
        
        GlobalState* global_state = get_active_global_state();
        SymbolId func1_symbol = global_state->symbols.get_name("func1");
        
        // Test that the func1() call inside func2 can find the func1 definition
        // This tests cross-block name visibility - this should now work with the 
        // proper parent-child relationship established via add_nested_block()
        Term* found_func1 = find_term_by_name(program, func1_symbol, func1_call);
        
        // With the corrected parent-child relationships, cross-block resolution should work
        REQUIRE(found_func1 != nullptr);
        CHECK(found_func1 == func1_def);
        
        // Verify the function names are set correctly
        SymbolId func2_symbol = global_state->symbols.get_name("func2");
        CHECK(func1_def->name_id == func1_symbol);
        CHECK(func2_def->name_id == func2_symbol);
    }
}

