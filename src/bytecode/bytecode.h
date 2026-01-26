#pragma once

#include "standard_headers.h"
#include <vector>
#include <unordered_map>

struct EntryPoint {
    u32 entry_pc;
    u32 frame_size;
};

struct Bytecode {
    std::vector<Instruction> isns;
    std::vector<u8> const_data;
    std::unordered_map<BlockId, EntryPoint> entry_points;
    
    const char* get_const_data_str(u16 offset) const {
        if (offset >= const_data.size()) {
            return nullptr;
        }
        return reinterpret_cast<const char*>(&const_data[offset]);
    }
};
