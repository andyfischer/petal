#pragma once

#include "standard_headers.h"
#include "variant/variant.h"
#include <vector>

struct ConstDataBuffer {
    std::vector<u8> data;

    ConstDataBuffer();

    // Add a variant32 to the buffer and return the offset.
    u32 alloc_variant_32(const Variant32& value);

    // Get a pointer to a variant32 at an offset.
    Variant32* get_variant_32(u32 offset);
};
