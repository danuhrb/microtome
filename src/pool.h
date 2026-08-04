#pragma once
#include <thread>
#include <iostream>
#include <vector>
#include <functional>
class ThreadPool {
    public:
        ThreadPool(size_t threads = std::thread::hardware_concurrency());
};
