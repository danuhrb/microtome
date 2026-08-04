#pragma once
#include <cstddef>
#include <cstdint>
#include <span>
#include <vector>
#include "../svs/svs.h"

// Planning happens before any tile is read, because the grouping that matters
// can only be seen with the whole tile list in hand. The cost of a read is
// dominated by its latency rather than its length: forty tiles of 40 KB that
// happen to lie next to each other in the file are one 1.6 MB read, and on
// object storage that is one range request instead of forty round trips.
//
// This is grouping by file locality, not by task count, so it belongs above the
// thread pool. The pool then receives one task per run.

namespace sched {

// One tile's byte range in the file. A tile whose byte count is zero never
// becomes a request; see Plan::blank.
struct TileRequest {
    size_t tileIndex = 0;
    uint64_t offset = 0;
    uint64_t size = 0;
};

// A single read that covers a group of requests. [start, end) spans every
// member, including any unwanted bytes between them.
struct Run {
    uint64_t start = 0;
    uint64_t end = 0; // exclusive
    size_t first = 0; // index of the first member in Plan::requests
    size_t count = 0;
    uint64_t length() const { return end - start; }
};

struct CoalesceLimits {
    // Unwanted bytes worth swallowing to avoid a second read. Too small and
    // coalescing is lost; too large and bandwidth is spent on bytes nobody
    // asked for.
    uint64_t maxGap = 64ull * 1024;
    // Ceiling on one run. Too large and memory spikes while load balancing
    // grows coarse, since a run is one task.
    uint64_t maxRun = 8ull * 1024 * 1024;
};

// The right limits follow from the latency of the storage, not from the slide.
enum class Storage {
    LocalDisk,   // latency low, bandwidth cheap
    NetworkFs,   // a round trip costs more than a megabyte
    ObjectStore, // one range request is worth several megabytes of slack
};

CoalesceLimits limitsFor(Storage storage);

struct Plan {
    // Sorted by offset and free of duplicate tile indices. Run::first indexes
    // into this.
    std::vector<TileRequest> requests;
    std::vector<Run> runs;
    // Tiles the directory records with a byte count of zero. Aperio omits blank
    // tiles this way, so these need a filled destination and no I/O at all.
    std::vector<size_t> blank;
    // Indices past the end of the directory, or entries whose range cannot be
    // read: a corrupt offset, or one that runs past the end of the file.
    std::vector<size_t> missing;
    uint64_t wantedBytes = 0;  // sum of the request sizes
    uint64_t fetchedBytes = 0; // sum of the run lengths, so gaps included
};

// Gathers, orders and coalesces in one step. Duplicate entries in `wanted`
// collapse into a single request, so a caller that maps two destinations onto
// one tile must fan out from the tile index itself. A non-zero `fileSize`
// rejects entries that would read past the end of the file.
Plan planTileFetch(const svs::Level& level, std::span<const size_t> wanted,
                   const CoalesceLimits& limits, uint64_t fileSize = 0);

// The coalescing step alone. Input must already be sorted by offset.
std::vector<Run> coalesceRuns(std::span<const TileRequest> byOffset,
                              const CoalesceLimits& limits);

inline std::span<const TileRequest> members(const Plan& plan, const Run& run) {
    return std::span<const TileRequest>(plan.requests).subspan(run.first,
                                                               run.count);
}

} // namespace sched
