#include "coalesce.h"

#include <algorithm>
#include <limits>

namespace sched {

CoalesceLimits limitsFor(Storage storage) {
    switch (storage) {
        case Storage::LocalDisk:
            return {64ull * 1024, 8ull << 20};
        case Storage::NetworkFs:
            return {1ull << 20, 16ull << 20};
        case Storage::ObjectStore:
            return {4ull << 20, 32ull << 20};
    }
    return {};
}

std::vector<Run> coalesceRuns(std::span<const TileRequest> byOffset,
                              const CoalesceLimits& limits) {
    std::vector<Run> runs;
    if (byOffset.empty()) return runs;

    Run current{byOffset[0].offset, byOffset[0].offset + byOffset[0].size, 0, 1};
    for (size_t i = 1; i < byOffset.size(); ++i) {
        const TileRequest& r = byOffset[i];
        const uint64_t end = r.offset + r.size;
        // Input is ordered, so the next request never starts before the run
        // does, but two requests may overlap. An overlap is not a gap.
        const uint64_t gap = r.offset > current.end ? r.offset - current.end : 0;
        const uint64_t grown = std::max(current.end, end) - current.start;

        if (gap <= limits.maxGap && grown <= limits.maxRun) {
            current.end = std::max(current.end, end);
            ++current.count;
            continue;
        }

        runs.push_back(current);
        // A first member is admitted whatever its size, so a tile larger than
        // maxRun still gets read whole rather than truncated.
        current = Run{r.offset, end, i, 1};
    }
    runs.push_back(current);
    return runs;
}

Plan planTileFetch(const svs::Level& level, std::span<const size_t> wanted,
                   const CoalesceLimits& limits, uint64_t fileSize) {
    Plan plan;

    // Sorting first makes the duplicates adjacent, and the caller's order is of
    // no use here: a request is identified by its tile index.
    std::vector<size_t> unique(wanted.begin(), wanted.end());
    std::sort(unique.begin(), unique.end());
    unique.erase(std::unique(unique.begin(), unique.end()), unique.end());

    // A directory that disagrees with itself about how many tiles it has is only
    // trustworthy up to the shorter of the two arrays.
    const size_t tiles =
        std::min(level.tileOffsets.size(), level.tileSizes.size());

    plan.requests.reserve(unique.size());
    for (size_t index : unique) {
        if (index >= tiles) {
            plan.missing.push_back(index);
            continue;
        }
        const uint64_t size = level.tileSizes[index];
        if (size == 0) {
            plan.blank.push_back(index);
            continue;
        }
        const uint64_t offset = level.tileOffsets[index];
        if (offset > std::numeric_limits<uint64_t>::max() - size ||
            (fileSize != 0 && offset + size > fileSize)) {
            plan.missing.push_back(index);
            continue;
        }
        plan.requests.push_back(TileRequest{index, offset, size});
        plan.wantedBytes += size;
    }

    // Grid order is not offset order. Aperio usually writes tiles in order, but
    // nothing in TIFF requires it, so the directory decides the layout.
    std::sort(plan.requests.begin(), plan.requests.end(),
              [](const TileRequest& a, const TileRequest& b) {
                  if (a.offset != b.offset) return a.offset < b.offset;
                  return a.size < b.size;
              });

    plan.runs = coalesceRuns(plan.requests, limits);
    for (const Run& run : plan.runs) plan.fetchedBytes += run.length();
    return plan;
}

} // namespace sched
