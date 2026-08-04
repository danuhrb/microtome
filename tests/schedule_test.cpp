// Tests for the coalescing scheduler. The build system does not wire this up
// yet, so build it directly:
//
//   g++ -std=c++23 -O2 -pthread -o schedule_test tests/schedule_test.cpp
//       src/schedule/coalesce.cpp src/schedule/buffers.cpp
//       src/schedule/fetch.cpp src/pool.cpp src/file.cpp
//
// Nothing here needs a decoder, so the sink checks bytes instead of pixels.

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <mutex>
#include <numeric>
#include <string>
#include <thread>
#include <vector>

#include "../src/pool.h"
#include "../src/schedule/buffers.h"
#include "../src/schedule/coalesce.h"
#include "../src/schedule/fetch.h"

namespace {

int failures = 0;

void check(bool ok, const std::string& what) {
    if (!ok) {
        ++failures;
        std::printf("  FAIL  %s\n", what.c_str());
    }
}

void checkEq(uint64_t got, uint64_t want, const std::string& what) {
    if (got != want) {
        ++failures;
        std::printf("  FAIL  %s: got %llu, want %llu\n", what.c_str(),
                    static_cast<unsigned long long>(got),
                    static_cast<unsigned long long>(want));
    }
}

svs::Level levelWith(std::vector<uint64_t> offsets, std::vector<uint64_t> sizes) {
    svs::Level level;
    level.width = 1024;
    level.height = 1024;
    level.tileWidth = 256;
    level.tileHeight = 256;
    level.compression = svs::kCompressionNone;
    level.tileOffsets = std::move(offsets);
    level.tileSizes = std::move(sizes);
    return level;
}

std::vector<size_t> iota(size_t n) {
    std::vector<size_t> v(n);
    std::iota(v.begin(), v.end(), size_t{0});
    return v;
}

// Any byte of the fixture file is a function of its position, so a tile's
// expected content follows from its offset alone.
uint8_t byteAt(uint64_t position) {
    uint64_t x = position * 0x9E3779B97F4A7C15ull;
    x ^= x >> 29;
    return static_cast<uint8_t>(x >> 17);
}

void testContiguousBecomesOneRun() {
    // Four tiles laid end to end, the layout Aperio usually writes.
    auto level = levelWith({1000, 2000, 3000, 4000}, {1000, 1000, 1000, 1000});
    auto plan = sched::planTileFetch(level, iota(4), sched::CoalesceLimits{});

    checkEq(plan.runs.size(), 1, "contiguous tiles coalesce into one run");
    checkEq(plan.runs[0].start, 1000, "run starts at the first tile");
    checkEq(plan.runs[0].length(), 4000, "run spans every member");
    checkEq(plan.runs[0].count, 4, "run holds all four members");
    checkEq(plan.fetchedBytes, plan.wantedBytes, "no wasted bytes");
}

void testGapWithinBudgetIsSwallowed() {
    auto level = levelWith({0, 40960}, {1024, 1024}); // 39 KB hole
    sched::CoalesceLimits limits{64ull * 1024, 8ull << 20};
    auto plan = sched::planTileFetch(level, iota(2), limits);

    checkEq(plan.runs.size(), 1, "a gap under maxGap does not split the run");
    checkEq(plan.wantedBytes, 2048, "wanted counts only the tiles");
    checkEq(plan.fetchedBytes, 40960 + 1024, "fetched counts the hole too");
}

void testGapOverBudgetSplits() {
    auto level = levelWith({0, 1ull << 20}, {1024, 1024});
    sched::CoalesceLimits limits{64ull * 1024, 8ull << 20};
    auto plan = sched::planTileFetch(level, iota(2), limits);

    checkEq(plan.runs.size(), 2, "a gap over maxGap splits the run");
    checkEq(plan.fetchedBytes, 2048, "split runs fetch only the tiles");
}

void testMaxRunCapsARun() {
    // Ten contiguous tiles of 1 MB against a 4 MB ceiling.
    std::vector<uint64_t> offsets, sizes;
    for (uint64_t i = 0; i < 10; ++i) {
        offsets.push_back(i * (1ull << 20));
        sizes.push_back(1ull << 20);
    }
    auto level = levelWith(offsets, sizes);
    sched::CoalesceLimits limits{64ull * 1024, 4ull << 20};
    auto plan = sched::planTileFetch(level, iota(10), limits);

    checkEq(plan.runs.size(), 3, "runs break at the ceiling: 4 + 4 + 2");
    for (const auto& run : plan.runs)
        check(run.length() <= (4ull << 20), "no run exceeds maxRun");
    checkEq(plan.fetchedBytes, 10ull << 20, "every wanted byte is still fetched");
}

void testOversizedTileIsNotTruncated() {
    auto level = levelWith({0}, {32ull << 20});
    sched::CoalesceLimits limits{64ull * 1024, 8ull << 20};
    auto plan = sched::planTileFetch(level, iota(1), limits);

    checkEq(plan.runs.size(), 1, "one tile is one run");
    checkEq(plan.runs[0].length(), 32ull << 20,
            "a tile larger than maxRun is read whole");
}

void testOffsetOrderNotGridOrder() {
    // A directory free to store tiles in any order: grid order 0,1,2,3 sits at
    // descending offsets.
    auto level = levelWith({3000, 2000, 1000, 0}, {1000, 1000, 1000, 1000});
    auto plan = sched::planTileFetch(level, iota(4), sched::CoalesceLimits{});

    checkEq(plan.runs.size(), 1, "reversed layout still coalesces");
    checkEq(plan.runs[0].start, 0, "run starts at the lowest offset");
    check(std::is_sorted(plan.requests.begin(), plan.requests.end(),
                         [](const auto& a, const auto& b) {
                             return a.offset < b.offset;
                         }),
          "requests are ordered by offset");
    checkEq(plan.requests[0].tileIndex, 3, "lowest offset is the last tile");
}

void testDuplicatesCollapse() {
    auto level = levelWith({0, 1000}, {1000, 1000});
    std::vector<size_t> wanted{1, 0, 1, 1, 0};
    auto plan = sched::planTileFetch(level, wanted, sched::CoalesceLimits{});

    checkEq(plan.requests.size(), 2, "a repeated tile is fetched once");
    checkEq(plan.runs.size(), 1, "the pair still coalesces");
}

void testBlankAndMissingTiles() {
    auto level = levelWith({0, 0, 2000}, {1000, 0, 1000});
    std::vector<size_t> wanted{0, 1, 2, 99};
    auto plan = sched::planTileFetch(level, wanted, sched::CoalesceLimits{});

    checkEq(plan.blank.size(), 1, "a zero byte count is a blank tile");
    checkEq(plan.blank[0], 1, "the right tile is blank");
    checkEq(plan.missing.size(), 1, "an index past the directory is missing");
    checkEq(plan.missing[0], 99, "the right tile is missing");
    checkEq(plan.requests.size(), 2, "blank and missing tiles cost no I/O");
}

void testFileSizeGuard() {
    auto level = levelWith({0, 9000}, {1000, 1000});
    auto plan = sched::planTileFetch(level, iota(2), sched::CoalesceLimits{}, 9500);

    checkEq(plan.requests.size(), 1, "an entry running past the end is dropped");
    checkEq(plan.missing.size(), 1, "and reported as missing");
    checkEq(plan.missing[0], 1, "the right tile is missing");
}

void testOverlappingRanges() {
    // A malformed directory can point two tiles at overlapping bytes. An overlap
    // must not be read as a negative gap that wraps around.
    auto level = levelWith({0, 500}, {1000, 1000});
    auto plan = sched::planTileFetch(level, iota(2), sched::CoalesceLimits{});

    checkEq(plan.runs.size(), 1, "overlapping tiles coalesce");
    checkEq(plan.runs[0].length(), 1500, "the run spans the union");
}

void testEmptyRequest() {
    auto level = levelWith({0}, {1000});
    auto plan = sched::planTileFetch(level, {}, sched::CoalesceLimits{});
    checkEq(plan.runs.size(), 0, "asking for nothing plans nothing");
}

void testBufferPoolRecyclesBlocks() {
    sched::BufferPool pool(4ull << 20, 4);
    uint8_t* first = nullptr;
    {
        auto lease = pool.acquire(1000);
        check(lease.valid(), "a lease is valid");
        checkEq(lease.size(), 1000, "size reports the request");
        first = lease.data();
        std::memset(lease.data(), 0xAB, lease.size());
    }
    checkEq(pool.inFlightBytes(), 0, "the lease returns its block on scope exit");

    auto again = pool.acquire(1000);
    check(again.data() == first, "the next request reuses the same block");
}

void testBufferPoolHonoursBudget() {
    sched::BufferPool pool(1ull << 20, 4);
    auto held = pool.acquire(1ull << 20);
    check(held.valid(), "the first request fits the budget");

    std::atomic<bool> secondAcquired{false};
    std::thread waiter([&] {
        auto blocked = pool.acquire(1ull << 20);
        secondAcquired.store(true);
    });

    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    check(!secondAcquired.load(), "a request over the budget waits");
    checkEq(pool.peakBytes(), 1ull << 20, "the budget is not exceeded");

    held.release();
    waiter.join();
    check(secondAcquired.load(), "releasing the first lets the second through");
    checkEq(pool.peakBytes(), 1ull << 20, "the peak never rose above the budget");
}

void testBufferPoolAdmitsOversizedRequest() {
    sched::BufferPool pool(64ull * 1024, 2);
    auto lease = pool.acquire(4ull << 20);
    check(lease.valid(), "a request larger than the budget still proceeds");
    checkEq(lease.size(), 4ull << 20, "and gets the whole thing");
}

// Writes a file whose bytes are byteAt(position), then describes tiles inside it.
struct Fixture {
    std::filesystem::path path;
    svs::Slide slide;
    std::vector<uint64_t> offsets;
    std::vector<uint64_t> sizes;

