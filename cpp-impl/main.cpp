#include "parser.h"
#include "interpreter.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>

static char* read_file(const char* path) {
    FILE* file = fopen(path, "rb");
    if (file == nullptr) {
        fprintf(stderr, "Could not open file '%s'\n", path);
        return nullptr;
    }

    fseek(file, 0, SEEK_END);
    long size = ftell(file);
    fseek(file, 0, SEEK_SET);

    char* buffer = (char*)malloc(size + 1);
    if (buffer == nullptr) {
        fprintf(stderr, "Could not allocate memory for file\n");
        fclose(file);
        return nullptr;
    }

    size_t bytes_read = fread(buffer, 1, size, file);
    buffer[bytes_read] = '\0';

    fclose(file);
    return buffer;
}

static void run_file(const char* path) {
    char* source = read_file(path);
    if (source == nullptr) {
        exit(1);
    }

    Parser parser;
    parser_init(&parser, source);

    ASTNode* program = parser_parse(&parser);

    if (parser_had_error(&parser)) {
        fprintf(stderr, "%s\n", parser_error_message(&parser));
        free(source);
        exit(1);
    }

    Interpreter interp;
    interpreter_init(&interp);

    interpreter_run(&interp, program);

    if (interp.had_error) {
        fprintf(stderr, "%s\n", interp.error_message);
        free(source);
        exit(1);
    }

    free(source);
}

static void run_repl() {
    printf("Petal REPL (type 'exit' to quit)\n");

    Interpreter interp;
    interpreter_init(&interp);

    char line[1024];

    while (true) {
        printf("> ");
        fflush(stdout);

        if (fgets(line, sizeof(line), stdin) == nullptr) {
            printf("\n");
            break;
        }

        // Remove trailing newline
        size_t len = strlen(line);
        if (len > 0 && line[len - 1] == '\n') {
            line[len - 1] = '\0';
        }

        if (strcmp(line, "exit") == 0 || strcmp(line, "quit") == 0) {
            break;
        }

        if (strlen(line) == 0) {
            continue;
        }

        Parser parser;
        parser_init(&parser, line);

        ASTNode* program = parser_parse(&parser);

        if (parser_had_error(&parser)) {
            fprintf(stderr, "%s\n", parser_error_message(&parser));
            continue;
        }

        Value result = interpreter_eval(&interp, program);

        if (interp.had_error) {
            fprintf(stderr, "%s\n", interp.error_message);
            interp.had_error = false;
            interp.error_message[0] = '\0';
            continue;
        }

        // Print result if not null
        if (result.type != VAL_NULL) {
            print_value(result);
            printf("\n");
        }
    }
}

static void print_usage(const char* program_name) {
    printf("Usage: %s [options] [script]\n", program_name);
    printf("\n");
    printf("Options:\n");
    printf("  -h, --help     Show this help message\n");
    printf("  -v, --version  Show version information\n");
    printf("  --ast          Print AST instead of running\n");
    printf("\n");
    printf("If no script is provided, starts an interactive REPL.\n");
}

static void print_version() {
    printf("Petal 0.1.0\n");
}

int main(int argc, char* argv[]) {
    bool print_ast = false;
    const char* script_path = nullptr;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "-h") == 0 || strcmp(argv[i], "--help") == 0) {
            print_usage(argv[0]);
            return 0;
        }
        else if (strcmp(argv[i], "-v") == 0 || strcmp(argv[i], "--version") == 0) {
            print_version();
            return 0;
        }
        else if (strcmp(argv[i], "--ast") == 0) {
            print_ast = true;
        }
        else if (argv[i][0] != '-') {
            script_path = argv[i];
        }
        else {
            fprintf(stderr, "Unknown option: %s\n", argv[i]);
            return 1;
        }
    }

    if (script_path != nullptr) {
        if (print_ast) {
            char* source = read_file(script_path);
            if (source == nullptr) return 1;

            Parser parser;
            parser_init(&parser, source);
            ASTNode* program = parser_parse(&parser);

            if (parser_had_error(&parser)) {
                fprintf(stderr, "%s\n", parser_error_message(&parser));
                free(source);
                return 1;
            }

            ast_print(program, 0);
            free(source);
        }
        else {
            run_file(script_path);
        }
    }
    else {
        run_repl();
    }

    return 0;
}
