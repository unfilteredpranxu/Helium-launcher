#ifndef HELIUM_METAL_PROTOTYPE_H
#define HELIUM_METAL_PROTOTYPE_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
// Blocking offscreen test. Apple worker thread only. No production surface or world access.
// 0: terrain/depth readback passed; 1: Metal/oracle error; 2: caught Rust panic.
int32_t helium_metal_smoke_test(void);
#ifdef __cplusplus
}
#endif
#endif
