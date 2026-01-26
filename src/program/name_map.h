#pragma once

#include "standard_headers.h"
#include <unordered_map>
#include <string>
#include "utils/id_source.h"

struct NameMap {
    std::unordered_map<u32, std::string> id_to_string;
    std::unordered_map<std::string, u32> string_to_id;
    IDSource next_id;

    NameMap();
    
    // Get an existing symbol or create a new one.
    u32 get_or_add_name(const char* name);

    u32 get_name(const char* name);

    // Get the string for a symbol ID
    const char* get_name_string(u32 name_id) const;
};