#include "fetch.h"

#include <algorithm>
#include <atomic>
#include <condition_variable>
#include <mutex>
#include <utility>
#include "buffers.h"

namespace sched {
namespace {

// Above this, a large batch on a many-core machine would reserve more memory for
// buffers than the tiles it is fetching are worth.
constexpr uint64_t kBudgetCeiling = 256ull << 20;

// The pool's waitIdle() would also wait on work posted by other callers, and it
// races with concurrent posts. Counting only this batch's runs keeps one batch
// independent of every other user of the same pool.
class Latch {
public:
    explicit Latch(size_t count) : remaining_(count) {}

    void countDown() {
        bool last = false;
        {
            std::lock_guard<std::mutex> lock(mutex_);
            if (remaining_ > 0 && --remaining_ == 0) last = true;
        }
        if (last) done_.notify_all();
    }

    void wait() {
        std::unique_lock<std::mutex> lock(mutex_);
        done_.wait(lock, [this] { return remaining_ == 0; });
    }

private:
    std::mutex mutex_;
    std::condition_variable done_;
    size_t remaining_;
};

// The pool swallows an exception from a task, so counting down has to happen on
// the way out either way, or the batch would wait forever.
struct Countdown {
    Latch& latch;
    ~Countdown() { latch.countDown(); }
};

} // namespace

bool fetchTiles(const svs::Slide& slide, size_t level,
                std::span<const size_t> wanted, ThreadPool& pool,
                const FetchOptions& options, const TileSink& sink,
                FetchResult* result) {
    FetchResult discarded;
    FetchResult& out = result ? *result : discarded;
    out = FetchResult{};

    if (!slide.file || !slide.file->isOpen()) return false;
    if (level >= slide.levels.size()) return false;
    const svs::Level& lvl = slide.levels[level];

    const Plan plan =
        planTileFetch(lvl, wanted, options.limits, slide.file->size());
    out.runs = plan.runs.size();
    out.blank = plan.blank;
    out.missing = plan.missing;
    out.wantedBytes = plan.wantedBytes;
    out.fetchedBytes = plan.fetchedBytes;

    if (plan.runs.empty()) return out.complete();
    if (!sink) return false;

    // More buffers than there are runs, or than there are workers to hold them,
    // would never be used.
    const size_t concurrency =
        std::max<size_t>(1, std::min(pool.threadCount(), plan.runs.size()));
    uint64_t budget = options.memoryBudget;
    if (budget == 0)
        budget = std::min<uint64_t>(concurrency * options.limits.maxRun,
                                    kBudgetCeiling);
    BufferPool buffers(budget, concurrency);

    Latch latch(plan.runs.size());
    std::atomic<size_t> fetched{0};
    const RandomAccessFile& file = *slide.file;

    std::vector<ThreadPool::Task> tasks;
    tasks.reserve(plan.runs.size());
    for (const Run& current : plan.runs) {
        const Run* run = &current; // stable: the plan outlives the wait below
        tasks.emplace_back([run, &plan, &file, &buffers, &latch, &fetched,
                            &sink] {
            Countdown guard{latch};

            BufferPool::Lease lease =
                buffers.acquire(static_cast<size_t>(run->length()));
            if (!lease.valid()) return;
            if (!file.read(run->start, lease.data(), lease.size())) return;

            for (const TileRequest& request : members(plan, *run)) {
                sink(request.tileIndex, lease.data() + (request.offset - run->start),
                     static_cast<size_t>(request.size));
                fetched.fetch_add(1, std::memory_order_relaxed);
            }
        });
    }

    // postBulk takes the whole range or none of it, and takes none once the pool
    // is shutting down. The batch still has to finish, so run what it refused.
    const size_t accepted = pool.postBulk(tasks.begin(), tasks.end());
    for (size_t i = accepted; i < tasks.size(); ++i)
        if (tasks[i]) tasks[i]();

    latch.wait();

    // Deriving the failures instead of counting them covers every way a run can
    // end early, including a sink that threw partway through one.
    out.tilesFetched = fetched.load(std::memory_order_relaxed);
    out.tilesFailed =
        plan.requests.size() - std::min(out.tilesFetched, plan.requests.size());
    out.peakBytes = buffers.peakBytes();
    return out.complete();
}

} // namespace sched
