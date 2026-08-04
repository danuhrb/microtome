#include <future>
#include <thread>

void job_func() {
    
}

void foo() {
    std::packaged_task<void(void)> task(job_func());

    std::future future = task.get_future();
    
    std::thread thread(std::move(task));

    future.wait_for(std::chrono::seconds(1));
}

std::vector<std::thread> threads;
bool shutdown_requested;

std::queue<std::function<void()>> queue;



