
interface OperandDetails {
    category: string;
    value?: number;
    params?: string;
    native_func_name?: string;
    comment?: string;
}

const FixedBytecodeSize = 32;
const OpcodeSectionSize = 8;

const EveryBytecodeOperand: Record<string, OperandDetails> = {

    // Empty opcodes
    op_unreachable: {
        category: 'execution',
        value: 0,
        comment: 'Indicates an unreachable code path that should never be executed',
    },
    op_nope: {
        category: 'execution',
        comment: 'No operation - does nothing',
    },
    op_stop: {
        category: 'execution',
        comment: 'Halts execution of the current program',
    },

    // Function calls
    op_call: {
        category: 'execution',
        params: "func_address(u16) stack_size(u8)",
        comment: 'Calls function at address, saves frame header in local:0',
    },
    op_return: {
        category: 'execution',
        params: "return_slot(u8)",
        comment: 'Returns from current function with value from specified slot',
    },
    op_call_host: {
        category: 'execution',
        params: "symbol_slot(u8) argc(u8)",
        comment: 'Calls a host function by symbol, with specified argument count',
    },

    // Data movement
    op_move: {
        category: 'execution',
        params: "from_slot(u8) to_slot(u8)",
        comment: 'Moves value from one slot to another',
    },
    op_reserve_slots: {
        category: 'execution',
        params: "count(u16)",
        comment: 'Reserves the specified number of slots on the stack',
    },

    // Inline constants
    op_const_i16: {
        category: 'math',
        params: "slot(u8) value(i16)",
        comment: 'Loads a 16-bit signed integer constant into a stack slot',
    },
    op_const_u16: {
        category: 'math',
        params: "slot(u8) value(u16)",
        comment: 'Loads a 16-bit unsigned integer constant into a stack slot',
    },
    op_const_u16_sym: {
        category: 'math',
        params: "slot(u8) value(u16)",
        comment: 'Loads a 16-bit symbol ID constant into a stack slot',
    },

    /*
    op_local_set: {
        category: 'variable',
        params: "local_idx(u32)",
    },
    op_local_get: {
        category: 'variable',
        params: "local_idx(u32)",
    },
    */

    // Builtin math
    op_i32_add: {
        category: 'math',
        native_func_name: 'add',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Adds two 32-bit integers from the specified stack slots',
    },
    op_i32_sub: {
        category: 'math',
        native_func_name: 'sub',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Subtracts two 32-bit integers from the specified stack slots',
    },
    op_i32_mult: {
        category: 'math',
        native_func_name: 'mult',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Multiplies two 32-bit integers from the specified stack slots',
    },
    op_i32_div_s: {
        category: 'math',
        native_func_name: 'div',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Divides two 32-bit signed integers from the specified stack slots',
    },
    op_i32_div_u: {
        category: 'math',
        comment: 'Divides two 32-bit unsigned integers',
    },


    // Unary
    op_copy: {
        category: 'variable',
        params: "from_slot(u8) to_slot(u8)",
        comment: 'Copies a value from one stack slot to another',
    },

    // Control flow operations
    op_jump: {
        category: 'control',
        params: "address(u16)",
        comment: 'Unconditional jump to specified address',
    },
    op_jump_if_true: {
        category: 'control',
        params: "condition_slot(u8) address(u16)",
        comment: 'Jump to address if condition slot is true (non-zero)',
    },
    op_jump_if_false: {
        category: 'control',
        params: "condition_slot(u8) address(u16)",
        comment: 'Jump to address if condition slot is false (zero)',
    },

    // Comparison operations for conditions
    op_i32_eq: {
        category: 'math',
        native_func_name: 'eq',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Compare two 32-bit integers for equality',
    },
    op_i32_lt: {
        category: 'math',
        native_func_name: 'lt',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Compare if slot_a < slot_b (signed)',
    },
    op_i32_gt: {
        category: 'math',
        native_func_name: 'gt',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Compare if slot_a > slot_b (signed)',
    },
    op_i32_le: {
        category: 'math',
        native_func_name: 'le',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Compare if slot_a <= slot_b (signed)',
    },
    op_i32_ge: {
        category: 'math',
        native_func_name: 'ge',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Compare if slot_a >= slot_b (signed)',
    },
    op_i32_ne: {
        category: 'math',
        native_func_name: 'ne',
        params: "slot_a(u8) slot_b(u8) slot_out(u8)",
        comment: 'Compare two 32-bit integers for inequality',
    },

    // For loop support
    op_i32_inc: {
        category: 'math',
        native_func_name: 'inc',
        params: "slot(u8)",
        comment: 'Increment 32-bit integer in slot by 1',
    },

    /*
    op_return: {
        category: 'control',
    },
    op_end: {
        category: 'control',
    },
    op_loop: {
        category: 'control',
    },
    op_block: {
        category: 'control',
    },
    op_if: {
        category: 'control',
    },
    op_br: {
        category: 'control',
        params: "label_idx(u16)",
    },
    op_br_if: {
        category: 'control',
        params: "label_idx(u16)",
    },
    op_exec_grow_locals: {
        category: 'execution',
        params: "local_count(u32)",
    },
    op_exec_shrink_locals: {
        category: 'execution',
        params: "local_count(u32)",
    },
    op_temp_call_block_id: {
        category: 'execution',
        params: "block_id(u32)",
    },
    */
    op_compile_error: {
        category: 'debug',
        params: "error_idx(u16)",
        comment: 'Represents a compile-time error at this location',
    },
    op_comment: {
        category: 'debug',
        params: "comment_idx(u16)",
        comment: 'Represents a comment at this location',
    },
};

