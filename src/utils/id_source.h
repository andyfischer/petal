#pragma once

#include "standard_headers.h"

struct IDSource {
    u32 next_id;

    IDSource();
    
    // Get the next available ID
    u32 take();
};