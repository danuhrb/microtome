#pragma once
#include <cstddef>
#include <cstdint>
#include <vector>
#include "../svs/svs.h"

// Tiles decode into a caller-supplied surface rather than a fresh buffer, so a
// tile can land directly in its final place inside a larger output image.

namespace decode {

enum class Status {
    Ok,
    BadArgument,      // unusable destination, or input that cannot fit it
    UnsupportedCodec, // a compression scheme this build does not handle
    NotBuilt,         // the codec is known but its library was not compiled in
    BadData,          // the codec rejected the stream
};

const char* statusName(Status s);

// A stride wider than one row lets a tile be written into a window of a bigger
// buffer without an extra copy.
struct Surface {
    uint8_t* data = nullptr;
    size_t stride = 0; // bytes between the start of consecutive rows
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t channels = 3; // bytes per pixel, 8 bits per channel
};

// SVS stores the quantization and Huffman tables once per level, so a tile on
// its own is not a decodable JPEG. Splicing the two yields a complete
// datastream. A level with no tables has self-contained tiles, which pass
// through unchanged.
bool buildJpegStream(const std::vector<uint8_t>& tables, const uint8_t* tile,
                     size_t tileSize, std::vector<uint8_t>& out);

Status decodeTile(const svs::Level& level, const uint8_t* bytes, size_t n,
                  const Surface& dst);

namespace detail {
// Backed by libjpeg-turbo when MICROTOME_HAVE_JPEG is defined, otherwise
// reports Status::NotBuilt.
Status decodeJpeg(const uint8_t* stream, size_t n, uint16_t photometric,
                  const Surface& dst);
} // namespace detail

} // namespace decode
