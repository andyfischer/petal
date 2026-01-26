#pragma once

#include "bytecode/bytecode.h"

struct Program;
struct Bytecode;

Bytecode* compile_program(Program* program);
