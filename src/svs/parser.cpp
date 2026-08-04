#include <cctype>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>
#include "svs.h"
#include "../tiff/tiff.h"

// SVS is a TIFF/BigTIFF container, so parsing reads the 8- or 16-byte header
// and then walks the linked list of Image File Directories. Each IFD is
// classified as a tiled pyramid level or a stripped associated image.

namespace svs {
namespace {

enum Tag : uint16_t {
    kImageWidth = 256,
    kImageLength = 257,
    kBitsPerSample = 258,
    kCompression = 259,
    kPhotometric = 262,
    kImageDescription = 270,
    kStripOffsets = 273,
    kSamplesPerPixel = 277,
    kTileWidth = 322,
    kTileLength = 323,
    kTileOffsets = 324,
    kTileByteCounts = 325,
    kJpegTables = 347,
};

// Returns 0 for field types this parser does not know.
size_t tiffTypeSize(uint16_t type) {
    switch (type) {
        case 1: case 2: case 6: case 7:            return 1; // BYTE ASCII SBYTE UNDEFINED
        case 3: case 8:                            return 2; // SHORT SSHORT
        case 4: case 9: case 11: case 13:          return 4; // LONG SLONG FLOAT IFD
        case 5: case 10: case 12: case 16:
        case 17: case 18:                          return 8; // RATIONAL DOUBLE LONG8 ...
        default:                                   return 0;
    }
}

bool hostIsLittleEndian() {
    uint16_t x = 1;
    return *reinterpret_cast<uint8_t*>(&x) == 1;
}

// Byte-swaps when the file's endianness differs from the host's.
struct Reader {
    const RandomAccessFile* file = nullptr;
    bool needSwap = false;

    bool readBytes(uint64_t offset, void* dst, size_t n) {
        return file && file->read(offset, dst, n);
    }

