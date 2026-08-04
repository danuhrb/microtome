#pragma once
#include <condition_variable>
#include <cstddef>
#include <deque>
#include <functional>
#include <future>
#include <mutex>
#include <thread>
#include <type_traits>
#include <utility>
#include <vector>

// A fixed-size worker pool for tile reads and decodes.
//
// Tasks are std::move_only_function rather than std::function. A
// std::packaged_task is move-only, so std::function cannot hold one; the usual
// workaround is to wrap it in a shared_ptr, which costs an extra allocation and
// an atomic refcount on every task. move_only_function stores it directly.
class ThreadPool {
public:
    using Task = std::move_only_function<void()>;

    // Nothing the caller asks for can exceed this, however the request arrives.
    static constexpr std::size_t kThreadCeiling = 64;

    // Hardware concurrency, clamped to kThreadCeiling. Never 0.
    static std::size_t defaultThreadCount() noexcept;

    // threads == 0 means defaultThreadCount(), and maxThreads == 0 means the
    // cap is defaultThreadCount(). The final count is clamped to
    // [1, min(maxThreads, kThreadCeiling)], so a caller arriving from Python can
    // request any number of threads without oversubscribing the machine.
    explicit ThreadPool(std::size_t threads = 0, std::size_t maxThreads = 0);
    ~ThreadPool();

    ThreadPool(const ThreadPool&) = delete;
    ThreadPool& operator=(const ThreadPool&) = delete;

    std::size_t threadCount() const noexcept { return threadCount_; }
    std::size_t pending() const;

    // Fire and forget, skipping the promise and future that submit() allocates.
    // Returns false once the pool is shutting down.
    bool post(Task task);

    // Enqueues a range of Tasks under one lock instead of one lock per task.
    // Returns how many were accepted.
    template <class It>
    std::size_t postBulk(It first, It last);

    // The future reports a broken promise if the pool shuts down before the
    // task runs.
    template <class F, class... Args>
    auto submit(F&& f, Args&&... args)
        -> std::future<std::invoke_result_t<std::decay_t<F>, std::decay_t<Args>...>>;

    // Blocks until the queue is empty and no task is still running. Intended
    // for post-a-batch-then-wait use; concurrent posts race with it.
    void waitIdle();

    // Stops accepting work, lets the queue drain, then joins.
    void shutdown();

private:
    void worker();
    void wake(std::size_t count);

    mutable std::mutex mutex_;
    std::condition_variable work_;
    std::condition_variable idle_;
    std::deque<Task> queue_;
    // Immutable after construction, so wake() and threadCount() can read it
    // without holding the mutex.
    const std::size_t threadCount_;
    std::vector<std::thread> workers_;
    std::size_t active_ = 0;
    bool stopping_ = false;
};

template <class It>
std::size_t ThreadPool::postBulk(It first, It last) {
    std::size_t added = 0;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (stopping_) return 0;
        for (; first != last; ++first) {
            queue_.push_back(std::move(*first));
            ++added;
        }
    }
    wake(added);
    return added;
}

template <class F, class... Args>
auto ThreadPool::submit(F&& f, Args&&... args)
    -> std::future<std::invoke_result_t<std::decay_t<F>, std::decay_t<Args>...>> {
    using R = std::invoke_result_t<std::decay_t<F>, std::decay_t<Args>...>;
    std::packaged_task<R()> job(
        [fn = std::forward<F>(f),
         ... bound = std::forward<Args>(args)]() mutable -> R {
            return std::invoke(std::move(fn), std::move(bound)...);
        });
    auto future = job.get_future();
    post(Task(std::move(job)));
    return future;
}
