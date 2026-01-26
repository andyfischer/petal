#include <sstream>
#include <string>
#include <vector>

#include "bytecode/debug_format_program.h"
#include "program/block.h"
#include "program/program.h"
#include "program/term.h"
#include "program/term_ref.h"
#include "program/term_id.h"
#include "globals/global_state.h"
#include "program/name_map.h"
#include "runtime/native_funcs.h"
#include "program/find_by_name.h"
#include "variant/variant_debug.h"

// Helper function to convert a term ID reference to a string
void write_term_id_ref(std::ostream& os, const TermId& location, const TermId& term_id) {
    if (location.block_id == term_id.block_id) {
        os << "$" << term_id.term_local_id;
    } else {
        os << "$block$" << term_id.block_id << "$" << term_id.term_local_id;
    }
}


// Convert a symbolic reference to a string
void write_term_ref(std::ostream& os, Program* program, const TermId& location, const TermRef& sref) {
    switch (sref.type) {
        case TermRefType::TermIdRef:
            write_term_id_ref(os, location, sref.term_id);
            break;
            
        case TermRefType::NameRef: {
            const char* name = get_active_global_state()->symbols.get_name_string(sref.name_id);
            std::string name_str(name ? name : "unknown");
            
            Term* location_term = program->get_term(location);
            Term* found_term = find_term_by_name(program, sref.name_id, location_term);

            if (found_term) {
                os << name_str;
                write_term_id_ref(os, location, found_term->term_id);
            } else {
                os << name_str << "$?";
            }
            break;
        }
            
        case TermRefType::NativeFunctionRef:
            // Unexpected to see a native function reference here.
            os << "native_func_" << sref.name_id;
            break;
            
        case TermRefType::None:
        default:
            os << "(none)";
    }
}


void write_term_func(std::ostream& os, Program* program, const TermId& location, const TermRef& func) {
    if (func.is_native_function()) {
        switch (func.native_function_id) {
            case NativeFunctionId::Input:
                os << "#input";
                break;
            case NativeFunctionId::Value:
                os << "#value";
                break;
            default:
                os << "#unknown_native_func_" << static_cast<int>(func.native_function_id);
        }
        return;
    }

    write_term_ref(os, program, location, func);
}

// Format a single block for debugging
void write_block(std::ostream& os, Program* program, Block* block) {
    std::vector<std::string> lines;
    
    os << "Block: " << block->block_id << std::endl;
    
    for (auto term : block->terms.items) {
        os << " $" << term->term_id.term_local_id << " ";
        
        if (term->name_id != 0) {
            const char* name = get_active_global_state()->symbols.get_name_string(term->name_id);
            if (name) {
                os << "let " << std::string(name) << " = ";
            }
        }
        
        write_term_func(os, program, term->term_id, term->func);
        os << "(";
        
        for (size_t inputIndex = 0; inputIndex < term->inputs.size(); inputIndex++) {
            if (inputIndex > 0) {
                os << ", ";
            }
            write_term_ref(os, program, term->term_id, term->inputs[inputIndex]);
        }
        
        os << ")";
        
        // Check if the term has a fixed value
        if (term->has_fixed_value()) {
            Variant32* value = term->get_fixed_value();
            os << " // ";
            debug_format_variant32(os, *value);
        }
        
        os << std::endl;
    }
}

// Format all blocks in the program for debugging
void debug_format_program(std::ostream& os, Program* program) {
    for (auto block : program->blocks.items) {
        write_block(os, program, block);
    }
}