    ~Fixture() {
        slide.file.reset();
        std::error_code ignored;
        std::filesystem::remove(path, ignored);
    }
};

bool buildFixture(Fixture& fixture, uint64_t fileBytes, bool scatter) {
    fixture.path = std::filesystem::temp_directory_path() /
                   "microtome_schedule_test.bin";
    {
        std::ofstream out(fixture.path, std::ios::binary | std::ios::trunc);
        if (!out) return false;
        std::vector<char> chunk(64 * 1024);
        for (uint64_t written = 0; written < fileBytes;) {
            const uint64_t n = std::min<uint64_t>(chunk.size(), fileBytes - written);
            for (uint64_t i = 0; i < n; ++i)
                chunk[static_cast<size_t>(i)] =
                    static_cast<char>(byteAt(written + i));
            out.write(chunk.data(), static_cast<std::streamsize>(n));
            written += n;
        }
    }

    // Mostly contiguous tiles with occasional holes, some of them wide enough to
    // break a run, plus one blank tile.
    uint64_t at = 4096;
    for (size_t i = 0; i < 64; ++i) {
        const uint64_t size = 20000 + (i % 7) * 1500;
        if (at + size > fileBytes) break;
        fixture.offsets.push_back(at);
        fixture.sizes.push_back(size);
        at += size;
        if (i % 8 == 7) at += 300000; // a hole no maxGap of 64 KB will swallow
        else if (i % 3 == 0) at += 4096; // a hole worth swallowing
    }
    fixture.sizes[5] = 0; // Aperio omits this tile

    if (scatter) {
        // Grid order stops matching offset order, which is what the sort exists
        // for: rotate the offsets against the indices.
        std::rotate(fixture.offsets.begin(),
                    fixture.offsets.begin() + 17,
                    fixture.offsets.end());
    }

    auto file = std::make_shared<RandomAccessFile>();
    if (!file->open(fixture.path.string())) return false;

    fixture.slide.path = fixture.path.string();
    fixture.slide.levels.push_back(levelWith(fixture.offsets, fixture.sizes));
    fixture.slide.file = std::move(file);
    return true;
}

void testEndToEnd(size_t threads, bool scatter) {
    const std::string label =
        "end to end (" + std::to_string(threads) + " threads" +
        (scatter ? ", scattered layout)" : ", ordered layout)");

    Fixture fixture;
    if (!buildFixture(fixture, 8ull << 20, scatter)) {
        ++failures;
        std::printf("  FAIL  %s: could not build the fixture\n", label.c_str());
        return;
    }

    const size_t tileCount = fixture.offsets.size();
    ThreadPool pool(threads, threads);

    std::mutex mutex;
    std::vector<size_t> seen(tileCount, 0);
    size_t wrongBytes = 0;

    sched::FetchOptions options;
    options.limits = sched::limitsFor(sched::Storage::LocalDisk);

    sched::FetchResult result;
    const bool ok = sched::fetchTiles(
        fixture.slide, 0, iota(tileCount), pool, options,
        [&](size_t tileIndex, const uint8_t* bytes, size_t size) {
            bool good = size == fixture.sizes[tileIndex];
            for (size_t i = 0; good && i < size; ++i)
                good = bytes[i] == byteAt(fixture.offsets[tileIndex] + i);
            std::lock_guard<std::mutex> lock(mutex);
            ++seen[tileIndex];
            if (!good) ++wrongBytes;
        },
        &result);

    check(ok, label + ": reports success");
    check(result.complete(), label + ": nothing failed or went missing");
    checkEq(wrongBytes, 0, label + ": every tile's bytes are correct");
    checkEq(result.blank.size(), 1, label + ": the blank tile did no I/O");
    checkEq(result.tilesFetched, tileCount - 1, label + ": every other tile arrived");

    size_t duplicates = 0, absent = 0;
    for (size_t i = 0; i < tileCount; ++i) {
        if (i == 5) continue; // the blank one
        if (seen[i] == 0) ++absent;
        if (seen[i] > 1) ++duplicates;
    }
    checkEq(absent, 0, label + ": no tile was skipped");
    checkEq(duplicates, 0, label + ": no tile was handed over twice");

    check(result.runs < tileCount, label + ": coalescing beat one read per tile");
    check(result.fetchedBytes >= result.wantedBytes,
          label + ": fetched at least what was wanted");
    check(result.peakBytes <= pool.threadCount() * options.limits.maxRun +
                                  sched::BufferPool::kGranularity,
          label + ": memory stayed inside the bound");

    std::printf("  %s: %zu tiles -> %zu reads, %llu KB wanted, %llu KB fetched\n",
                label.c_str(), tileCount, result.runs,
                static_cast<unsigned long long>(result.wantedBytes / 1024),
                static_cast<unsigned long long>(result.fetchedBytes / 1024));
}

void testObjectStoreLimitsFetchFewerRuns() {
    Fixture fixture;
    if (!buildFixture(fixture, 8ull << 20, false)) {
        ++failures;
        std::printf("  FAIL  object store limits: could not build the fixture\n");
        return;
    }

    const size_t tileCount = fixture.offsets.size();
    ThreadPool pool(4, 4);
    auto count = [&](sched::Storage storage, sched::FetchResult& out) {
        sched::FetchOptions options;
        options.limits = sched::limitsFor(storage);
        std::atomic<size_t> tiles{0};
        sched::fetchTiles(fixture.slide, 0, iota(tileCount), pool, options,
                          [&](size_t, const uint8_t*, size_t) { ++tiles; },
                          &out);
        return tiles.load();
    };

    sched::FetchResult local, remote;
    const size_t localTiles = count(sched::Storage::LocalDisk, local);
    const size_t remoteTiles = count(sched::Storage::ObjectStore, remote);

    checkEq(localTiles, remoteTiles, "both settings deliver every tile");
    check(remote.runs < local.runs,
          "a wider gap budget trades bytes for fewer requests");
    check(remote.fetchedBytes > local.fetchedBytes,
          "and pays for it in bytes read");

    std::printf("  tuning: local disk %zu reads / %llu KB, object store %zu reads"
                " / %llu KB\n",
                local.runs,
                static_cast<unsigned long long>(local.fetchedBytes / 1024),
                remote.runs,
                static_cast<unsigned long long>(remote.fetchedBytes / 1024));
}

void testMemoryBudgetThrottles() {
    Fixture fixture;
    if (!buildFixture(fixture, 8ull << 20, false)) {
        ++failures;
        std::printf("  FAIL  memory budget: could not build the fixture\n");
        return;
    }

    const size_t tileCount = fixture.offsets.size();
    ThreadPool pool(8, 8);

    sched::FetchOptions options;
    options.limits = sched::limitsFor(sched::Storage::LocalDisk);
    options.memoryBudget = 256ull * 1024; // far below threads * maxRun

    std::atomic<size_t> tiles{0};
    sched::FetchResult result;
    const bool ok =
        sched::fetchTiles(fixture.slide, 0, iota(tileCount), pool, options,
                          [&](size_t, const uint8_t*, size_t) { ++tiles; },
                          &result);

    check(ok, "a small budget still completes the batch");
    checkEq(tiles.load(), tileCount - 1, "and delivers every tile");
    check(result.peakBytes <= 256ull * 1024 + sched::BufferPool::kGranularity,
          "the budget held: peak stayed at the cap");
}

void testBadArguments() {
    svs::Slide empty;
    ThreadPool pool(2, 2);
    sched::FetchResult result;
    check(!sched::fetchTiles(empty, 0, iota(4), pool, sched::FetchOptions{},
                             [](size_t, const uint8_t*, size_t) {}, &result),
          "a slide with no file fails");

    Fixture fixture;
    if (!buildFixture(fixture, 1ull << 20, false)) return;
    check(!sched::fetchTiles(fixture.slide, 7, iota(4), pool,
                             sched::FetchOptions{},
                             [](size_t, const uint8_t*, size_t) {}, &result),
          "a level that does not exist fails");
}

} // namespace

int main() {
    std::printf("coalescing\n");
    testContiguousBecomesOneRun();
    testGapWithinBudgetIsSwallowed();
    testGapOverBudgetSplits();
    testMaxRunCapsARun();
    testOversizedTileIsNotTruncated();
    testOffsetOrderNotGridOrder();
    testDuplicatesCollapse();
    testBlankAndMissingTiles();
    testFileSizeGuard();
    testOverlappingRanges();
    testEmptyRequest();

    std::printf("buffers\n");
    testBufferPoolRecyclesBlocks();
    testBufferPoolHonoursBudget();
    testBufferPoolAdmitsOversizedRequest();

    std::printf("fetch\n");
    testEndToEnd(1, false);
    testEndToEnd(8, false);
    testEndToEnd(8, true);
    testObjectStoreLimitsFetchFewerRuns();
    testMemoryBudgetThrottles();
    testBadArguments();

    if (failures == 0) {
        std::printf("all checks passed\n");
        return 0;
    }
    std::printf("%d check(s) failed\n", failures);
    return 1;
}
