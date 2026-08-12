//! AArch64 NEON/CRC kernels.
//!
//! These kernels are source-reviewed against Rust 1.97.1 `std::arch::aarch64` intrinsics.
//! The supplied offline kit does not contain an AArch64 target standard library, so this
//! source is intentionally not described as cross-compiled or runtime-validated here.

#[cfg(target_arch = "aarch64")]
use core::arch::aarch64::{__crc32cd, vabdl_high_u8, vabdl_u8, vaddlvq_u16, vget_low_u8, vld1q_u8};

/// CRC-32C using the ARMv8 CRC extension for complete words and the scalar recurrence for tail bytes.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "crc")]
pub(super) unsafe fn crc32c_crc(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        let word = u64::from_le_bytes(c.try_into().expect("exact 8-byte chunk"));
        crc = __crc32cd(crc, word);
    }
    // Continue the exact reflected Castagnoli recurrence from the hardware-produced state.
    for &b in chunks.remainder() {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// NEON byte SAD over the common prefix of two slices.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(super) unsafe fn sad_neon(a: &[u8], b: &[u8]) -> u64 {
    let n = a.len().min(b.len());
    let mut i = 0usize;
    let mut sum = 0u64;
    while i + 16 <= n {
        // SAFETY: loop condition guarantees 16 readable bytes; vld1q_u8 permits unaligned input.
        let va = unsafe { vld1q_u8(a.as_ptr().add(i)) };
        // SAFETY: same bound as above for `b`.
        let vb = unsafe { vld1q_u8(b.as_ptr().add(i)) };
        let lo = vabdl_u8(vget_low_u8(va), vget_low_u8(vb));
        let hi = vabdl_high_u8(va, vb);
        sum += u64::from(vaddlvq_u16(lo)) + u64::from(vaddlvq_u16(hi));
        i += 16;
    }
    sum + super::scalar::sad(&a[i..n], &b[i..n])
}
