#include "../third_party/doctest.h"
#include <iostream>
#include <sstream>
#include "parse_program.h"
#include "../program/program.h"
#include "../program/term.h"
#include "../program/block.h"
#include "../globals/global_state.h"
#include "../program/find_by_name.h"

// ============================================================================
// PARSER UNIT TESTS
// ============================================================================

TEST_CASE("parser - simple literal values") {
    SUBCASE("integer literal") {
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
    
    SUBCASE("float literal") {
        const char* source = "3.14";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_float32() == 3.14f);
    }
    
    SUBCASE("boolean literals") {
        const char* source_true = "true";
        Program* program_true = parse_program(source_true);
        
        REQUIRE(program_true != nullptr);
        Block* block_true = program_true->get_block(1);
        REQUIRE(block_true != nullptr);
        
        Term* term_true = block_true->get_term_by_local_id(1);
        REQUIRE(term_true != nullptr);
        REQUIRE(term_true->has_fixed_value());
        CHECK(term_true->get_fixed_value()->get_int32() == 1);
        
        const char* source_false = "false";
        Program* program_false = parse_program(source_false);
        
        REQUIRE(program_false != nullptr);
        Block* block_false = program_false->get_block(1);
        REQUIRE(block_false != nullptr);
        
        Term* term_false = block_false->get_term_by_local_id(1);
        REQUIRE(term_false != nullptr);
        REQUIRE(term_false->has_fixed_value());
        CHECK(term_false->get_fixed_value()->get_int32() == 0);
    }
}

TEST_CASE("parser - variable declarations") {
    SUBCASE("simple let statement") {
        const char* source = "let x = 42";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 42);
        CHECK(term->name_id == get_active_global_state()->symbols.get_name("x"));
    }
    
    SUBCASE("let with different types") {
        const char* source = "let x = 42\nlet y = 3.14\nlet z = true";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Check integer variable
        Term* term_x = block->get_term_by_local_id(1);
        REQUIRE(term_x != nullptr);
        REQUIRE(term_x->has_fixed_value());
        CHECK(term_x->get_fixed_value()->get_int32() == 42);
        CHECK(term_x->name_id == get_active_global_state()->symbols.get_name("x"));
        
        // Check float variable
        Term* term_y = block->get_term_by_local_id(2);
        REQUIRE(term_y != nullptr);
        REQUIRE(term_y->has_fixed_value());
        CHECK(term_y->get_fixed_value()->get_float32() == 3.14f);
        CHECK(term_y->name_id == get_active_global_state()->symbols.get_name("y"));
        
        // Check boolean variable
        Term* term_z = block->get_term_by_local_id(3);
        REQUIRE(term_z != nullptr);
        REQUIRE(term_z->has_fixed_value());
        CHECK(term_z->get_fixed_value()->get_int32() == 1);
        CHECK(term_z->name_id == get_active_global_state()->symbols.get_name("z"));
    }
}

TEST_CASE("parser - arithmetic expressions") {
    SUBCASE("simple addition") {
        const char* source = "let result = 5 + 3";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Should have multiple terms: 5, 3, +, and the result
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        CHECK(term->name_id == get_active_global_state()->symbols.get_name("result"));
    }
    
    SUBCASE("multiplication") {
        const char* source = "let result = 4 * 7";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        CHECK(term->name_id == get_active_global_state()->symbols.get_name("result"));
    }
}

TEST_CASE("parser - string literals") {
    SUBCASE("simple string") {
        const char* source = "let greeting = \"Hello, World!\"";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);       
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        CHECK(term->name_id == get_active_global_state()->symbols.get_name("greeting"));
    }
    
    SUBCASE("empty string") {
        const char* source = "let empty = \"\"";
        Program* program = parse_program(source);
        
        REQUIRE(program != nullptr);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        CHECK(term->name_id == get_active_global_state()->symbols.get_name("empty"));
    }
}

TEST_CASE("name lookup") {
    const char* source = "let x = 1;\n"
                              "let x = 2;\n"
                              "let x = 3;\n";

    Program* program = parse_program(source);
    Block* block = program->get_block(1);

    REQUIRE(block != nullptr);

    Term* term_2 = block->get_term_by_local_id(2);
    REQUIRE(term_2 != nullptr);
    CHECK(term_2->get_fixed_value()->get_int32() == 2);

    Term* visible_term = find_term_by_name(program, get_active_global_state()->symbols.get_name("x"), term_2);
    REQUIRE(visible_term != nullptr);
    CHECK(visible_term->get_fixed_value()->get_int32() == 1);
}

// ============================================================================
// COMPREHENSIVE PARSER TESTS
// ============================================================================

