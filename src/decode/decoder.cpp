#include "decoder.h"

#include <cstring>

namespace decode {
namespace {

constexpr uint8_t kMarker = 0xFF;
constexpr uint8_t kSoi = 0xD8; // start of image
constexpr uint8_t kEoi = 0xD9; // end of image

bool startsWithSoi(const uint8_t* p, size_t n) {
    return n >= 2 && p[0] == kMarker && p[1] == kSoi;
}

// Edge tiles are padded to a full tile in the file, so the source always holds
// tileWidth * tileHeight pixels.
Status decodeRaw(const svs::Level& level, const uint8_t* bytes, size_t n,
                 const Surface& dst) {
    if (level.bitsPerSample != 0 && level.bitsPerSample != 8)
        return Status::UnsupportedCodec;

    uint32_t samples =
        level.samplesPerPixel ? level.samplesPerPixel : dst.channels;
    if (samples != dst.channels) return Status::BadArgument;

    uint32_t w = level.tileWidth;
    uint32_t h = level.tileHeight;
    if (w == 0 || h == 0) return Status::BadArgument;
    if (w > dst.width || h > dst.height) return Status::BadArgument;

    size_t srcRow = static_cast<size_t>(w) * samples;
    if (n < srcRow * h) return Status::BadData;

    for (uint32_t y = 0; y < h; ++y)
        std::memcpy(dst.data + static_cast<size_t>(y) * dst.stride,
                    bytes + static_cast<size_t>(y) * srcRow, srcRow);
    return Status::Ok;
}

} // namespace

const char* statusName(Status s) {
    switch (s) {
        case Status::Ok: return "ok";
        case Status::BadArgument: return "bad argument";
        case Status::UnsupportedCodec: return "unsupported codec";
        case Status::NotBuilt: return "codec not built in";
        case Status::BadData: return "bad data";
    }
    return "unknown";
}

bool buildJpegStream(const std::vector<uint8_t>& tables, const uint8_t* tile,
                     size_t tileSize, std::vector<uint8_t>& out) {
    out.clear();
    if (!tile || tileSize < 2) return false;

    const bool tileHasSoi = startsWithSoi(tile, tileSize);

    if (tables.empty()) {
        if (!tileHasSoi) return false;
        out.assign(tile, tile + tileSize);
        return true;
    }

    if (!startsWithSoi(tables.data(), tables.size())) return false;

    // Drop the tables stream's terminating EOI so the tile's scan can follow it.
    size_t tablesEnd = tables.size();
    if (tablesEnd >= 4 && tables[tablesEnd - 2] == kMarker &&
        tables[tablesEnd - 1] == kEoi)
        tablesEnd -= 2;

    // The tables already supplied the SOI, so skip the tile's own.
    size_t tileStart = tileHasSoi ? 2 : 0;
    if (tileStart >= tileSize) return false;

    out.reserve(tablesEnd + (tileSize - tileStart));
    out.insert(out.end(), tables.begin(),
               tables.begin() + static_cast<std::ptrdiff_t>(tablesEnd));
    out.insert(out.end(), tile + tileStart, tile + tileSize);
    return true;
}

Status decodeTile(const svs::Level& level, const uint8_t* bytes, size_t n,
                  const Surface& dst) {
    if (!bytes || n == 0) return Status::BadArgument;
    if (!dst.data || dst.channels == 0) return Status::BadArgument;
    if (dst.width == 0 || dst.height == 0) return Status::BadArgument;
    if (dst.stride < static_cast<size_t>(dst.width) * dst.channels)
        return Status::BadArgument;

    switch (level.compression) {
        case svs::kCompressionNone:
            return decodeRaw(level, bytes, n, dst);

        case svs::kCompressionJpeg: {
            std::vector<uint8_t> stream;
            if (!buildJpegStream(level.jpegTables, bytes, n, stream))
                return Status::BadData;
            return detail::decodeJpeg(stream.data(), stream.size(),
                                      level.photometric, dst);
        }

        // JPEG 2000 needs OpenJPEG, which is not wired up yet.
        case svs::kCompressionJp2kYCbCr:
        case svs::kCompressionJp2kRgb:
        default:
            return Status::UnsupportedCodec;
    }
}

} // namespace decode
