// MIT; Copyright (c) 2026 Horizon contributors. Diagnostics ABI v2.
#include <stdint.h>
#include <stddef.h>
uint32_t helium_diag_init_v2(uint32_t test_only);
uint32_t helium_diag_event_v2(uint64_t attempt,uint32_t stage,uint32_t cause,uint64_t expected,uint64_t observed);
uint32_t helium_diag_snapshot_v2(uint64_t *words,size_t count);
uint32_t helium_diag_test_attempt_v2(uint64_t attempt);
uint32_t helium_diag_flush_test_v2(void);