TEST_CASE("parser - literal values") {
    SUBCASE("integer literal parsing") {
        Program* program = parse_program("42");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 42);
    }
    
    SUBCASE("float literal parsing") {
        Program* program = parse_program("3.14159");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_float32() == 3.14159f);
    }
    
    SUBCASE("scientific notation parsing") {
        Program* program = parse_program("1.23e-4");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_float32() == 0.000123f);
    }
    
    SUBCASE("hexadecimal literal parsing") {
        Program* program = parse_program("0xFF");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 255);
    }
    
    SUBCASE("binary literal parsing") {
        Program* program = parse_program("0b1010");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 10);
    }
    
    SUBCASE("boolean literal parsing") {
        Program* program_true = parse_program("true");
        Block* block_true = program_true->get_block(1);
        REQUIRE(block_true != nullptr);
        
        Term* term_true = block_true->get_term_by_local_id(1);
        REQUIRE(term_true != nullptr);
        REQUIRE(term_true->has_fixed_value());
        CHECK(term_true->get_fixed_value()->get_int32() == 1);
        
        Program* program_false = parse_program("false");
        Block* block_false = program_false->get_block(1);
        REQUIRE(block_false != nullptr);
        
        Term* term_false = block_false->get_term_by_local_id(1);
        REQUIRE(term_false != nullptr);
        REQUIRE(term_false->has_fixed_value());
        CHECK(term_false->get_fixed_value()->get_int32() == 0);
    }
}

TEST_CASE("parser - unary operators") {
    SUBCASE("unary minus integer") {
        Program* program = parse_program("-42");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Check that we have at least one term with the correct negative value
        // The unary minus implementation creates multiple terms, and we want the final result
        bool found_negative = false;
        for (int i = 1; i <= 3; i++) {
            Term* term = block->get_term_by_local_id(i);
            if (term && term->has_fixed_value()) {
                int value = term->get_fixed_value()->get_int32();
                if (value == -42) {
                    found_negative = true;
                    break;
                }
            }
        }
        CHECK(found_negative);
    }
    
    SUBCASE("unary minus float") {
        Program* program = parse_program("-3.14");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Check that we have at least one term with the correct negative value
        bool found_negative = false;
        for (int i = 1; i <= 3; i++) {
            Term* term = block->get_term_by_local_id(i);
            if (term && term->has_fixed_value() && term->get_fixed_value()->type == VariantType::Float32) {
                float value = term->get_fixed_value()->get_float32();
                if (value == -3.14f) {
                    found_negative = true;
                    break;
                }
            }
        }
        CHECK(found_negative);
    }
}

TEST_CASE("parser - let statements") {
    SUBCASE("simple let statement") {
        Program* program = parse_program("let x = 42");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 42);
        CHECK(term->name_id == get_active_global_state()->symbols.get_name("x"));
    }
    
    SUBCASE("multiple let statements") {
        Program* program = parse_program("let x = 42\nlet y = 3.14\nlet name = \"hello\"");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Check first variable
        Term* term1 = block->get_term_by_local_id(1);
        REQUIRE(term1 != nullptr);
        REQUIRE(term1->has_fixed_value());
        CHECK(term1->get_fixed_value()->get_int32() == 42);
        CHECK(term1->name_id == get_active_global_state()->symbols.get_name("x"));
        
        // Check second variable
        Term* term2 = block->get_term_by_local_id(2);
        REQUIRE(term2 != nullptr);
        REQUIRE(term2->has_fixed_value());
        CHECK(term2->get_fixed_value()->get_float32() == 3.14f);
        CHECK(term2->name_id == get_active_global_state()->symbols.get_name("y"));
        
        // Check third variable (string stored as symbol)
        Term* term3 = block->get_term_by_local_id(3);
        REQUIRE(term3 != nullptr);
        REQUIRE(term3->has_fixed_value());
        CHECK(term3->name_id == get_active_global_state()->symbols.get_name("name"));
    }
}

TEST_CASE("parser - assignment statements") {
    SUBCASE("simple assignment") {
        Program* program = parse_program("x = 100");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 100);
    }
    
    SUBCASE("assignment with different types") {
        Program* program = parse_program("x = 42\ny = \"hello\"\nz = true");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Integer assignment
        Term* term1 = block->get_term_by_local_id(1);
        REQUIRE(term1 != nullptr);
        REQUIRE(term1->has_fixed_value());
        CHECK(term1->get_fixed_value()->get_int32() == 42);
        
        // String assignment (stored as symbol)
        Term* term2 = block->get_term_by_local_id(2);
        REQUIRE(term2 != nullptr);
        REQUIRE(term2->has_fixed_value());
        
        // Boolean assignment
        Term* term3 = block->get_term_by_local_id(3);
        REQUIRE(term3 != nullptr);
        REQUIRE(term3->has_fixed_value());
        CHECK(term3->get_fixed_value()->get_int32() == 1);
    }
    
    SUBCASE("assignment without spaces") {
        Program* program = parse_program("x=42");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        REQUIRE(term->has_fixed_value());
        CHECK(term->get_fixed_value()->get_int32() == 42);
    }
}

