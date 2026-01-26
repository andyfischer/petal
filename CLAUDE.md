
# Overview

Petal is a programming language implemented in C++

Helpful docs:

 - ./README.md      - Introduction to the project.
 - ./DEVELOPMENT.md - Lots of instructions on how to develop on this codebase.
 - ./docs/development/bytecode.md - Details on the bytecode system. How to add/change bytecode ops.

# Project Coding Guidelines

This document outlines the coding style and conventions used in the Petal project to ensure consistent code quality.

## Genercal coding style

- Feel free to add `// TODO` sections if finishing a section is out of scope for the request.

## General C++ Style

### Naming Conventions

- **Functions:** Use `snake_case` for function names (e.g., `is_letter`, `consume_whitespace`)
- **Variables:** Use `snake_case` for variable names (e.g., `char_position`, `line_position`)
- **Constants:** Use `snake_case` for constants
- **Types/Classes/Structs:** Use `PascalCase` for type names (e.g., `CharIterator`, `FoundToken`)
- **Enums:** Use `PascalCase` for enum class names (e.g., `Token`) and `PascalCase` for enum values

### Class usage

- Only use 'struct' and not 'class'
- Do not use any visibilty keywords (private/protected/etc)
- Prefer to use pointers instead of references.
- Do not use 'const' unless required (such as `const char \*`)
- Prefer to have minimal logic in the constructor function. Do not use the RAII pattern.

### Standard library usage

- Prefer to use 'const char \*' instead of 'std::string' whenever possible.
- Avoid using standard library structures like `std::hash_map` or `std::string` on
  a struct's public API. If they are used then they should only be used internally
  inside the struct that owns them. We want it to be possible to remove `std::`
  structures in the future.
- Do not use `std::optional`

### Code Structure

- Use forward declarations for functions when needed to avoid circular dependencies
- Group related functionality together in the source files
- Use clear, descriptive function names that indicate purpose
- Keep functions focused on a single responsibility

### Indentation and Formatting

- Use 4 spaces for indentation (not tabs)
- Place opening braces on the same line as control statements
- Place else statements on the line after the closing brace
- Use spaces around binary operators
- No space between function name and opening parenthesis
- One space after keywords like if, for, while

### Header Files

- Use `#pragma once` at the top of every file.
- Order includes: standard library, third-party libraries, project includes
- Don't include unnecessary headers
- Whenever possible, function implementations should be in the .cpp file not the .h file.

### Error Handling

- Use assertions (assert) to check invariants and internal logic
- Use defensive programming with clear validation checks


