#include "file.h"

#include <algorithm>

#ifdef _WIN32
#include <windows.h>
#else
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

RandomAccessFile::~RandomAccessFile() { close(); }

#ifdef _WIN32

bool RandomAccessFile::open(const std::string& path) {
    close();
    HANDLE h = CreateFileA(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                           OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return false;
    handle_ = h;
    return true;
}

void RandomAccessFile::close() {
    if (handle_) {
        CloseHandle(static_cast<HANDLE>(handle_));
        handle_ = nullptr;
    }
}

bool RandomAccessFile::isOpen() const { return handle_ != nullptr; }

bool RandomAccessFile::read(uint64_t offset, void* dst, size_t n) const {
    if (!handle_) return false;
    auto* out = static_cast<uint8_t*>(dst);
    size_t done = 0;
    // Passing an OVERLAPPED with an explicit offset reads from that position
    // without depending on the handle's file pointer, so concurrent callers do
    // not interfere with each other.
    while (done < n) {
        uint64_t at = offset + done;
        OVERLAPPED ov = {};
        ov.Offset = static_cast<DWORD>(at & 0xFFFFFFFFull);
        ov.OffsetHigh = static_cast<DWORD>(at >> 32);
        DWORD want = static_cast<DWORD>(std::min<size_t>(n - done, 1u << 30));
        DWORD got = 0;
        if (!ReadFile(static_cast<HANDLE>(handle_), out + done, want, &got, &ov))
            return false;
        if (got == 0) return false; // EOF before n bytes
        done += got;
    }
    return true;
}

uint64_t RandomAccessFile::size() const {
    if (!handle_) return 0;
    LARGE_INTEGER li;
    if (!GetFileSizeEx(static_cast<HANDLE>(handle_), &li)) return 0;
    return static_cast<uint64_t>(li.QuadPart);
}

#else

bool RandomAccessFile::open(const std::string& path) {
    close();
    int fd = ::open(path.c_str(), O_RDONLY);
    if (fd < 0) return false;
    fd_ = fd;
    return true;
}

void RandomAccessFile::close() {
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
}

bool RandomAccessFile::isOpen() const { return fd_ >= 0; }

bool RandomAccessFile::read(uint64_t offset, void* dst, size_t n) const {
    if (fd_ < 0) return false;
    auto* out = static_cast<uint8_t*>(dst);
    size_t done = 0;
    // pread does not use or modify the shared file offset.
    while (done < n) {
        ssize_t got = ::pread(fd_, out + done, n - done,
                              static_cast<off_t>(offset + done));
        if (got < 0) {
            if (errno == EINTR) continue;
            return false;
        }
        if (got == 0) return false; // EOF before n bytes
        done += static_cast<size_t>(got);
    }
    return true;
}

uint64_t RandomAccessFile::size() const {
    if (fd_ < 0) return 0;
    struct stat st;
    if (::fstat(fd_, &st) != 0) return 0;
    return static_cast<uint64_t>(st.st_size);
}

#endif
