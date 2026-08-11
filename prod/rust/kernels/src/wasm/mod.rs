//! WASM SIMD128 kernels.
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
use core::arch::wasm32::{
    u8x16_sub_sat, u16x8_extadd_pairwise_u8x16, u32x4_extadd_pairwise_u16x8, u32x4_extract_lane,
    v128_load, v128_or,
};

/// SIMD128 byte SAD over the common prefix of two slices.
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub(super) unsafe fn sad_simd128(a: &[u8], b: &[u8]) -> u64 {
    let n = a.len().min(b.len());
    let mut i = 0usize;
    let mut sum = 0u64;
    while i + 16 <= n {
        // SAFETY: the loop condition guarantees 16 readable bytes in both slices.
        let va = unsafe { v128_load(a.as_ptr().add(i).cast()) };
        let vb = unsafe { v128_load(b.as_ptr().add(i).cast()) };
        let d = v128_or(u8x16_sub_sat(va, vb), u8x16_sub_sat(vb, va));
        let s16 = u16x8_extadd_pairwise_u8x16(d);
        let s32 = u32x4_extadd_pairwise_u16x8(s16);
        sum += u64::from(u32x4_extract_lane::<0>(s32))
            + u64::from(u32x4_extract_lane::<1>(s32))
            + u64::from(u32x4_extract_lane::<2>(s32))
            + u64::from(u32x4_extract_lane::<3>(s32));
        i += 16;
    }
    sum + super::scalar::sad(&a[i..n], &b[i..n])
}
