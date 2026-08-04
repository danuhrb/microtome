#pragma once
#include <cstddef>
#include <cstdint>

inline uint16_t bswap16(uint16_t value) { return uint16_t(( value >> 8 ) | ( value << 8 )); };

inline uint32_t bswap32(uint32_t value) {
return ((value & 0x000000FFu) << 24) | ((value & 0x0000FF00u) << 8) |
       ((value & 0x00FF0000u) >> 8)  | ((value & 0xFF000000u) >> 24);
};

inline uint64_t bswap64(uint64_t value) {
return (uint64_t(bswap32(uint32_t(value))) << 32) | bswap32(uint32_t(value >> 32));
};