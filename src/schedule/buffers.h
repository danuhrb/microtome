#pragma once
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <vector>

// A run needs a buffer as large as itself, so the bytes a batch holds at once
// are set by how many runs are in flight, up to threads * maxRun. Left
// unbounded that breaks the promise that memory does not grow with the thread
// count, and a wide read_batch on a slow disk would queue every run's buffer at
// once. Acquiring against a fixed budget blocks the worker instead.
//
// The blocks are recycled rather than freed, because a batch of a thousand runs
// would otherwise allocate and return several megabytes a thousand times.

namespace sched {

class BufferPool {
public:
    // Rounding up the request improves the odds that a returned block fits the
    // next one, at the cost of a bounded amount of slack per block.
    static constexpr size_t kGranularity = 64ull * 1024;

    // A budget smaller than one request is still honoured for that request when
    // nothing else is in flight, so an oversized run cannot deadlock.
    explicit BufferPool(uint64_t budgetBytes, size_t maxIdleBlocks = 8);

    BufferPool(const BufferPool&) = delete;
    BufferPool& operator=(const BufferPool&) = delete;

private:
    struct Block {
        std::unique_ptr<uint8_t[]> bytes;
        size_t capacity = 0;
    };

public:
    // Returns its block to the pool when it goes out of scope, so an early
    // return from a failed read cannot lose it.
    class Lease {
    public:
        Lease() = default;
        ~Lease() { release(); }

        Lease(Lease&& other) noexcept { *this = std::move(other); }
        Lease& operator=(Lease&& other) noexcept;

        Lease(const Lease&) = delete;
        Lease& operator=(const Lease&) = delete;

        uint8_t* data() const { return block_.bytes.get(); }
        size_t size() const { return size_; } // as requested, not as allocated
        bool valid() const { return owner_ != nullptr && block_.bytes != nullptr; }

        void release();

    private:
        friend class BufferPool;
        BufferPool* owner_ = nullptr;
        Block block_;
        size_t size_ = 0;
    };

    // Blocks until the budget has room. Never returns a smaller block than
    // asked for.
    Lease acquire(size_t bytes);

    uint64_t inFlightBytes() const;
    uint64_t peakBytes() const;

private:
    void recycle(Block&& block);

    mutable std::mutex mutex_;
    std::condition_variable available_;
    std::vector<Block> idle_;
    const uint64_t budget_;
    const size_t maxIdleBlocks_;
    uint64_t inFlight_ = 0;
    uint64_t peak_ = 0;
};

} // namespace sched
