// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#include <stddef.h>
#include <stdint.h>
#include <string.h>

#if defined(__i386__)
uint32_t rust_crypto_util_supports_aesni() {
    #ifdef __SSE__
    uint32_t flags;
    asm(
        "pushl %%ebx; \
         mov $1, %%eax; cpuid; \
         popl %%ebx;"
        : "=c" (flags) // output
        : // input
        : "eax", "edx" // clobbers
    );
    return flags & 0x02000000;
    #else
    return 0;
    #endif
}
#endif

#if defined(__x86_64__)
uint32_t rust_crypto_util_supports_aesni() {
    #ifdef __SSE__
    uint32_t flags;
    asm(
        "mov $1, %%eax; cpuid;"
        : "=c" (flags) // output
        : // input
        : "eax", "ebx", "edx" // clobbers
    );
    return flags & 0x02000000;
    #else
    return 0;
    #endif
}
#endif

#if defined(__i386__) || defined(__x86_64__)
uint32_t rust_crypto_util_fixed_time_eq_asm(uint8_t* lhsp, uint8_t* rhsp, size_t count) {
    if (count == 0) {
        return 1;
    }
    uint8_t result = 0;
    asm(
        " \
            1: \
            \
            mov (%1), %%cl; \
            xor (%2), %%cl; \
            or %%cl, %0; \
            \
            inc %1; \
            inc %2; \
            dec %3; \
            jnz 1b; \
        "
        : "+&r" (result), "+&r" (lhsp), "+&r" (rhsp), "+&r" (count) // all input and output
        : // input
        : "cl", "cc" // clobbers
    );

    return result;
}
#endif

#ifdef __arm__
uint32_t rust_crypto_util_fixed_time_eq_asm(uint8_t* lhsp, uint8_t* rhsp, size_t count) {
    if (count == 0) {
        return 1;
    }
    uint8_t result = 0;
    asm(
        " \
            1: \
            \
            ldrb r4, [%1]; \
            ldrb r5, [%2]; \
            eor r4, r4, r5; \
            orr %0, %0, r4; \
            \
            add %1, %1, #1; \
            add %2, %2, #1; \
            subs %3, %3, #1; \
            bne 1b; \
        "
        : "+&r" (result), "+&r" (lhsp), "+&r" (rhsp), "+&r" (count) // all input and output
        : // input
        : "r4", "r5", "cc" // clobbers
    );

    return result;
}
#endif

void rust_crypto_util_secure_memset(uint8_t* dst, uint8_t val, size_t count) {
    memset(dst, val, count);
    asm volatile("" : : "g" (dst) : "memory");
}

