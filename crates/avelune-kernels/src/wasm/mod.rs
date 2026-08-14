//! WASM SIMD128 kernels.
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
use core::arch::wasm32::{
    i16x8_splat, u8x16_avgr, u8x16_sub_sat, u16x8_add, u16x8_extadd_pairwise_u8x16,
    u16x8_extend_low_u8x16, u16x8_shr, u32x4_extadd_pairwise_u16x8, u32x4_extract_lane, v128_load,
    v128_load64_zero, v128_or, v128_store,
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

/// SIMD128 exact 8x8 half-sample prediction for a validated interior footprint.
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub(super) unsafe fn halfpel_predict_8x8_simd128(
    src: &[u8],
    stride: usize,
    fx: u8,
    fy: u8,
    out: &mut [u8; 64],
) {
    for y in 0..8 {
        // SAFETY: safe wrapper validated all required 8-byte row loads.
        let a = unsafe { v128_load64_zero(src.as_ptr().add(y * stride).cast::<u64>()) };
        let pred = match (fx, fy) {
            (0, 0) => a,
            (1, 0) => {
                let b = unsafe { v128_load64_zero(src.as_ptr().add(y * stride + 1).cast::<u64>()) };
                u8x16_avgr(a, b)
            }
            (0, 1) => {
                let c =
                    unsafe { v128_load64_zero(src.as_ptr().add((y + 1) * stride).cast::<u64>()) };
                u8x16_avgr(a, c)
            }
            (1, 1) => {
                let b = unsafe { v128_load64_zero(src.as_ptr().add(y * stride + 1).cast::<u64>()) };
                let c =
                    unsafe { v128_load64_zero(src.as_ptr().add((y + 1) * stride).cast::<u64>()) };
                let d = unsafe {
                    v128_load64_zero(src.as_ptr().add((y + 1) * stride + 1).cast::<u64>())
                };
                let aw = u16x8_extend_low_u8x16(a);
                let bw = u16x8_extend_low_u8x16(b);
                let cw = u16x8_extend_low_u8x16(c);
                let dw = u16x8_extend_low_u8x16(d);
                let sum = u16x8_add(u16x8_add(aw, bw), u16x8_add(cw, dw));
                u16x8_shr(u16x8_add(sum, i16x8_splat(2)), 2)
            }
            _ => unreachable!(),
        };
        if fx == 1 && fy == 1 {
            let mut lanes = [0u16; 8];
            // SAFETY: local array is exactly one writable v128 region.
            unsafe { v128_store(lanes.as_mut_ptr().cast(), pred) };
            for x in 0..8 {
                out[y * 8 + x] = lanes[x] as u8;
            }
        } else {
            let mut bytes = [0u8; 16];
            // SAFETY: local array is exactly one writable v128 region.
            unsafe { v128_store(bytes.as_mut_ptr().cast(), pred) };
            out[y * 8..y * 8 + 8].copy_from_slice(&bytes[..8]);
        }
    }
}
