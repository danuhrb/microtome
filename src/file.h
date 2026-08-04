#pragma once
#include <cstddef>
#include <cstdint>
#include <string>

// Every read specifies its own absolute offset, so there is no shared seek
// pointer and one handle can serve many threads at once. This is what lets a
// slide keep a single handle no matter how many workers read tiles from it.
class RandomAccessFile {
public:
    RandomAccessFile() = default;
    ~RandomAccessFile();

    RandomAccessFile(const RandomAccessFile&) = delete;
    RandomAccessFile& operator=(const RandomAccessFile&) = delete;

    bool open(const std::string& path);
    void close();
    bool isOpen() const;

    // Reads exactly n bytes, failing on a short read. Safe to call from
    // multiple threads at the same time.
    bool read(uint64_t offset, void* dst, size_t n) const;

    // Returns 0 if the size cannot be determined.
    uint64_t size() const;

private:
#ifdef _WIN32
    void* handle_ = nullptr; // HANDLE, kept as void* to avoid <windows.h> here
#else
    int fd_ = -1;
#endif
};
