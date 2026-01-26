#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <iostream>
#include <sstream>
#include "parser/parse_program.h"
#include "program/program.h"
#include "bytecode/debug_format_program.h"
#include "parser/parse_token_iterator.h"
#include "bytecode/compile.h"
#include "bytecode/bytecode.h"
#include "bytecode/debug_format_bytecode.h"
#include "runtime/vm.h"
#include "globals/global_state.h"
#include "host/host_api.h"
#include "variant/variant_debug.h"
#include "runtime/heap_types.h"

void cli_run_tests();
void show_help();

// Host function for print
Variant32 host_print(VM* vm) {
    Variant32* value = vm_io_slot(vm, 0);
    
    // Convert the value to string and print it without newline
    HeapString* message_str = variant_to_heap_string(vm, *value);
    printf("%s", message_str->data);
    
    return Variant32::None();
}

void setup_host_functions() {
    GlobalState* gs = get_active_global_state();
    SymbolId print_symbol = gs->get_or_create_symbol("print");
    gs->register_host_function(print_symbol, host_print, 1);
}

char* load_file(const char* path) {
    FILE* file = fopen(path, "r");
    if (!file) {
        printf("Error: Failed to open file %s\n", path);
        return nullptr;
    }

    fseek(file, 0, SEEK_END);
    long fileSize = ftell(file);
    fseek(file, 0, SEEK_SET);

    char* fileContent = (char*)malloc(fileSize + 1);
    fread(fileContent, 1, fileSize, file);
    fileContent[fileSize] = '\0';

    fclose(file);
    return fileContent;
}

char* load_stdin() {
    std::stringstream buffer;
    std::string line;
    
    // Read all lines from stdin
    while (std::getline(std::cin, line)) {
        buffer << line << '\n';
    }
    
    std::string content = buffer.str();
    
    // Allocate and copy to C-style string
    char* result = (char*)malloc(content.length() + 1);
    strcpy(result, content.c_str());
    
    return result;
}

void run_test_parse(const char* path) {
    char* fileContent = load_file(path);

    // parse the file
    Program* program = parse_program(fileContent, ParseProgramOptions{ .stdout_trace = false });

    // format the program
    debug_format_program(std::cout, program);

    delete program;
    free(fileContent);
}

void run_test_parse_extended(const char* path) {
    char* fileContent = load_file(path);

    // parse the file with extended trace output
    Program* program = parse_program(fileContent, ParseProgramOptions{ .stdout_trace = true });

    // format the program
    debug_format_program(std::cout, program);

    delete program;
    free(fileContent);
}

void run_test_lex(const char* path) {
    char* fileContent = load_file(path);

    auto& out = std::cout;

    out << "[" << std::endl;
    bool first = true;
    TokenIterator it(fileContent);
    while (!it.finished()) {
        if (!first)
            out << "," << std::endl;
        first = false;
        out << "{\"text\": \"" << it.next_text() << "\", \"tok\": "
            << static_cast<int>(it.next()->tok_match) << "}";
        it.consume();
    }
    out << "]" << std::endl;

    free(fileContent);
}

void run_test_compile(const char* path) {
    // Set up host functions for compilation
    setup_host_functions();
    
    char* fileContent = load_file(path);

    Program* program = parse_program(fileContent, ParseProgramOptions{ });

    Bytecode* bytecode = compile_program(program);

    debug_format_bytecode(std::cout, bytecode);

    delete bytecode;
    delete program;
    free(fileContent);
}

// Stdin versions of test functions
void run_test_parse_stdin() {
    char* content = load_stdin();

    // parse the input
    Program* program = parse_program(content, ParseProgramOptions{ .stdout_trace = false });

    // format the program
    debug_format_program(std::cout, program);

    delete program;
    free(content);
}

void run_test_parse_extended_stdin() {
    char* content = load_stdin();

    // parse the input with extended trace output
    Program* program = parse_program(content, ParseProgramOptions{ .stdout_trace = true });

    // format the program
    debug_format_program(std::cout, program);

    delete program;
    free(content);
}

void run_test_lex_stdin() {
    char* content = load_stdin();

    auto& out = std::cout;

    out << "[\n";
    bool first = true;
    TokenIterator it(content);
    while (!it.finished()) {
        if (!first)
            out << ",\n";
        first = false;
        out << "{\"text\": \"" << it.next_text() << "\", \"tok\": "
            << static_cast<int>(it.next()->tok_match) << "}";
        it.consume();
    }
    out << "]\n";

    free(content);
}

void run_test_compile_stdin() {
    // Set up host functions for compilation
    setup_host_functions();
    
    char* content = load_stdin();

    Program* program = parse_program(content, ParseProgramOptions{ });

    Bytecode* bytecode = compile_program(program);

    debug_format_bytecode(std::cout, bytecode);

    delete bytecode;
    delete program;
    free(content);
}

void run_script_stdin() {
    // Set up host functions
    setup_host_functions();
    
    // Load and compile from stdin
    char* content = load_stdin();
    if (content == nullptr) {
        return;
    }
    
    Program* program = parse_program(content, ParseProgramOptions{});
    if (program == nullptr) {
        printf("Error: Failed to parse program\n");
        free(content);
        return;
    }
    
    Bytecode* bytecode = compile_program(program);
    if (!bytecode) {
        printf("Error: Failed to compile program\n");
        delete program;
        free(content);
        return;
    }
    
    // Execute the bytecode
    VM* vm = vm_create();
    vm_prepare_entry(vm, bytecode, 1);
    vm_execute(vm, bytecode->isns.data(), bytecode->isns.size());
    
    // Check execution status
    if (vm_get_last_run_status(vm) != VM_STATUS_SUCCESS) {
        printf("VM execution failed\n");
    }
    
    // Cleanup
    vm_destroy(vm);
    delete bytecode;
    delete program;
    free(content);
}