const FirstValueByCategory = {
    execution: 0x00,
    variable: 0x10,
    memory: 0x20,
    math: 0x30,
    control: 0x40,
    misc: 0x50,
    debug: 0x60,
};

export function* everyBytecodeOp() {
    const nextValueByCategory = new Map();
    const usedValues = new Set();
    function takeNextValue(category) {
        for (let iteration = 0; iteration < 10; iteration++) {
            if (!nextValueByCategory.has(category)) {
                if (FirstValueByCategory[category] == null) {
                    throw new Error(`Category missing from FirstValueByCategory: ${category}`);
                }
                nextValueByCategory.set(category, FirstValueByCategory[category]);
            }
            const nextValue = nextValueByCategory.get(category);
            nextValueByCategory.set(category, nextValue + 1);
            if (!usedValues.has(nextValue)) {
                usedValues.add(nextValue);
                return nextValue;
            }
        }
        throw new Error(`Failed to find a unique ID for category ${category}`);
    }
    for (const [name, details] of Object.entries(EveryBytecodeOperand)) {
        const const_ident = name.toUpperCase().replace(/^OP_/, "OP_");

        if (details.value != null) {
            if (usedValues.has(details.value)) {
                throw new Error(`Duplicate value for ${name}: ${details.value}`);
            }
            usedValues.add(details.value);
        }
        const value = details.value ?? takeNextValue(details.category);
        // Parse the params string

        let params = details.params?.split(' ').map(param => {
            let [name, type] = param.split('(');
            if (!type) {
                throw new Error(`Invalid param format: ${param}`);
            }
            type = type.replace(')', '');
            // parse "u32" to get the bit size
            let bit_size = parseInt(type.match(/(\d+)/)[1], 10);
            return { name, bit_size, type };
        });

        const total_bit_size = 8 + (params?.reduce((acc, param) => acc + param.bit_size, 0) ?? 0);
        yield {
            const_ident,
            value,
            name,
            category: details.category,
            params: params ?? [],
            total_bit_size,
            native_func_name: stringToCapitalCase(details.native_func_name ?? ''),
            comment: details.comment,
        };
    }
}
function stringToCapitalCase(s) {
    if (!s)
        return s;
    // Split the string into words (by underscores)
    // Capitalize the first letter of each word
    // Join the words back together
    return s.split('_').map(word => word.charAt(0).toUpperCase() + word.slice(1)).join('');
}
//# sourceMappingURL=bytecode_ops.js.map
