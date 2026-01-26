#include "variant_debug.h"

void debug_format_variant32(std::ostream& os, const Variant32& variant) {
    switch (variant.type) {
        case VariantType::I32:
            os << "Int32::" << variant.get_int32();
            break;
        case VariantType::Float32:
            os << "Float32::" << variant.get_float32();
            break;
        case VariantType::Symbol:
            os << "Symbol::" << variant.get_symbol_id();
            break;
        case VariantType::FunctionDef:
            os << "FunctionDef(Block$" << variant.get_block_id() << ")";
            break;
        default:
            os << "(unhandled variant type: " << static_cast<int>(variant.type) << ")";
            break;
    }
}