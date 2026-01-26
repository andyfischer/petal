# Petal Development Guide

This guide covers how to build the Petal project and use the various development tools.

# Code Organization #

 `./ts/`             - Typescript helper code, for code generation and some testing.
 `./src/`            - The project's C++ code.
 `./src/bytecode`    - Bytecode op handling and compilation.
 `./src/cli`         - Implementation of the command-line too.
 `./src/codegen`     - (Future) Code for generating programs in other languages from Petal programs.
 `./src/parser`      - Parses source text into a parsed program.
 `./src/program`     - Data structures for parsed programs.
 `./src/runtime`     - The virtual machine implementation.
 `./src/third_party` - Code for 3rd party libraries.
 `./src/utils`       - Miscellaneous helper code.
 `./samples/`        - Samples of Petal code files.

# Important Classes #

### Parsed program / AST ###

Parsed source is stored using Program / Block / Term:

 - `Program` - Top level data structure for a parsed program. Stores a collection of Blocks.
 - `Block` - A single control-flow block. Has an ordered list of Terms. Has a scope of local names.
 - `Term` - A single expression or statement node. Has inputs and may produce an output.

Other important classes:

 - `GlobalState` - Object that holds various state for the current environment. Usually there is
   only one of these.

# How names are handled #

All the names used in programs, and some other names, are interned as "symbols"

 - `GlobalState` contains a `NameMap` which has a bidirectional map from the SymbolId (an unsigned int) to
   the full string.
 - The NameMap is populated as needed, during parsing and other times.
 - Across the code, the SymbolId type is used anywhere that names are stored.

# Terminology #

Various terms used inside the project:

 * "native" - If something is named "native" then it's handled by builtin ops in Petal bytecode.
 * "host" - The "host" is the outside client environment that has embedded Petal and is using it.
 * "host function" - a callback provided by the client environment.

## Building the Project

### Prerequisites

- C++ compiler with C++17 support (g++ or clang++)
- Make
- Node.js and Yarn (for running tests)
- The `cpp-build-tool` (located at `../cpp-build-tool/`)

### Build Commands

To build the CLI executable:

```bash
make
```

This will:
1. Generate a unity build file (`src/unity.cpp`) using the cpp-build-tool
2. Compile the project into `dist/cli/main`

Other useful make commands:

- `make clean` - Remove all build artifacts
- `make rebuild` - Clean and rebuild from scratch
- `make test` - Build and run the CLI tests
- `make install` - Install the executable to `/usr/local/bin/petal`

The Makefile automatically tracks dependencies on all `.cpp` and `.h` files, so any changes to source files will trigger a rebuild.

## CLI Test Options

The Petal CLI provides several testing commands for development and debugging:

### Running Scripts

To run a Petal script file:

```bash
./bin/petal script.petal
```

### Test Commands

#### `-test`
Run the built-in unit test suite:

```bash
./bin/petal -test
```

#### `-test-parse <file>`
Parse a file and output the AST structure without trace information:

```bash
./bin/petal -test-parse example.petal
```

This is useful for verifying that the parser correctly understands the program structure.

#### `-test-parse-extended <file>`
Parse a file and output the AST structure with detailed trace information:

```bash
./bin/petal -test-parse-extended example.petal
```

This shows the full parsing process with XML-style trace output, helpful for debugging parser issues.

#### `-test-lex <file>`
Run the lexer on a file and output the token stream as JSON:

```bash
./bin/petal -test-lex example.petal
```

Output format:
```json
[
  {"text": "let", "tok": 15},
  {"text": "x", "tok": 1},
  {"text": "=", "tok": 37},
  {"text": "5", "tok": 3}
]
```

#### `-test-compile <file>`
Parse and compile a file, then output the bytecode:

```bash
./bin/petal -test-compile example.petal
```

This shows the compiled bytecode instructions, useful for debugging the compiler.

### Error Handling

If an unrecognized command is provided, the CLI will output an error:

```bash
./bin/petal -unknown
Error: Unrecognized command '-unknown'
```

## Testing Strategy

The project uses multiple testing approaches:

1. **Vitest Tests** (`ts/test/*`) - Comprehensive test suite that runs the C++ code compiled to WebAssembly
   - Build WASM and run: `yarn node-build-and-vitest`

Advantages of Vitest based testing: Much more expansive and thorough. Easier to have snapshots.
Test runner is easier to use.

Disadvantage of Vitest based testing: Harder to debug when a crash happens.

2. **C++ Doctest Unit Tests** - Basic tests to verify the native code doesn't crash
   - Stored with the .cpp code as test_xxx.cpp
   - Run with: `make && ./bin/petal -test`

Advantages of Doctest: Easier to debug crashes. Can use `gdb` or etc more easily.

## Development Workflow

1. Make changes to C++ source files
2. Run `make` to rebuild
3. Test your changes using the appropriate CLI commands
. Commit your changes when tests pass

## Memory Leak Detection

### Ubuntu ###
When running in Ubuntu, you can use `valgrind` to check for leaks.

    $ valgrind bin/petal ...


### MacOS

The `leaks` tool is built into macOS and can detect memory leaks at program exit.

```bash
# Run the program with leak detection
make leaks

# Run tests with leak detection
make test-leaks
```

The leaks tool will report any leaked memory when the program exits. It uses `MallocStackLogging=1`
to enable stack traces for allocations.

