#pragma once
#include <cstddef>
#include <cstdint>
#include <functional>
#include <span>
#include <vector>
#include "../pool.h"
#include "../svs/svs.h"
#include "coalesce.h"

// The scheduler that sits between a request for tiles and the thread pool. It
// plans the reads, posts one task per run, and hands each tile's bytes to a sink
// as a slice of the run's buffer rather than as a copy.

namespace sched {

// Called once per fetched tile, on whichever worker read the run holding it.
// `bytes` points into that run's buffer and is valid only for the duration of
// the call, so a sink that needs the tile afterwards must decode or copy it.
// Sinks for different tiles run concurrently, and two tiles of the same run run
// one after the other on the same thread.
using TileSink =
    std::function<void(size_t tileIndex, const uint8_t* bytes, size_t size)>;

struct FetchOptions {
    CoalesceLimits limits;
    // Ceiling on the run buffers alive at once. 0 derives it from the run
    // ceiling and the number of runs that can actually be in flight.
    uint64_t memoryBudget = 0;
};

struct FetchResult {
    size_t runs = 0;
    size_t tilesFetched = 0;
    size_t tilesFailed = 0;
    std::vector<size_t> blank;   // in the directory, with a zero byte count
    std::vector<size_t> missing; // absent from the directory, or unreadable
    uint64_t wantedBytes = 0;
    uint64_t fetchedBytes = 0; // includes the gaps coalescing swallowed
    uint64_t peakBytes = 0;    // largest total held in run buffers at once
    // Blank tiles are not failures: the directory says they hold nothing.
    bool complete() const { return tilesFailed == 0 && missing.empty(); }
};

// Blocks until every run has been read and every tile handed to the sink.
//
// Must not be called from a task already running on `pool`. It waits for the
// tasks it posts, and a worker that waits on its own pool can deadlock.
bool fetchTiles(const svs::Slide& slide, size_t level,
                std::span<const size_t> wanted, ThreadPool& pool,
                const FetchOptions& options, const TileSink& sink,
                FetchResult* result = nullptr);

} // namespace sched