TEST_CASE("parser - milestone coverage") {
    SUBCASE("milestone 01 - literals") {
        const char* source = 
            "42\n"
            "3.14159\n"
            "1.23e-4\n"
            "0xFF\n"
            "0b1010\n"
            "\"hello\"\n"
            "true\n"
            "false\n"
            "null\n"
            ":color";
            
        Program* program = parse_program(source);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Verify we have multiple terms
        CHECK(block->get_term_by_local_id(1) != nullptr); // 42
        CHECK(block->get_term_by_local_id(2) != nullptr); // 3.14159
        CHECK(block->get_term_by_local_id(3) != nullptr); // 1.23e-4
        CHECK(block->get_term_by_local_id(4) != nullptr); // 0xFF
        CHECK(block->get_term_by_local_id(5) != nullptr); // 0b1010
        CHECK(block->get_term_by_local_id(6) != nullptr); // "hello"
        CHECK(block->get_term_by_local_id(7) != nullptr); // true
        CHECK(block->get_term_by_local_id(8) != nullptr); // false
        CHECK(block->get_term_by_local_id(9) != nullptr); // null
        CHECK(block->get_term_by_local_id(10) != nullptr); // :color
        
        // Verify specific values
        CHECK(block->get_term_by_local_id(1)->get_fixed_value()->get_int32() == 42);
        CHECK(block->get_term_by_local_id(2)->get_fixed_value()->get_float32() == 3.14159f);
        CHECK(block->get_term_by_local_id(4)->get_fixed_value()->get_int32() == 255); // 0xFF
        CHECK(block->get_term_by_local_id(5)->get_fixed_value()->get_int32() == 10);  // 0b1010
        CHECK(block->get_term_by_local_id(7)->get_fixed_value()->get_int32() == 1);   // true
        CHECK(block->get_term_by_local_id(8)->get_fixed_value()->get_int32() == 0);   // false
    }
    
    SUBCASE("milestone 02 - variables subset") {
        const char* source = 
            "let x = 42\n"
            "let y = 3.14\n"
            "let name = \"Alice\"\n"
            "let active = true\n"
            "let empty = null\n"
            "x = 100\n"
            "name = \"Bob\"";
            
        Program* program = parse_program(source);
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        // Verify let statements created terms
        Term* term1 = block->get_term_by_local_id(1);
        REQUIRE(term1 != nullptr);
        CHECK(term1->get_fixed_value()->get_int32() == 42);
        CHECK(term1->name_id == get_active_global_state()->symbols.get_name("x"));
        
        Term* term2 = block->get_term_by_local_id(2);
        REQUIRE(term2 != nullptr);
        CHECK(term2->get_fixed_value()->get_float32() == 3.14f);
        CHECK(term2->name_id == get_active_global_state()->symbols.get_name("y"));
        
        // Verify assignments created additional terms
        Term* assignment1 = block->get_term_by_local_id(6); // x = 100
        REQUIRE(assignment1 != nullptr);
        CHECK(assignment1->get_fixed_value()->get_int32() == 100);
        
        Term* assignment2 = block->get_term_by_local_id(7); // name = "Bob"
        REQUIRE(assignment2 != nullptr);
        // String stored as symbol, so just verify it exists
    }
}

// ============================================================================
// EDGE CASES AND ERROR HANDLING
// ============================================================================

TEST_CASE("parser - edge cases") {
    SUBCASE("empty program") {
        Program* program = parse_program("");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        // Should not crash and should have no terms
    }
    
    SUBCASE("whitespace only") {
        Program* program = parse_program("   \n\t  \n  ");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        // Should not crash
    }
    
    SUBCASE("comments only") {
        Program* program = parse_program("// This is a comment\n// Another comment");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        // Should not crash
    }
    
    SUBCASE("mixed comments and code") {
        Program* program = parse_program("// Comment\nlet x = 42 // End comment\n// Final comment");
        Block* block = program->get_block(1);
        REQUIRE(block != nullptr);
        
        Term* term = block->get_term_by_local_id(1);
        REQUIRE(term != nullptr);
        CHECK(term->get_fixed_value()->get_int32() == 42);
    }
}


