#include "runtime/const_data_buffer.h"

ConstDataBuffer::ConstDataBuffer() {
    // Initialize with empty vector
    data = std::vector<u8>();
}

u32 ConstDataBuffer::alloc_variant_32(const Variant32& value) {
    // Reserve space for a Variant32 at the end of the buffer

    size_t new_value_offset = data.size();

    // Add 1 byte for the type tag.
    auto data_size = 1 + sizeof(Variant32);

    data.resize(new_value_offset + data_size);

    size_t write_pos = new_value_offset;

    // Write the type tag
    data[write_pos] = static_cast<u8>(VariantSize::Variant32);
    write_pos += 1;

    // Write the data
    Variant32* value_ptr = reinterpret_cast<Variant32*>(&data[write_pos]);
    *value_ptr = value;

    // The exposed offset is 1 based.
    return new_value_offset + 1;
}

Variant32* ConstDataBuffer::get_variant_32(u32 offset) {

    if (offset <= 0 || offset >= data.size()) {
        std::string error_message = "ConstDataBuffer::get_variant_32: Invalid offset: " + std::to_string(offset);
        throw std::runtime_error(error_message);
    }

    auto actual_offset = offset - 1;

    // Check the type tag
    if (data[actual_offset] != static_cast<u8>(VariantSize::Variant32)) {
        std::string error_message = "ConstDataBuffer::get_variant_32: Invalid access, data has tag: " + std::to_string(data[actual_offset]);
        throw std::runtime_error(error_message);
    }

    actual_offset += 1;

    return reinterpret_cast<Variant32*>(&data[actual_offset]);
}