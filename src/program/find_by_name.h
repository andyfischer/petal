
#pragma once

#include "program/term_ref.h"
#include "program/program.h"
#include "program/term.h"

Term* find_term_by_name(Program* program, SymbolId name_id, const Term* position);
Term* resolve_term_ref(Program* program, TermRef ref, const Term* position);