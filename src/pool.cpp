#include "pool.h"

#include <algorithm>

namespace {

std::size_t resolveThreadCount(std::size_t requested, std::size_t maxThreads) {
    std::size_t ceiling = maxThreads
                              ? std::min(maxThreads, ThreadPool::kThreadCeiling)
                              : ThreadPool::defaultThreadCount();
    if (ceiling == 0) ceiling = 1;
    std::size_t want = requested ? requested : ThreadPool::defaultThreadCount();
    return std::clamp(want, std::size_t{1}, ceiling);
}

} // namespace

std::size_t ThreadPool::defaultThreadCount() noexcept {
    unsigned hardware = std::thread::hardware_concurrency();
    if (hardware == 0) return 1;
    return std::min<std::size_t>(hardware, kThreadCeiling);
}

ThreadPool::ThreadPool(std::size_t threads, std::size_t maxThreads)
    : threadCount_(resolveThreadCount(threads, maxThreads)) {
    workers_.reserve(threadCount_);
    for (std::size_t i = 0; i < threadCount_; ++i)
        workers_.emplace_back([this] { worker(); });
}

ThreadPool::~ThreadPool() { shutdown(); }

std::size_t ThreadPool::pending() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return queue_.size();
}

// Waking exactly as many workers as there is work avoids the thundering herd of
// notify_all when a batch is smaller than the pool.
void ThreadPool::wake(std::size_t count) {
    if (count == 0) return;
    if (count >= threadCount_) {
        work_.notify_all();
        return;
    }
    for (std::size_t i = 0; i < count; ++i) work_.notify_one();
}

bool ThreadPool::post(Task task) {
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (stopping_) return false;
        queue_.push_back(std::move(task));
    }
    work_.notify_one();
    return true;
}

void ThreadPool::waitIdle() {
    std::unique_lock<std::mutex> lock(mutex_);
    idle_.wait(lock, [this] { return queue_.empty() && active_ == 0; });
}

void ThreadPool::shutdown() {
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (stopping_) return;
        stopping_ = true;
    }
    work_.notify_all();
    for (auto& thread : workers_)
        if (thread.joinable()) thread.join();
}

void ThreadPool::worker() {
    for (;;) {
        Task task;
        {
            std::unique_lock<std::mutex> lock(mutex_);
            work_.wait(lock, [this] { return stopping_ || !queue_.empty(); });
            // An empty queue here means shutdown, since the predicate only
            // admits the two cases. Draining first keeps shutdown from silently
            // discarding work that was already accepted.
            if (queue_.empty()) return;
            task = std::move(queue_.front());
            queue_.pop_front();
            ++active_;
        }

        // A throwing task must not take its worker, and so the pool, down.
        try {
            task();
        } catch (...) {
        }

        {
            std::lock_guard<std::mutex> lock(mutex_);
            --active_;
            if (queue_.empty() && active_ == 0) idle_.notify_all();
        }
    }
}
