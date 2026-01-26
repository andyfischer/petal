# Changelog

## 2025-06-25 - Math Operations Refactoring and Error Handling Implementation

### Summary of Changes

- **Updated math operations to use 3-slot pattern**: Modified `op_i32_sub`, `op_i32_mult`, and `op_i32_div_s` to follow the same pattern as `op_i32_add` with separate input and output slots
- **Implemented comprehensive error handling system**: Built a complete error message system for bytecode compilation that stores error messages in `const_data` and displays them in bytecode dumps
- **Enhanced CLI argument validation**: Fixed segmentation faults caused by missing filename arguments in test commands
- **Added utility functions**: Created `add_const_str()` and `get_const_data_str()` for managing string data in bytecode
- **Fixed const_data index remapping**: Implemented proper index adjustment for multi-block compilation to ensure error messages point to correct locations

### Technical Details

- **Bytecode Operations**: Updated TypeScript definitions in `ts/bytecodeOps.ts` and regenerated C++ headers to include `slot_out` parameter for all math operations
- **VM Implementation**: Modified `src/runtime/vm.cpp` to use the new 3-slot pattern for math operations, preventing destructive updates to input values
- **Compiler**: Updated `src/bytecode/compile.cpp` to call math operations with proper slot allocation and error handling
- **Error System**: 
  - `record_compile_error()` now stores messages in `const_data` and emits `op_compile_error` instructions
  - `debug_format_bytecode()` displays both error index and actual error message text
  - Multi-block compilation correctly remaps const_data indexes to prevent conflicts
- **CLI Safety**: Added bounds checking for all file-based test commands to prevent segmentation faults

### Considerations and Next Steps

1. **Float Support**: The current system generates compile errors for float constants. Consider adding `op_const_f32` instruction for proper float handling.

2. **Error Recovery**: The current error system stops compilation on errors. Consider implementing error recovery to continue compilation and report multiple issues.

3. **Error Categorization**: Add error severity levels (warning, error, fatal) and error codes for better tooling integration.

4. **Testing Coverage**: Add specific tests for the error handling system and multi-block const_data remapping.

5. **Performance**: Consider optimizing string storage in const_data with deduplication for repeated error messages.

### Things Learned During This Session

1. **Bytecode Generation Architecture**: Understanding how TypeScript definitions drive C++ code generation and the importance of keeping them synchronized.

2. **Multi-Block Compilation**: Learning about the complexities of combining bytecode from multiple blocks and the need for index remapping.

3. **CLI Argument Parsing**: Discovered the importance of bounds checking when accessing command line arguments to prevent crashes.

4. **Destructive vs Non-Destructive Operations**: Understanding why math operations should use separate output slots rather than overwriting input values.

5. **Error Message Storage**: Learning how to efficiently store and retrieve variable-length strings in fixed bytecode structures.

### Documentation Suggestions

The following should be added to project documentation:

1. **Bytecode Development Guide**: Document the process of adding new bytecode operations, including updating TypeScript definitions, regenerating headers, and implementing VM handlers.

2. **Error Handling Best Practices**: Guidelines for when and how to use `record_compile_error()`, with examples of good error messages.

3. **Multi-Block Compilation**: Explain how const_data and other resources are combined across blocks and why index remapping is necessary.

4. **CLI Development**: Document the pattern for adding new CLI commands with proper argument validation.

5. **Debugging Bytecode**: Guide on using `-test-compile` and `-test-compile-stdin` for debugging compilation issues and interpreting bytecode dumps.

### Files Modified

- `ts/bytecodeOps.ts` - Added `slot_out` parameter to math operations
- `src/bytecode/bytecode_encoding.h` - Regenerated with new pack/unpack functions
- `src/runtime/vm.cpp` - Updated math operations to use 3-slot pattern
- `src/bytecode/compile.cpp` - Implemented error handling and const_data remapping
- `src/bytecode/bytecode.h` - Added `get_const_data_str()` method
- `src/bytecode/debug_format_bytecode.cpp` - Enhanced error message display
- `src/cli/main.cpp` - Added argument validation for test commands
- `Makefile` - Disabled warnings with `-w` flag

## 2025-06-24 - WebAssembly Build System Refactoring

### Summary of Changes

- Created a new TypeScript-based WebAssembly build script (`ts/buildWasm.ts`) to replace the complex cpp-build dependency
- Consolidated all WebAssembly build functionality into a single, self-contained file
- Added support for both web and Node.js target builds via command line flags (`--target web` or `--target node`)
- Integrated with existing test infrastructure - Vitest tests can now load and execute the generated WASM modules
- Added new npm script `build-and-vitest` that builds WASM and runs tests in one command
- Suppressed unnecessary compiler warnings for cleaner build output

### Technical Details

- **New script**: `ts/buildWasm.ts` - Standalone TypeScript build script using Emscripten directly
- **Removed dependency**: No longer requires importing from `@scaffold/cpp-build`
- **Uses existing unity file**: Leverages the pre-generated `src/unity.cpp` instead of creating its own
- **Output locations**: 
  - Web target: `dist/wasm/petal.js` + `petal.wasm`
  - Node.js target: `dist/wasm/petal-node.js` + `petal-node.wasm`
- **Test integration**: Tests load WASM module from expected location and execute successfully

### Considerations and Next Steps

1. **Test Snapshot Updates**: Some test snapshots are mismatched (symbol IDs changed from 1 to 12). These need to be updated or investigated to understand why symbol numbering changed.

2. **Build Script Improvements**: 
   - Consider adding more build targets (debug/release modes)
   - Add build caching or incremental builds for faster development
   - Add validation that EMSDK_DIR is correctly set and Emscripten is available

3. **Documentation**: The new build script should be documented in README.md with usage examples

4. **Error Handling**: Add better error messages when builds fail, especially for common issues like missing EMSDK

5. **Performance**: Consider optimizing compiler flags for different use cases (development vs production)

### Things Learned During This Session

1. **Emscripten Integration**: Understanding how to properly set up Emscripten compiler flags for both web and Node.js targets, including the differences in module loading and environment settings.

2. **Unity Build Complexity**: The existing unity build system has specific include/exclude rules that needed to be understood to avoid duplicate symbol errors.

3. **WASM Module Loading**: The test infrastructure expects specific file locations and naming conventions that the build system must respect.

4. **Compiler Warning Management**: C++ projects often have many harmless warnings that can be selectively disabled for cleaner output without affecting functionality.

5. **Build System Dependencies**: Removing external dependencies (like cpp-build) requires understanding all the functionality they provided and reimplementing it correctly.

### Documentation Suggestions

The following should be added to project documentation:

1. **EMSDK Setup Guide**: Clear instructions on installing and configuring Emscripten SDK, including setting EMSDK_DIR environment variable.

2. **Build Target Guide**: Document the differences between web and Node.js targets, when to use each, and how the generated files differ.

3. **Testing WebAssembly**: Explain how the test infrastructure loads and executes WASM modules, and what to do when tests fail due to module loading issues.

4. **Troubleshooting Build Issues**: Common problems like:
   - Missing EMSDK_DIR
   - Unity file not found
   - Symbol redefinition errors
   - Snapshot test mismatches

5. **Development Workflow**: Recommended commands for different development scenarios (quick testing, full build, etc.).

### Files Modified

- `ts/buildWasm.ts` (new) - Main build script
- `package.json` - Added `build-and-vitest` script
- Existing build works with `src/unity.cpp` (generated externally)