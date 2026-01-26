#include "bytecode/debug_format_bytecode.h"
#include "bytecode/bytecode.h"
#include "bytecode/bytecode_encoding.h"
#include "bytecode/bytecode_helpers.h"

void debug_format_bytecode(std::ostream& os, Bytecode* bytecode) {
    for (Instruction ins : bytecode->isns) {
        u8 opcode = unpack_opcode(ins);
        
        if (opcode == OP_COMPILE_ERROR) {
            // Special handling for compile errors to show the actual message
            u16 error_idx = unpack_op_compile_error__error_idx(ins);
            os << "op_compile_error error_idx:" << error_idx;
            
            // Extract the error message from const_data
            const char* error_msg = bytecode->get_const_data_str(error_idx);
            if (error_msg != nullptr) {
                os << " message:\"" << error_msg << "\"";
            }

            os << "\n";
            continue;
        }

        if (opcode == OP_COMMENT) {
            os << "# ";
            u16 idx = unpack_op_comment__comment_idx(ins);
            const char* comment_msg = bytecode->get_const_data_str(idx);
            if (comment_msg != nullptr) {
                os << comment_msg;
            }
            os << "\n";
            continue;
        }

        debug_format_instruction(os, ins);
        os << "\n";
    }
}
