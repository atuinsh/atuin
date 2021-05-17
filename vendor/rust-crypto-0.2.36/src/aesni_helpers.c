// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#include <stdint.h>
#include <stdlib.h>

#if defined(__i386__) || defined(__x86_64__)

void rust_crypto_aesni_aesimc(uint8_t* round_keys) {
    #ifdef __SSE__
    asm volatile(
        " \
            movdqu (%0), %%xmm1; \
            aesimc %%xmm1, %%xmm1; \
            movdqu %%xmm1, (%0); \
        "
    : // outputs
    : "r" (round_keys) // inputs
    : "xmm1", "memory" // clobbers
    );
    #else
    exit(1);
    #endif
}

void rust_crypto_aesni_setup_working_key_128(
        uint8_t* key,
        uint8_t* round_key) {
    #ifdef __SSE__
    asm volatile(
        " \
            movdqu (%1), %%xmm1; \
            movdqu %%xmm1, (%0); \
            add $0x10, %0; \
            \
            aeskeygenassist $0x01, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x02, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x04, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x08, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x10, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x20, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x40, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x80, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x1b, %%xmm1, %%xmm2; \
            call 1f; \
            aeskeygenassist $0x36, %%xmm1, %%xmm2; \
            call 1f; \
            \
            jmp 2f; \
            \
            1: \
            pshufd $0xff, %%xmm2, %%xmm2; \
            vpslldq $0x04, %%xmm1, %%xmm3; \
            pxor %%xmm3, %%xmm1; \
            vpslldq $0x4, %%xmm1, %%xmm3; \
            pxor %%xmm3, %%xmm1; \
            vpslldq $0x04, %%xmm1, %%xmm3; \
            pxor %%xmm3, %%xmm1; \
            pxor %%xmm2, %%xmm1; \
            movdqu %%xmm1, (%0); \
            add $0x10, %0; \
            ret; \
            \
            2: \
        "
    : "+r" (round_key)
    : "r" (key)
    : "xmm1", "xmm2", "xmm3", "memory"
    );
    #else
    exit(1);
    #endif
}

void rust_crypto_aesni_setup_working_key_192(
        uint8_t* key,
        uint8_t* round_key) {
    #ifdef __SSE__
    asm volatile(
        " \
            movdqu (%1), %%xmm1; \
            movdqu 16(%1), %%xmm3; \
            movdqu %%xmm1, (%0); \
            movdqa %%xmm3, %%xmm5; \
            \
            aeskeygenassist $0x1, %%xmm3, %%xmm2; \
            call 1f; \
            shufpd $0, %%xmm1, %%xmm5; \
            movdqu %%xmm5, 16(%0); \
            movdqa %%xmm1, %%xmm6; \
            shufpd $1, %%xmm3, %%xmm6; \
            movdqu %%xmm6, 32(%0); \
            \
            aeskeygenassist $0x2, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 48(%0); \
            movdqa %%xmm3, %%xmm5; \
            \
            aeskeygenassist $0x4, %%xmm3, %%xmm2; \
            call 1f; \
            shufpd $0, %%xmm1, %%xmm5; \
            movdqu %%xmm5, 64(%0); \
            movdqa %%xmm1, %%xmm6; \
            shufpd $1, %%xmm3, %%xmm6; \
            movdqu %%xmm6, 80(%0); \
            \
            aeskeygenassist $0x8, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 96(%0); \
            movdqa %%xmm3, %%xmm5; \
            \
            aeskeygenassist $0x10, %%xmm3, %%xmm2; \
            call 1f; \
            shufpd $0, %%xmm1, %%xmm5; \
            movdqu %%xmm5, 112(%0); \
            movdqa %%xmm1, %%xmm6; \
            shufpd $1, %%xmm3, %%xmm6; \
            movdqu %%xmm6, 128(%0); \
            \
            aeskeygenassist $0x20, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 144(%0); \
            movdqa %%xmm3, %%xmm5; \
            \
            aeskeygenassist $0x40, %%xmm3, %%xmm2; \
            call 1f; \
            shufpd $0, %%xmm1, %%xmm5; \
            movdqu %%xmm5, 160(%0); \
            movdqa %%xmm1, %%xmm6; \
            shufpd $1, %%xmm3, %%xmm6; \
            movdqu %%xmm6, 176(%0); \
            \
            aeskeygenassist $0x80, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 192(%0); \
            \
            jmp 2f; \
            \
            1: \
            pshufd $0x55, %%xmm2, %%xmm2; \
            movdqu %%xmm1, %%xmm4; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm1; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm1; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm1; \
            pxor %%xmm2, %%xmm1; \
            pshufd $0xff, %%xmm1, %%xmm2; \
            movdqu %%xmm3, %%xmm4; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm3; \
            pxor %%xmm2, %%xmm3; \
            ret; \
            \
            2: \
        "
    : "+r" (round_key)
    : "r" (key)
    : "xmm1", "xmm2", "xmm3", "memory"
    );
    #else
    exit(1);
    #endif
}

