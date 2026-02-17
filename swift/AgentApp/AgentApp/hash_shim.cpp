// Shim for std::__1::__hash_memory — declared in Xcode 26 beta libc++ headers
// but missing from the runtime .tbd stubs. Provides a simple FNV-1a implementation.
// Remove this file once Apple ships the symbol in a future Xcode release.

#include <cstddef>
#include <cstdint>

namespace std { inline namespace __1 {

__attribute__((visibility("default")))
size_t __hash_memory(const void* ptr, size_t len) noexcept {
#if __SIZEOF_SIZE_T__ == 8
    // FNV-1a 64-bit
    const uint64_t FNV_OFFSET = 14695981039346656037ULL;
    const uint64_t FNV_PRIME  = 1099511628211ULL;
#else
    // FNV-1a 32-bit
    const uint32_t FNV_OFFSET = 2166136261U;
    const uint32_t FNV_PRIME  = 16777619U;
#endif
    auto data = static_cast<const unsigned char*>(ptr);
    size_t hash = FNV_OFFSET;
    for (size_t i = 0; i < len; ++i) {
        hash ^= static_cast<size_t>(data[i]);
        hash *= FNV_PRIME;
    }
    return hash;
}

}} // namespace std::__1
