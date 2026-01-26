#include <cstring>
#include <cstdlib>
#include <sstream>
#include "program/program.h"
#include "parser/parse_program.h"
#include "bytecode/debug_format_program.h"
#include "parser/parse_token_iterator.h"
#include "bytecode/compile.h"
#include "bytecode/debug_format_bytecode.h"
#include "runtime/vm.h"
#include "globals/global_state.h"

extern "C" {
    size_t add_callback(void (*callback)(const char*)) {
        // TODO: Replace with new host function registration system
        return 0;
    }

    void set_log_callback(size_t callback_id) {
        // TODO: Replace with new host function registration system
    }

    const char* get_library_version() {
        return "petal 0.1";
    }

    const char* debug_get_lexed(const char* program_text) {
        // Hold the exposed result as a static string so that it stays valid
        // long enough for the JS context to use it.
        std::stringstream out;

        TokenIterator it(program_text);

        out << "[" << std::endl;
        bool first = true;
        while (!it.finished()) {
            if (!first)
                out << "," << std::endl;
            first = false;
            out << "{\"text\": \"" << it.next_text() << "\", \"tok\": "
                << static_cast<int>(it.next()->tok_match) << "}";
            it.consume();
        }
        out << "]" << std::endl;

        static std::string result;
        result = out.str();
        return result.c_str();
    }

    const char* debug_get_parsed(const char* program_text) {
        // Hold the exposed result as a static string so that it stays valid
        // long enough for the JS context to use it.
        static std::string result;

        Program* program = parse_program(program_text);
        std::ostringstream oss;

        debug_format_program(oss, program);

        result = oss.str();
        delete program;
        return result.c_str();
    }

    const char* debug_get_bytecode(const char* program_text) {
        static std::string result;

        Program* program = parse_program(program_text);
        Bytecode* bytecode = compile_program(program);

        std::ostringstream oss;
        debug_format_bytecode(oss, bytecode);

        result = oss.str();
        delete bytecode;
        delete program;
        return result.c_str();
    }

    const char* debug_vm_execute(const char* program_text) {
        static std::string result;

        Program* program = parse_program(program_text);
        Bytecode* bytecode = compile_program(program);

        VM* vm = vm_create();
        vm_prepare_entry(vm, bytecode, 1);
        vm_execute(vm, bytecode->isns.data(), bytecode->isns.size());

        std::ostringstream oss;
        oss << "VM execution completed.";

        result = oss.str();
        vm_destroy(vm);
        delete bytecode;
        delete program;
        return result.c_str();
    }

    void debug_reset_global_state() {
        reset_active_global_state();
    }
}


