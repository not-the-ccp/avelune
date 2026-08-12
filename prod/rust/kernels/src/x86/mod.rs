//! x86/x86-64 architecture kernels.
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    __m128i, __m256i, _mm_add_epi16, _mm_add_epi64, _mm_avg_epu8, _mm_crc32_u8, _mm_crc32_u64,
    _mm_cvtsi128_si64, _mm_loadl_epi64, _mm_sad_epu8, _mm_set1_epi16, _mm_setzero_si128,
    _mm_srli_epi16, _mm_storel_epi64, _mm_storeu_si128, _mm_unpacklo_epi8, _mm256_add_epi32,
    _mm256_loadu_si256, _mm256_sad_epu8, _mm256_set1_epi32, _mm256_srai_epi32, _mm256_storeu_si256,
    _mm256_sub_epi32, _mm256_xor_si256,
};

/// Computes CRC-32C using SSE4.2 after the safe caller has verified feature support.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
pub(super) unsafe fn crc32c_sse42(data: &[u8]) -> u32 {
    let mut crc = u64::from(!0u32);
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        let word = u64::from_le_bytes(c.try_into().expect("exact 8-byte chunk"));
        crc = _mm_crc32_u64(crc, word);
    }
    let mut tail = crc as u32;
    for &b in chunks.remainder() {
        tail = _mm_crc32_u8(tail, b);
    }
    !tail
}

/// SSE2 8x8 strided SAD. SSE2 is baseline on x86-64; the safe caller validates slice bounds.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn sad_8x8_sse2(a: &[u8], a_stride: usize, b: &[u8], b_stride: usize) -> u64 {
    let mut acc = _mm_setzero_si128();
    for y in 0..8 {
        // SAFETY: the safe wrapper proves each row has at least 8 readable bytes. The intrinsic
        // performs an unaligned 64-bit load from the provided pointer.
        let va = unsafe { _mm_loadl_epi64(a.as_ptr().add(y * a_stride).cast::<__m128i>()) };
        // SAFETY: same row-bound invariant for `b`.
        let vb = unsafe { _mm_loadl_epi64(b.as_ptr().add(y * b_stride).cast::<__m128i>()) };
        acc = _mm_add_epi64(acc, _mm_sad_epu8(va, vb));
    }
    _mm_cvtsi128_si64(acc) as u64
}

/// SSE2 exact 8x8 half-sample prediction for an already bounds-validated interior footprint.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn halfpel_predict_8x8_sse2(
    src: &[u8],
    stride: usize,
    fx: u8,
    fy: u8,
    out: &mut [u8; 64],
) {
    let zero = _mm_setzero_si128();
    let two = _mm_set1_epi16(2);
    for y in 0..8 {
        // SAFETY: safe wrapper proves all required 8/9-byte rows and optional ninth row exist.
        let a = unsafe { _mm_loadl_epi64(src.as_ptr().add(y * stride).cast::<__m128i>()) };
        let pred = match (fx, fy) {
            (0, 0) => a,
            (1, 0) => {
                let b =
                    unsafe { _mm_loadl_epi64(src.as_ptr().add(y * stride + 1).cast::<__m128i>()) };
                _mm_avg_epu8(a, b)
            }
            (0, 1) => {
                let c = unsafe {
                    _mm_loadl_epi64(src.as_ptr().add((y + 1) * stride).cast::<__m128i>())
                };
                _mm_avg_epu8(a, c)
            }
            (1, 1) => {
                let b =
                    unsafe { _mm_loadl_epi64(src.as_ptr().add(y * stride + 1).cast::<__m128i>()) };
                let c = unsafe {
                    _mm_loadl_epi64(src.as_ptr().add((y + 1) * stride).cast::<__m128i>())
                };
                let d = unsafe {
                    _mm_loadl_epi64(src.as_ptr().add((y + 1) * stride + 1).cast::<__m128i>())
                };
                let aw = _mm_unpacklo_epi8(a, zero);
                let bw = _mm_unpacklo_epi8(b, zero);
                let cw = _mm_unpacklo_epi8(c, zero);
                let dw = _mm_unpacklo_epi8(d, zero);
                let sum = _mm_add_epi16(_mm_add_epi16(aw, bw), _mm_add_epi16(cw, dw));
                _mm_srli_epi16::<2>(_mm_add_epi16(sum, two))
            }
            _ => unreachable!(),
        };
        if fx == 1 && fy == 1 {
            let mut lanes = [0u16; 8];
            // SAFETY: lanes is exactly one writable 128-bit region.
            unsafe { _mm_storeu_si128(lanes.as_mut_ptr().cast::<__m128i>(), pred) };
            for x in 0..8 {
                out[y * 8 + x] = lanes[x] as u8;
            }
        } else {
            // SAFETY: each iteration writes exactly 8 bytes into one output row.
            unsafe { _mm_storel_epi64(out.as_mut_ptr().add(y * 8).cast::<__m128i>(), pred) };
        }
    }
}