void rust_crypto_aesni_setup_working_key_256(
        uint8_t* key,
        uint8_t* round_key) {
    #ifdef __SSE__
    asm volatile(
        " \
            movdqu (%1), %%xmm1; \
            movdqu 16(%1), %%xmm3; \
            movdqu %%xmm1, (%0); \
            movdqu %%xmm3, 16(%0); \
            \
            aeskeygenassist $0x1, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 32(%0); \
            \
            aeskeygenassist $0x0, %%xmm1, %%xmm2; \
            call 2f; \
            movdqu %%xmm3, 48(%0); \
            \
            aeskeygenassist $0x2, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 64(%0); \
            \
            aeskeygenassist $0x0, %%xmm1, %%xmm2; \
            call 2f; \
            movdqu %%xmm3, 80(%0); \
            \
            aeskeygenassist $0x4, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 96(%0); \
            \
            aeskeygenassist $0x0, %%xmm1, %%xmm2; \
            call 2f; \
            movdqu %%xmm3, 112(%0); \
            \
            aeskeygenassist $0x8, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 128(%0); \
            \
            aeskeygenassist $0x0, %%xmm1, %%xmm2; \
            call 2f; \
            movdqu %%xmm3, 144(%0); \
            \
            aeskeygenassist $0x10, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 160(%0); \
            \
            aeskeygenassist $0x0, %%xmm1, %%xmm2; \
            call 2f; \
            movdqu %%xmm3, 176(%0); \
            \
            aeskeygenassist $0x20, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 192(%0); \
            \
            aeskeygenassist $0x0, %%xmm1, %%xmm2; \
            call 2f; \
            movdqu %%xmm3, 208(%0); \
            \
            aeskeygenassist $0x40, %%xmm3, %%xmm2; \
            call 1f; \
            movdqu %%xmm1, 224(%0); \
            \
            jmp 3f; \
            \
            1: \
            pshufd $0xff, %%xmm2, %%xmm2; \
            movdqa %%xmm1, %%xmm4; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm1; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm1; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm1; \
            pxor %%xmm2, %%xmm1; \
            ret; \
            \
            2: \
            pshufd $0xaa, %%xmm2, %%xmm2; \
            movdqa %%xmm3, %%xmm4; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm3; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm3; \
            pslldq $4, %%xmm4; \
            pxor %%xmm4, %%xmm3; \
            pxor %%xmm2, %%xmm3; \
            ret; \
            \
            3: \
        "
    : "+r" (round_key)
    : "r" (key)
    : "xmm1", "xmm2", "xmm3", "memory"
    );
    #else
    exit(1);
    #endif
}

void rust_crypto_aesni_encrypt_block(
            uint8_t rounds,
            uint8_t* input,
            uint8_t* round_keys,
            uint8_t* output) {
    #ifdef __SSE__
    asm volatile(
    " \
        /* Copy the data to encrypt to xmm1 */ \
        movdqu (%2), %%xmm1; \
        \
        /* Perform round 0 - the whitening step */ \
        movdqu (%1), %%xmm0; \
        add $0x10, %1; \
        pxor %%xmm0, %%xmm1; \
        \
        /* Perform all remaining rounds (except the final one) */ \
        1: \
        movdqu (%1), %%xmm0; \
        add $0x10, %1; \
        aesenc %%xmm0, %%xmm1; \
        sub $0x01, %0; \
        cmp $0x01, %0; \
        jne 1b; \
        \
        /* Perform the last round */ \
        movdqu (%1), %%xmm0; \
        aesenclast %%xmm0, %%xmm1; \
        \
        /* Finally, move the result from xmm1 to outp */ \
        movdqu %%xmm1, (%3); \
    "
    : "+&r" (rounds), "+&r" (round_keys) // outputs
    : "r" (input), "r" (output) // inputs
    : "xmm0", "xmm1", "memory", "cc" // clobbers
    );
    #else
    exit(1);
    #endif
}

void rust_crypto_aesni_decrypt_block(
            uint8_t rounds,
            uint8_t* input,
            uint8_t* round_keys,
            uint8_t* output) {
    #ifdef __SSE__
    asm volatile(
        " \
            /* Copy the data to decrypt to xmm1 */ \
            movdqu (%2), %%xmm1; \
            \
            /* Perform round 0 - the whitening step */ \
            movdqu (%1), %%xmm0; \
            sub $0x10, %1; \
            pxor %%xmm0, %%xmm1; \
            \
            /* Perform all remaining rounds (except the final one) */ \
            1: \
            movdqu (%1), %%xmm0; \
            sub $0x10, %1; \
            aesdec %%xmm0, %%xmm1; \
            sub $0x01, %0; \
            cmp $0x01, %0; \
            jne 1b; \
            \
            /* Perform the last round */ \
            movdqu (%1), %%xmm0; \
            aesdeclast %%xmm0, %%xmm1; \
            \
            /* Finally, move the result from xmm1 to outp */ \
            movdqu %%xmm1, (%3); \
        "
    : "+&r" (rounds), "+&r" (round_keys) // outputs
    : "r" (input), "r" (output) // inputs
    : "xmm0", "xmm1", "memory", "cc" // clobbers
    );
    #else
    exit(1);
    #endif
}

#endif
