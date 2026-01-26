#pragma once

#include "standard_headers.h"
#include <vector>

void write_u32(std::vector<u8>& bytes, u32 value) {
    size_t old_size = bytes.size();
    bytes.reserve(4);
    ((u32*)bytes.data())[old_size] = value;
}

void write_u8(std::vector<u8>& bytes, u8 value) {
    size_t old_size = bytes.size();
    bytes.reserve(1);
    ((u8*)bytes.data())[old_size] = value;
}

void write_u16(std::vector<u8>& bytes, u16 value) {
    size_t old_size = bytes.size();
    bytes.reserve(2);
    ((u16*)bytes.data())[old_size] = value;
}

void write_i32(std::vector<u8>& bytes, i32 value) {
    size_t old_size = bytes.size();
    bytes.reserve(4);
    ((i32*)bytes.data())[old_size] = value;
}

void write_i16(std::vector<u8>& bytes, i16 value) {
    size_t old_size = bytes.size();
    bytes.reserve(2);
    ((i16*)bytes.data())[old_size] = value;
}

void write_f32(std::vector<u8>& bytes, f32 value) {
    size_t old_size = bytes.size();
    bytes.reserve(4);
    ((f32*)bytes.data())[old_size] = value;
}

u32 read_u32(const u8* bytes) {
    return (bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3];
}

u8 read_u8(const u8* bytes) {
    return bytes[0];
}

u16 read_u16(const u8* bytes) {
    return (bytes[0] << 8) | bytes[1];
}

i32 read_i32(const u8* bytes) {
    return (bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3];
}

i16 read_i16(const u8* bytes) {
    return (bytes[0] << 8) | bytes[1];
}

f32 read_f32(const u8* bytes) {
    return *(f32*)bytes;
}

u8 unpack_opcode(Instruction ins) {
    return ins >> 24;
}