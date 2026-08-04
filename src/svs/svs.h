#pragma once
#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>
#include "../file.h"

// Aperio SVS is a tiled, pyramidal BigTIFF/TIFF file. Each IFD is one image:
// level 0 is full resolution, then downsampled levels, then associated images
// (thumbnail, label, macro). Level metadata (mpp, magnification, etc.) is
// packed as text in the TIFF ImageDescription tag of the first IFD.

namespace svs {

struct Level {
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t tileWidth = 0;
    uint32_t tileHeight = 0;
    uint16_t compression = 0;
    uint16_t photometric = 0;
    uint16_t samplesPerPixel = 0;
    uint16_t bitsPerSample = 0;
    double downsample = 1.0; // relative to level 0
    std::vector<uint64_t> tileOffsets;
    std::vector<uint64_t> tileSizes; // bytes, not pixels
    // Quantization and Huffman tables shared by every JPEG tile in this level.
    // TIFF stores them once here instead of in each tile (TIFF Technical Note 2).
    std::vector<uint8_t> jpegTables;
};

enum Compression : uint16_t {
    kCompressionNone = 1,
    kCompressionJpeg = 7,
    kCompressionJp2kYCbCr = 33003, // Aperio-specific JPEG 2000
    kCompressionJp2kRgb = 33005,   // Aperio-specific JPEG 2000
};

enum Photometric : uint16_t {
    kPhotometricRgb = 2,
    kPhotometricYCbCr = 6,
};

// Tiles are stored row-major, and a partial tile at the right or bottom edge
// still occupies a full tile slot.
inline uint32_t tilesAcross(const Level& l) {
    return l.tileWidth ? (l.width + l.tileWidth - 1) / l.tileWidth : 0;
}
inline uint32_t tilesDown(const Level& l) {
    return l.tileHeight ? (l.height + l.tileHeight - 1) / l.tileHeight : 0;
}
inline size_t tileIndexAt(const Level& l, uint32_t col, uint32_t row) {
    return static_cast<size_t>(row) * tilesAcross(l) + col;
}

struct Associated {
    std::string name; // "thumbnail", "label", "macro"
    uint32_t width = 0;
    uint32_t height = 0;
    uint64_t offset = 0;
};

struct Slide {
    std::string path;
    bool bigTiff = false;
    bool littleEndian = true;
    double mpp = 0.0; // microns per pixel at level 0
    double magnification = 0.0;
    std::vector<Level> levels;
    std::vector<Associated> associated;
    std::string imageDescription;
    // One handle shared by every reader of this slide. Held by shared_ptr so
    // copying a Slide shares the handle instead of reopening the file.
    std::shared_ptr<RandomAccessFile> file;
};

// Walks the IFD chain to populate the levels and associated images.
bool openSlide(const std::string& path, Slide& out);

// Extracts mpp and magnification from an Aperio ImageDescription.
bool parseImageDescription(const std::string& text, Slide& out);

// Thread-safe: many threads may read different tiles of the same slide at the
// same time. A tile with a byte count of zero is absent from the file and
// yields an empty dst.
bool readTile(const Slide& slide, size_t level, size_t tileIndex,
              std::vector<uint8_t>& dst);

bool readTile(const Slide& slide, size_t level, uint32_t col, uint32_t row,
              std::vector<uint8_t>& dst);

} // namespace svs