void run_script_file(const char* path) {
    // Set up host functions
    setup_host_functions();
    
    // Load and compile the file
    char* fileContent = load_file(path);
    if (fileContent == nullptr) {
        return;
    }
    
    Program* program = parse_program(fileContent, ParseProgramOptions{});
    if (program == nullptr) {
        printf("Error: Failed to parse program\n");
        free(fileContent);
        return;
    }
    
    Bytecode* bytecode = compile_program(program);
    if (!bytecode) {
        printf("Error: Failed to compile program\n");
        delete program;
        free(fileContent);
        return;
    }
    
    // Execute the bytecode
    VM* vm = vm_create();
    vm_prepare_entry(vm, bytecode, 1);
    vm_execute(vm, bytecode->isns.data(), bytecode->isns.size());
    
    // Check execution status
    if (vm_get_last_run_status(vm) != VM_STATUS_SUCCESS) {
        printf("VM execution failed\n");
    }
    
    // Cleanup
    vm_destroy(vm);
    delete bytecode;
    delete program;
    free(fileContent);
}

void show_help() {
    printf("Petal Programming Language\n");
    printf("Usage: petal [options] [file]\n");
    printf("\n");
    printf("Options:\n");
    printf("  -h, --help                    Show this help message\n");
    printf("  -test                         Run built-in unit tests\n");
    printf("  -stdin                        Execute script from standard input\n");
    printf("\n");
    printf("Development and debugging options:\n");
    printf("  -test-lex <file>              Perform lexical analysis on file\n");
    printf("  -test-lex-stdin               Perform lexical analysis on stdin\n");
    printf("  -test-parse <file>            Parse file and show AST\n");
    printf("  -test-parse-stdin             Parse stdin and show AST\n");
    printf("  -test-parse-extended <file>   Parse file with extended tracing\n");
    printf("  -test-parse-extended-stdin    Parse stdin with extended tracing\n");
    printf("  -test-compile <file>          Compile file to bytecode\n");
    printf("  -test-compile-stdin           Compile stdin to bytecode\n");
    printf("\n");
    printf("Examples:\n");
    printf("  petal script.ca               Execute script.ca\n");
    printf("  petal -stdin                  Read and execute from stdin\n");
    printf("  echo 'send_effect(:log 42)' | petal -stdin\n");
    printf("  petal -test                   Run unit tests\n");
    printf("  petal -test-parse script.ca   Parse and show AST structure\n");
    printf("\n");
}

int main(int argc, char** argv) {
    // If no arguments, show help
    if (argc == 1) {
        show_help();
        return 0;
    }
    
    // If called with exactly one argument that doesn't start with '-', compile and execute
    if (argc == 2 && argv[1][0] != '-') {
        run_script_file(argv[1]);
        return 0;
    }
    
    // Handle other command line options
    for (int argIndex = 1; argIndex < argc; argIndex++) {
        if (argv[argIndex][0] == '-') {
            // Check for help options first
            if (strcmp(argv[argIndex], "-h") == 0 || strcmp(argv[argIndex], "--help") == 0) {
                show_help();
                return 0;
            }
            
            // Check for recognized commands
            if (strcmp(argv[argIndex], "-test-parse") == 0) {
                if (argIndex + 1 < argc) {
                    run_test_parse(argv[argIndex + 1]);
                    argIndex++;
                } else {
                    printf("Error: -test-parse requires a filename\n");
                    return 1;
                }
                continue;
            }

            if (strcmp(argv[argIndex], "-test-parse-extended") == 0) {
                if (argIndex + 1 < argc) {
                    run_test_parse_extended(argv[argIndex + 1]);
                    argIndex++;
                } else {
                    printf("Error: -test-parse-extended requires a filename\n");
                    return 1;
                }
                continue;
            }

            if (strcmp(argv[argIndex], "-test-lex") == 0) {
                if (argIndex + 1 < argc) {
                    run_test_lex(argv[argIndex + 1]);
                    argIndex++;
                } else {
                    printf("Error: -test-lex requires a filename\n");
                    return 1;
                }
                continue;
            }

            if (strcmp(argv[argIndex], "-test-compile") == 0) {
                if (argIndex + 1 < argc) {
                    run_test_compile(argv[argIndex + 1]);
                    argIndex++;
                } else {
                    printf("Error: -test-compile requires a filename\n");
                    return 1;
                }
                continue;
            }

            // Stdin versions of test commands
            if (strcmp(argv[argIndex], "-test-parse-stdin") == 0) {
                run_test_parse_stdin();
                continue;
            }

            if (strcmp(argv[argIndex], "-test-parse-extended-stdin") == 0) {
                run_test_parse_extended_stdin();
                continue;
            }

            if (strcmp(argv[argIndex], "-test-lex-stdin") == 0) {
                run_test_lex_stdin();
                continue;
            }

            if (strcmp(argv[argIndex], "-test-compile-stdin") == 0) {
                run_test_compile_stdin();
                continue;
            }

            if (strcmp(argv[argIndex], "-stdin") == 0) {
                run_script_stdin();
                continue;
            }

            if (strcmp(argv[argIndex], "-test") == 0) {
                cli_run_tests();
                continue;
            }

            // Unrecognized dash command
            printf("Error: Unrecognized command '%s'\n", argv[argIndex]);
            return 1;
        }
    }

    return 0;
}
