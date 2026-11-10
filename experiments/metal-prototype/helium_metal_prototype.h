#ifndef HELIUM_METAL_PROTOTYPE_H
#define HELIUM_METAL_PROTOTYPE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
// Blocking offscreen test. Apple worker thread only. No production surface or world access.
// 0: terrain/depth readback passed; 1: Metal/oracle error; 2: caught Rust panic.
int32_t helium_metal_smoke_test(void);

enum {
    HELIUM_METAL_WIDTH = 256,
    HELIUM_METAL_HEIGHT = 256,
    HELIUM_METAL_RGBA_BYTES = 256 * 256 * 4,
    HELIUM_METAL_OK = 0,
    HELIUM_METAL_TEST_FAILED = 1,
    HELIUM_METAL_PANIC = 2,
    HELIUM_METAL_INVALID_ARGUMENT = 3
};

// Blocking Apple worker-thread call. Runs the unchanged smoke test, including its
// second render with reversed triangle order. On success copies the FIRST actual
// GPU readback into rgba_out: 256x256 tightly packed RGBA8Unorm, 1024 bytes/row,
// texture row 0 first. No borrowed Rust memory escapes; pointers are not retained.
//
// The caller owns both writable allocations exclusively for the entire call.
// They must be valid for their stated capacities and must not overlap. Null,
// zero/insufficient capacities, address-range overflow and overlap are rejected;
// allocation validity/dangling pointers cannot be verified by a C API.
// rgba_capacity must be >= HELIUM_METAL_RGBA_BYTES; message_capacity must be >= 1.
// On failure rgba_out is untouched. Extra pixel capacity is always untouched.
// message_out receives a NUL-terminated UTF-8 success diagnostic or failure reason,
// truncated at a UTF-8 boundary to fit; use >= 4096 bytes for useful diagnostics.
// If the message buffer is invalid or buffers overlap, returns INVALID_ARGUMENT
// without writing either buffer. Otherwise invalid pixel arguments get a message.
// Return codes are the constants above. Rust unwinding panics are caught; process
// aborts and Objective-C exceptions cannot be recovered by this interface.
// GPU completion/readback does not establish physical display presentation.
int32_t helium_metal_smoke_test_rgba(uint8_t *rgba_out, size_t rgba_capacity,
                                     char *message_out, size_t message_capacity);
#ifdef __cplusplus
}
#endif
#endif
