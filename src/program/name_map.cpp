#include "program/name_map.h"

NameMap::NameMap() : next_id() {
}

u32 NameMap::get_or_add_name(const char* name) {
    std::string name_str(name);
    
    // Check if symbol already exists
    auto it = string_to_id.find(name_str);
    if (it != string_to_id.end()) {
        return it->second;
    }

    // Create new symbol
    u32 name_id = next_id.take();
    
    string_to_id[name_str] = name_id;
    id_to_string[name_id] = name_str;
    
    return name_id;
}

u32 NameMap::get_name(const char* name) {
    std::string name_str(name);
    auto it = string_to_id.find(name_str);
    if (it != string_to_id.end()) {
        return it->second;
    }
    return 0;
}

const char* NameMap::get_name_string(u32 name_id) const {
    auto it = id_to_string.find(name_id);
    if (it != id_to_string.end()) {
        return it->second.c_str();
    }
    return nullptr;
} 