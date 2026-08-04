#include "buffers.h"

#include <algorithm>
#include <utility>

namespace sched {
namespace {

size_t roundUp(size_t bytes, size_t granularity) {
    if (bytes == 0) return granularity;
    const size_t slack = bytes % granularity;
    if (slack == 0) return bytes;
    const size_t pad = granularity - slack;
    if (bytes > SIZE_MAX - pad) return bytes; // no room to round
    return bytes + pad;
}

} // namespace

BufferPool::BufferPool(uint64_t budgetBytes, size_t maxIdleBlocks)
    : budget_(std::max<uint64_t>(budgetBytes, kGranularity)),
      maxIdleBlocks_(std::max<size_t>(maxIdleBlocks, 1)) {}

BufferPool::Lease& BufferPool::Lease::operator=(Lease&& other) noexcept {
    if (this != &other) {
        release();
        owner_ = other.owner_;
        block_ = std::move(other.block_);
        size_ = other.size_;
        other.owner_ = nullptr;
        other.size_ = 0;
    }
    return *this;
}

void BufferPool::Lease::release() {
    if (!owner_) return;
    BufferPool* owner = owner_;
    owner_ = nullptr;
    size_ = 0;
    owner->recycle(std::move(block_));
}

BufferPool::Lease BufferPool::acquire(size_t bytes) {
    const size_t want = roundUp(bytes, kGranularity);

    std::unique_lock<std::mutex> lock(mutex_);
    // The inFlight_ == 0 arm is what guarantees progress. The charge below
    // happens under the same lock, so a second waiter that also saw an empty
    // pool re-tests the predicate and waits its turn instead of overshooting.
    available_.wait(lock, [&] {
        return inFlight_ == 0 || inFlight_ + want <= budget_;
    });

    Block block;
    // Best fit, so a small run does not consume the one block large enough for
    // a large one. The list is at most one entry per worker.
    size_t best = idle_.size();
    for (size_t i = 0; i < idle_.size(); ++i) {
        if (idle_[i].capacity < want) continue;
        if (best == idle_.size() || idle_[i].capacity < idle_[best].capacity)
            best = i;
    }
    if (best != idle_.size()) {
        block = std::move(idle_[best]);
        idle_.erase(idle_.begin() + static_cast<std::ptrdiff_t>(best));
    } else {
        // new[] leaves the bytes indeterminate. A vector would zero them, and a
        // run overwrites every byte it uses anyway.
        block.bytes = std::unique_ptr<uint8_t[]>(new uint8_t[want]);
        block.capacity = want;
    }

    inFlight_ += block.capacity;
    peak_ = std::max(peak_, inFlight_);

    Lease lease;
    lease.owner_ = this;
    lease.size_ = bytes;
    lease.block_ = std::move(block);
    return lease;
}

uint64_t BufferPool::inFlightBytes() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return inFlight_;
}

uint64_t BufferPool::peakBytes() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return peak_;
}

void BufferPool::recycle(Block&& block) {
    if (!block.bytes) return;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        inFlight_ -= std::min<uint64_t>(inFlight_, block.capacity);
        if (idle_.size() < maxIdleBlocks_) idle_.push_back(std::move(block));
    }
    // Waiters want different amounts, so waking one is not enough: the one woken
    // might be the only one that still cannot proceed.
    available_.notify_all();
}

} // namespace sched
