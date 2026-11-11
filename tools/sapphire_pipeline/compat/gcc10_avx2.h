/*
 * GCC 10's <immintrin.h> has no _mm256_cvtsi256_si32, which GCC 11 added and the texture encoders' AVX2
 * kernels call (ctt-astcenc's vecmathlib, M4 C3). Steam Linux Runtime 3.0's SDK compiles with GCC 10, so
 * `cargo xtask package --platform linux` forces this header into the cooker's C++ there
 * (SAPPHIRE_COOKER_CXXFLAGS in packages.yml) and defines it as GCC 11 does: the vector's low 32 bits, one
 * vmovd. Any other compiler leaves it alone.
 */
#ifndef SAPPHIRE_GCC10_AVX2_H
#define SAPPHIRE_GCC10_AVX2_H

#if defined(__x86_64__) && defined(__GNUC__) && !defined(__clang__) && __GNUC__ < 11
#include <immintrin.h>

static inline __attribute__((target("avx2"), always_inline)) int _mm256_cvtsi256_si32(__m256i a)
{
    return _mm_cvtsi128_si32(_mm256_castsi256_si128(a));
}
#endif

#endif