/// AVX2 byte SAD over the common prefix of two slices.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(super) unsafe fn sad_avx2(a: &[u8], b: &[u8]) -> u64 {
    let n = a.len().min(b.len());
    let mut i = 0usize;
    let mut sum = 0u64;
    let mut lanes = [0u64; 4];
    while i + 32 <= n {
        // SAFETY: loop condition guarantees 32 readable bytes in both slices. Unaligned
        // loads are explicitly used, so there is no alignment precondition.
        let va = unsafe { _mm256_loadu_si256(a.as_ptr().add(i).cast::<__m256i>()) };
        let vb = unsafe { _mm256_loadu_si256(b.as_ptr().add(i).cast::<__m256i>()) };
        let s = _mm256_sad_epu8(va, vb);
        // SAFETY: `lanes` is a 32-byte writable region; unaligned store has no alignment requirement.
        unsafe { _mm256_storeu_si256(lanes.as_mut_ptr().cast::<__m256i>(), s) };
        sum += lanes.iter().sum::<u64>();
        i += 32;
    }
    sum + super::scalar::sad(&a[i..n], &b[i..n])
}

/// AVX2 inverse 8x8 WHT. Horizontal butterflies are scalar; vertical butterflies and
/// normalization operate on all eight columns at once.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(super) unsafe fn inverse_wht8x8_avx2(mut input: [i32; 64]) -> [i32; 64] {
    for y in 0..8 {
        let mut row = [0_i32; 8];
        row.copy_from_slice(&input[y * 8..y * 8 + 8]);
        super::scalar::hadamard8(&mut row);
        input[y * 8..y * 8 + 8].copy_from_slice(&row);
    }

    // SAFETY: each load is exactly one 8-i32 row inside the 64-element input array.
    let r0 = unsafe { _mm256_loadu_si256(input.as_ptr().cast::<__m256i>()) };
    let r1 = unsafe { _mm256_loadu_si256(input.as_ptr().add(8).cast::<__m256i>()) };
    let r2 = unsafe { _mm256_loadu_si256(input.as_ptr().add(16).cast::<__m256i>()) };
    let r3 = unsafe { _mm256_loadu_si256(input.as_ptr().add(24).cast::<__m256i>()) };
    let r4 = unsafe { _mm256_loadu_si256(input.as_ptr().add(32).cast::<__m256i>()) };
    let r5 = unsafe { _mm256_loadu_si256(input.as_ptr().add(40).cast::<__m256i>()) };
    let r6 = unsafe { _mm256_loadu_si256(input.as_ptr().add(48).cast::<__m256i>()) };
    let r7 = unsafe { _mm256_loadu_si256(input.as_ptr().add(56).cast::<__m256i>()) };

    let a0 = _mm256_add_epi32(r0, r1);
    let a1 = _mm256_sub_epi32(r0, r1);
    let a2 = _mm256_add_epi32(r2, r3);
    let a3 = _mm256_sub_epi32(r2, r3);
    let a4 = _mm256_add_epi32(r4, r5);
    let a5 = _mm256_sub_epi32(r4, r5);
    let a6 = _mm256_add_epi32(r6, r7);
    let a7 = _mm256_sub_epi32(r6, r7);

    let b0 = _mm256_add_epi32(a0, a2);
    let b1 = _mm256_add_epi32(a1, a3);
    let b2 = _mm256_sub_epi32(a0, a2);
    let b3 = _mm256_sub_epi32(a1, a3);
    let b4 = _mm256_add_epi32(a4, a6);
    let b5 = _mm256_add_epi32(a5, a7);
    let b6 = _mm256_sub_epi32(a4, a6);
    let b7 = _mm256_sub_epi32(a5, a7);

    let rows = [
        _mm256_add_epi32(b0, b4),
        _mm256_add_epi32(b1, b5),
        _mm256_add_epi32(b2, b6),
        _mm256_add_epi32(b3, b7),
        _mm256_sub_epi32(b0, b4),
        _mm256_sub_epi32(b1, b5),
        _mm256_sub_epi32(b2, b6),
        _mm256_sub_epi32(b3, b7),
    ];

    let bias = _mm256_set1_epi32(32);
    let mut out = [0_i32; 64];
    for (y, row) in rows.into_iter().enumerate() {
        let sign = _mm256_srai_epi32(row, 31);
        let abs = _mm256_sub_epi32(_mm256_xor_si256(row, sign), sign);
        let q = _mm256_srai_epi32(_mm256_add_epi32(abs, bias), 6);
        let rounded = _mm256_sub_epi32(_mm256_xor_si256(q, sign), sign);
        // SAFETY: each store writes exactly one 8-i32 row inside the 64-element output array.
        unsafe { _mm256_storeu_si256(out.as_mut_ptr().add(y * 8).cast::<__m256i>(), rounded) };
    }
    out
}
