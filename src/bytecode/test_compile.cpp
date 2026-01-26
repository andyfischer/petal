
#include "../third_party/doctest.h"
#include <iostream>
#include <sstream>
#include "../parser/parse_program.h"
#include "../program/program.h"
#include "../program/term.h"
#include "../program/block.h"
#include "../globals/global_state.h"
#include "compile.h"
#include "debug_format_program.h"
#include "debug_format_bytecode.h"

// ============================================================================
// BYTECODE COMPILATION TESTS
// ============================================================================

TEST_CASE("bytecode - simple literals") {
    SUBCASE("integer literal compilation") {
        const char* source = "42";
        Program* program = parse_program(source);
        REQUIRE(program != nullptr);
        
        Bytecode* bytecode = compile_program(program);
        REQUIRE(bytecode != nullptr);
        
        // Verify bytecode was generated
        CHECK(bytecode->isns.size() > 0);
        
        // Debug output to verify compilation
        std::ostringstream debug_out;
        debug_format_bytecode(debug_out, bytecode);
        std::string debug_str = debug_out.str();
        CHECK(debug_str.length() > 0);
    }
}

TEST_CASE("bytecode - variable declarations") {
    SUBCASE("simple let statement compilation") {
        const char* source = "let x = 42";
        Program* program = parse_program(source);
        REQUIRE(program != nullptr);
        
        Bytecode* bytecode = compile_program(program);
        REQUIRE(bytecode != nullptr);
        
        CHECK(bytecode->isns.size() > 0);
        
        std::ostringstream debug_out;
        debug_format_bytecode(debug_out, bytecode);
        std::string debug_str = debug_out.str();
        CHECK(debug_str.length() > 0);
    }
}

TEST_CASE("bytecode - native function calls") {
    SUBCASE("simple function call compilation") {
        const char* source = "add(5, 3)";
        Program* program = parse_program(source);
        REQUIRE(program != nullptr);
        
        Bytecode* bytecode = compile_program(program);
        REQUIRE(bytecode != nullptr);
        
        CHECK(bytecode->isns.size() > 0);
        
        std::ostringstream debug_out;
        debug_format_bytecode(debug_out, bytecode);
        std::string debug_str = debug_out.str();
        CHECK(debug_str.length() > 0);
    }
}

TEST_CASE("bytecode - control flow operations") {
    SUBCASE("comparison operation compilation") {
        const char* source = "eq(5, 3)";
        Program* program = parse_program(source);
        REQUIRE(program != nullptr);
        
        Bytecode* bytecode = compile_program(program);
        REQUIRE(bytecode != nullptr);
        
        CHECK(bytecode->isns.size() > 0);
        
        std::ostringstream debug_out;
        debug_format_bytecode(debug_out, bytecode);
        std::string debug_str = debug_out.str();
        CHECK(debug_str.length() > 0);
        
        // Check that the compilation included an i32_eq instruction
        CHECK(debug_str.find("op_i32_eq") != std::string::npos);
    }
    
    SUBCASE("less than operation compilation") {
        const char* source = "lt(10, 20)";
        Program* program = parse_program(source);
        REQUIRE(program != nullptr);
        
        Bytecode* bytecode = compile_program(program);
        REQUIRE(bytecode != nullptr);
        
        CHECK(bytecode->isns.size() > 0);
        
        std::ostringstream debug_out;
        debug_format_bytecode(debug_out, bytecode);
        std::string debug_str = debug_out.str();
        CHECK(debug_str.length() > 0);
        
        // Check that the compilation included an i32_lt instruction
        CHECK(debug_str.find("op_i32_lt") != std::string::npos);
    }
}