    bool u16(uint64_t offset, uint16_t& out) {
        uint16_t v;
        if (!readBytes(offset, &v, 2)) return false;
        out = needSwap ? bswap16(v) : v;
        return true;
    }
    bool u32(uint64_t offset, uint32_t& out) {
        uint32_t v;
        if (!readBytes(offset, &v, 4)) return false;
        out = needSwap ? bswap32(v) : v;
        return true;
    }
    bool u64(uint64_t offset, uint64_t& out) {
        uint64_t v;
        if (!readBytes(offset, &v, 8)) return false;
        out = needSwap ? bswap64(v) : v;
        return true;
    }
};

struct Entry {
    uint16_t tag = 0;
    uint16_t type = 0;
    uint64_t count = 0;
    uint64_t valueFieldOffset = 0;
};

// Bytes a value field holds inline before it instead becomes an offset.
uint64_t inlineCapacity(bool bigTiff) { return bigTiff ? 8 : 4; }

// Small values sit in the value field itself; larger ones live at the offset it
// points to.
bool dataOffset(Reader& r, const Entry& e, bool bigTiff, uint64_t byteCount,
                uint64_t& out) {
    if (byteCount <= inlineCapacity(bigTiff)) {
        out = e.valueFieldOffset;
        return true;
    }
    if (bigTiff) return r.u64(e.valueFieldOffset, out);
    uint32_t o;
    if (!r.u32(e.valueFieldOffset, o)) return false;
    out = o;
    return true;
}

// Integer-typed entries only: SHORT, LONG, LONG8.
bool readUints(Reader& r, const Entry& e, bool bigTiff,
               std::vector<uint64_t>& out) {
    size_t ts = tiffTypeSize(e.type);
    if (ts == 0) return false;
    uint64_t off;
    if (!dataOffset(r, e, bigTiff, e.count * ts, off)) return false;

    out.resize(static_cast<size_t>(e.count));
    for (uint64_t i = 0; i < e.count; ++i) {
        uint64_t at = off + i * ts;
        switch (e.type) {
            case 3: { uint16_t v; if (!r.u16(at, v)) return false; out[i] = v; break; }
            case 4: case 13: { uint32_t v; if (!r.u32(at, v)) return false; out[i] = v; break; }
            case 16: { uint64_t v; if (!r.u64(at, v)) return false; out[i] = v; break; }
            default: return false;
        }
    }
    return true;
}

// Trims the trailing NUL that TIFF includes in the count.
bool readAscii(Reader& r, const Entry& e, bool bigTiff, std::string& out) {
    uint64_t off;
    if (!dataOffset(r, e, bigTiff, e.count, off)) return false;
    std::string s(static_cast<size_t>(e.count), '\0');
    if (e.count && !r.readBytes(off, s.data(), static_cast<size_t>(e.count)))
        return false;
    size_t z = s.find('\0');
    if (z != std::string::npos) s.resize(z);
    out = std::move(s);
    return true;
}

bool readRaw(Reader& r, const Entry& e, bool bigTiff,
             std::vector<uint8_t>& out) {
    uint64_t off;
    if (!dataOffset(r, e, bigTiff, e.count, off)) return false;
    out.resize(static_cast<size_t>(e.count));
    if (e.count && !r.readBytes(off, out.data(), static_cast<size_t>(e.count))) {
        out.clear();
        return false;
    }
    return true;
}

// Appends the resulting level or associated image to the slide. Reports the
// next IFD offset, which is 0 at the end of the chain.
bool parseIfd(Reader& r, Slide& slide, uint64_t ifdOffset,
              uint64_t& nextIfdOffset) {
    const bool big = slide.bigTiff;
    const uint64_t entrySize = big ? 20 : 12;

    uint64_t count;
    uint64_t entriesStart;
    if (big) {
        if (!r.u64(ifdOffset, count)) return false;
        entriesStart = ifdOffset + 8;
    } else {
        uint16_t c;
        if (!r.u16(ifdOffset, c)) return false;
        count = c;
        entriesStart = ifdOffset + 2;
    }

    uint32_t width = 0, height = 0, tileW = 0, tileH = 0;
    uint16_t compression = 0;
    uint16_t photometric = 0;
    uint16_t samplesPerPixel = 0;
    uint16_t bitsPerSample = 0;
    bool tiled = false;
    std::vector<uint64_t> tileOffsets, tileByteCounts, stripOffsets;
    std::vector<uint8_t> jpegTables;
    std::string desc;

    for (uint64_t i = 0; i < count; ++i) {
        uint64_t eoff = entriesStart + i * entrySize;
        Entry e;
        if (!r.u16(eoff, e.tag)) return false;
        if (!r.u16(eoff + 2, e.type)) return false;
        if (big) {
            if (!r.u64(eoff + 4, e.count)) return false;
            e.valueFieldOffset = eoff + 12;
        } else {
            uint32_t cc;
            if (!r.u32(eoff + 4, cc)) return false;
            e.count = cc;
            e.valueFieldOffset = eoff + 8;
        }

        std::vector<uint64_t> vals;
        switch (e.tag) {
            case kImageWidth:
                if (readUints(r, e, big, vals) && !vals.empty())
                    width = static_cast<uint32_t>(vals[0]);
                break;
            case kImageLength:
                if (readUints(r, e, big, vals) && !vals.empty())
                    height = static_cast<uint32_t>(vals[0]);
                break;
            case kCompression:
                if (readUints(r, e, big, vals) && !vals.empty())
                    compression = static_cast<uint16_t>(vals[0]);
                break;
            case kPhotometric:
                if (readUints(r, e, big, vals) && !vals.empty())
                    photometric = static_cast<uint16_t>(vals[0]);
                break;
            case kSamplesPerPixel:
                if (readUints(r, e, big, vals) && !vals.empty())
                    samplesPerPixel = static_cast<uint16_t>(vals[0]);
                break;
            case kBitsPerSample:
                // One value per sample; they are equal for the formats we read.
                if (readUints(r, e, big, vals) && !vals.empty())
                    bitsPerSample = static_cast<uint16_t>(vals[0]);
                break;
            case kJpegTables:
                readRaw(r, e, big, jpegTables);
                break;
            case kImageDescription:
                readAscii(r, e, big, desc);
                break;
            case kTileWidth:
                if (readUints(r, e, big, vals) && !vals.empty()) {
                    tileW = static_cast<uint32_t>(vals[0]);
                    tiled = true;
                }
                break;
            case kTileLength:
                if (readUints(r, e, big, vals) && !vals.empty())
                    tileH = static_cast<uint32_t>(vals[0]);
                break;
            case kTileOffsets:
                if (readUints(r, e, big, tileOffsets)) tiled = true;
                break;
            case kTileByteCounts:
                readUints(r, e, big, tileByteCounts);
                break;
            case kStripOffsets:
                readUints(r, e, big, stripOffsets);
                break;
            default:
                break;
        }
    }

    uint64_t afterEntries = entriesStart + count * entrySize;
    if (big) {
        if (!r.u64(afterEntries, nextIfdOffset)) return false;
    } else {
        uint32_t n;
        if (!r.u32(afterEntries, n)) return false;
        nextIfdOffset = n;
    }

    if (tiled && !tileOffsets.empty()) {
        Level lvl;
        lvl.width = width;
        lvl.height = height;
        lvl.tileWidth = tileW;
        lvl.tileHeight = tileH;
        lvl.compression = compression;
        lvl.photometric = photometric;
        lvl.samplesPerPixel = samplesPerPixel;
        lvl.bitsPerSample = bitsPerSample;
        lvl.tileOffsets = std::move(tileOffsets);
        lvl.tileSizes = std::move(tileByteCounts);
        lvl.jpegTables = std::move(jpegTables);
        slide.levels.push_back(std::move(lvl));
        // Only level 0 carries the Aperio metadata.
        if (slide.levels.size() == 1 && !desc.empty())
            slide.imageDescription = desc;
    } else {
        Associated a;
        a.width = width;
        a.height = height;
        if (!stripOffsets.empty()) a.offset = stripOffsets[0];
        std::string low;
        low.reserve(desc.size());
        for (char c : desc)
            low.push_back(static_cast<char>(std::tolower(static_cast<unsigned char>(c))));
        if (low.find("label") != std::string::npos) a.name = "label";
        else if (low.find("macro") != std::string::npos) a.name = "macro";
        else a.name = "thumbnail";
        slide.associated.push_back(std::move(a));
    }
    return true;
}

} // namespace

bool openSlide(const std::string& path, Slide& out) {
    out = Slide{};
    out.path = path;

    auto file = std::make_shared<RandomAccessFile>();
    if (!file->open(path)) return false;

    Reader r;
    r.file = file.get();

    uint8_t bom[2];
    if (!r.readBytes(0, bom, 2)) return false;
    if (bom[0] == 'I' && bom[1] == 'I') out.littleEndian = true;
    else if (bom[0] == 'M' && bom[1] == 'M') out.littleEndian = false;
    else return false;
    r.needSwap = (out.littleEndian != hostIsLittleEndian());

    uint16_t magic;
    if (!r.u16(2, magic)) return false;

    uint64_t firstIfd = 0;
    if (magic == 42) {
        out.bigTiff = false;
        uint32_t off;
        if (!r.u32(4, off)) return false;
        firstIfd = off;
    } else if (magic == 43) {
        out.bigTiff = true;
        uint16_t offsetByteSize, reserved;
        if (!r.u16(4, offsetByteSize)) return false;
        if (!r.u16(6, reserved)) return false;
        if (offsetByteSize != 8 || reserved != 0) return false;
        if (!r.u64(8, firstIfd)) return false;
    } else {
        return false;
    }

    uint64_t ifdOffset = firstIfd;
    size_t guard = 0;
    while (ifdOffset != 0 && guard++ < 4096) {
        uint64_t next = 0;
        if (!parseIfd(r, out, ifdOffset, next)) return false;
        if (next == ifdOffset) break; // malformed self-referential chain
        ifdOffset = next;
    }

    if (!out.levels.empty()) {
        double w0 = static_cast<double>(out.levels[0].width);
        for (auto& lvl : out.levels)
            lvl.downsample = lvl.width > 0 ? w0 / static_cast<double>(lvl.width) : 1.0;
    }

    if (!out.imageDescription.empty())
        parseImageDescription(out.imageDescription, out);

    if (out.levels.empty()) return false;

    out.file = std::move(file);
    return true;
}

bool parseImageDescription(const std::string& text, Slide& out) {
    // Aperio packs "key = value" fields separated by '|'.
    auto field = [&](const char* key, double& dst) {
        size_t p = text.find(key);
        if (p == std::string::npos) return;
        p = text.find('=', p);
        if (p == std::string::npos) return;
        ++p;
        while (p < text.size() && (text[p] == ' ' || text[p] == '\t')) ++p;
        size_t end = p;
        while (end < text.size() && text[end] != '|' &&
               text[end] != '\r' && text[end] != '\n')
            ++end;
        try {
            dst = std::stod(text.substr(p, end - p));
        } catch (...) {
        }
    };
    field("MPP", out.mpp);
    field("AppMag", out.magnification);
    return true;
}

bool readTile(const Slide& slide, size_t level, size_t tileIndex,
              std::vector<uint8_t>& dst) {
    dst.clear();
    if (!slide.file || !slide.file->isOpen()) return false;
    if (level >= slide.levels.size()) return false;

    const Level& lvl = slide.levels[level];
    if (tileIndex >= lvl.tileOffsets.size() ||
        tileIndex >= lvl.tileSizes.size())
        return false;

    uint64_t offset = lvl.tileOffsets[tileIndex];
    uint64_t bytes = lvl.tileSizes[tileIndex];
    // Aperio omits blank tiles by giving them a zero byte count.
    if (bytes == 0) return true;

    // A corrupt directory could point past the end of the file.
    uint64_t fileSize = slide.file->size();
    if (fileSize && offset + bytes > fileSize) return false;

    dst.resize(static_cast<size_t>(bytes));
    if (!slide.file->read(offset, dst.data(), static_cast<size_t>(bytes))) {
        dst.clear();
        return false;
    }
    return true;
}

bool readTile(const Slide& slide, size_t level, uint32_t col, uint32_t row,
              std::vector<uint8_t>& dst) {
    dst.clear();
    if (level >= slide.levels.size()) return false;
    const Level& lvl = slide.levels[level];
    if (col >= tilesAcross(lvl) || row >= tilesDown(lvl)) return false;
    return readTile(slide, level, tileIndexAt(lvl, col, row), dst);
}

} // namespace svs
