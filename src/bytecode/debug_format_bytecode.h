#pragma once

#include "standard_headers.h"
#include <string>

// Forward declarations
struct Bytecode;

void debug_format_bytecode(std::ostream& os, Bytecode* bytecode);