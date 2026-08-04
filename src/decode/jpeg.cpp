#include "decoder.h"

// The libjpeg-turbo backend. This is the only translation unit that needs the
// library, so the rest of the decoder still builds without it.

#ifdef MICROTOME_HAVE_JPEG

#include <csetjmp>
#include <cstdio> // jpeglib.h expects FILE to be declared

extern "C" {
#include <jpeglib.h>
}

namespace decode {
namespace detail {
namespace {

// libjpeg reports fatal errors through error_exit, which must not return. The
// default handler calls exit(), so decoding a corrupt tile would kill the
// process; jump back to the caller instead.
struct ErrorTrap {
    jpeg_error_mgr mgr;
    jmp_buf escape;
};

void onFatal(j_common_ptr cinfo) {
    auto* trap = reinterpret_cast<ErrorTrap*>(cinfo->err);
    std::longjmp(trap->escape, 1);
}

void onMessage(j_common_ptr) {} // keep libjpeg from writing to stderr

} // namespace

Status decodeJpeg(const uint8_t* stream, size_t n, uint16_t photometric,
                  const Surface& dst) {
    jpeg_decompress_struct cinfo;
    ErrorTrap trap;

    cinfo.err = jpeg_std_error(&trap.mgr);
    trap.mgr.error_exit = onFatal;
    trap.mgr.output_message = onMessage;

    // No non-trivial locals are live here, so unwinding by longjmp is safe.
    if (setjmp(trap.escape)) {
        jpeg_destroy_decompress(&cinfo);
        return Status::BadData;
    }

    jpeg_create_decompress(&cinfo);
    jpeg_mem_src(&cinfo, stream, static_cast<unsigned long>(n));

    if (jpeg_read_header(&cinfo, TRUE) != JPEG_HEADER_OK) {
        jpeg_destroy_decompress(&cinfo);
        return Status::BadData;
    }

    // Aperio records the colour space in the TIFF tag, and its streams often
    // lack the markers libjpeg would otherwise use to guess. jpeg_read_header
    // resets these fields, so they must be set after it.
    if (photometric == svs::kPhotometricRgb && cinfo.num_components == 3)
        cinfo.jpeg_color_space = JCS_RGB;
    cinfo.out_color_space = cinfo.num_components == 1 ? JCS_GRAYSCALE : JCS_RGB;

    jpeg_start_decompress(&cinfo);

    if (cinfo.output_width > dst.width || cinfo.output_height > dst.height ||
        static_cast<uint32_t>(cinfo.output_components) != dst.channels) {
        jpeg_abort_decompress(&cinfo);
        jpeg_destroy_decompress(&cinfo);
        return Status::BadArgument;
    }

    // Decoding straight into the destination rows avoids an intermediate tile
    // buffer, so the pixels are written once.
    while (cinfo.output_scanline < cinfo.output_height) {
        JSAMPROW row =
            dst.data + static_cast<size_t>(cinfo.output_scanline) * dst.stride;
        if (jpeg_read_scanlines(&cinfo, &row, 1) != 1) {
            jpeg_abort_decompress(&cinfo);
            jpeg_destroy_decompress(&cinfo);
            return Status::BadData;
        }
    }

    jpeg_finish_decompress(&cinfo);
    jpeg_destroy_decompress(&cinfo);
    return Status::Ok;
}

} // namespace detail
} // namespace decode

#else // MICROTOME_HAVE_JPEG

namespace decode {
namespace detail {

Status decodeJpeg(const uint8_t*, size_t, uint16_t, const Surface&) {
    return Status::NotBuilt;
}

} // namespace detail
} // namespace decode

#endif
