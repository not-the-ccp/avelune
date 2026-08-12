//! Audited unsafe boundary for Avelune production kernels.
#![warn(missing_docs)]
mod aarch64;
mod scalar;
mod wasm;
mod x86;

/// Selected implementation for low-level kernels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Portable scalar implementation.
    Scalar,
    /// x86 SSE4.2 CRC implementation with scalar non-CRC kernels.
    Sse42,
    /// x86 AVX2 bulk kernels plus SSE4.2 CRC.
    Avx2,
    /// AArch64 NEON bulk kernels with scalar CRC fallback.
    Aarch64Neon,
    /// AArch64 NEON plus ARMv8 CRC extension.
    Aarch64NeonCrc,
    /// WebAssembly SIMD128 bulk kernels.
    WasmSimd128,
}

/// Error returned when a forced CPU backend is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendUnavailable;

/// Safe, immutable dispatch table selected once per codec/parser instance.
#[derive(Debug, Clone, Copy)]
pub struct KernelSet {
    backend: Backend,
}
impl KernelSet {
    /// Selects the strongest locally available stable-intrinsics backend.
    pub fn auto() -> Self {
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        {
            Self {
                backend: Backend::WasmSimd128,
            }
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
        {
            #[cfg(target_arch = "x86_64")]
            {
                if std::arch::is_x86_feature_detected!("avx2")
                    && std::arch::is_x86_feature_detected!("sse4.2")
                {
                    return Self {
                        backend: Backend::Avx2,
                    };
                }
                if std::arch::is_x86_feature_detected!("sse4.2") {
                    return Self {
                        backend: Backend::Sse42,
                    };
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                if std::arch::is_aarch64_feature_detected!("neon") {
                    return Self {
                        backend: if std::arch::is_aarch64_feature_detected!("crc") {
                            Backend::Aarch64NeonCrc
                        } else {
                            Backend::Aarch64Neon
                        },
                    };
                }
            }
            Self {
                backend: Backend::Scalar,
            }
        }
    }
    /// Forces portable scalar behavior.
    pub const fn scalar() -> Self {
        Self {
            backend: Backend::Scalar,
        }
    }
    /// Forces SSE4.2 when the current x86-64 CPU exposes it.
    pub fn sse42() -> Result<Self, BackendUnavailable> {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return Ok(Self {
                backend: Backend::Sse42,
            });
        }
        Err(BackendUnavailable)
    }
    /// Forces AVX2 when the current x86-64 CPU exposes AVX2 and SSE4.2.
    pub fn avx2() -> Result<Self, BackendUnavailable> {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("sse4.2")
        {
            return Ok(Self {
                backend: Backend::Avx2,
            });
        }
        Err(BackendUnavailable)
    }
    /// Forces the baseline AArch64 NEON backend; CRC acceleration is selected when available.
    pub fn neon() -> Result<Self, BackendUnavailable> {
        #[cfg(target_arch = "aarch64")]
        {
            if !std::arch::is_aarch64_feature_detected!("neon") {
                return Err(BackendUnavailable);
            }
            let backend = if std::arch::is_aarch64_feature_detected!("crc") {
                Backend::Aarch64NeonCrc
            } else {
                Backend::Aarch64Neon
            };
            return Ok(Self { backend });
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            Err(BackendUnavailable)
        }
    }
    /// Selected backend.
    pub const fn backend(self) -> Backend {
        self.backend
    }
    /// CRC-32C with exactly the same semantics for every backend.
    pub fn crc32c(self, data: &[u8]) -> u32 {
        match self.backend {
            Backend::Scalar => scalar::crc32c(data),
            Backend::Sse42 | Backend::Avx2 => {
                #[cfg(target_arch = "x86_64")]
                {
                    // SAFETY: these variants can only be constructed after SSE4.2 detection.
                    unsafe { x86::crc32c_sse42(data) }
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    scalar::crc32c(data)
                }
            }
            Backend::Aarch64Neon => scalar::crc32c(data),
            Backend::Aarch64NeonCrc => {
                #[cfg(target_arch = "aarch64")]
                {
                    // SAFETY: this variant is only created after CRC feature detection.
                    unsafe { aarch64::crc32c_crc(data) }
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    scalar::crc32c(data)
                }
            }
            Backend::WasmSimd128 => scalar::crc32c(data),
        }
    }
    /// Inverse 8x8 Walsh-Hadamard transform with V1 exact rounding semantics.
    pub fn inverse_wht8x8(self, input: [i32; 64]) -> [i32; 64] {
        match self.backend {
            Backend::Avx2 => {
                #[cfg(target_arch = "x86_64")]
                {
                    // SAFETY: the variant is only constructible after AVX2 feature detection;
                    // all memory accesses are confined to fixed-size local arrays.
                    unsafe { x86::inverse_wht8x8_avx2(input) }
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    scalar::inverse_wht8x8(input)
                }
            }
            _ => scalar::inverse_wht8x8(input),
        }
    }

    /// Exact V1 half-sample prediction for one full interior 8x8 block.
    ///
    /// `reference` must begin at the integer floor of the requested prediction footprint.
    /// Fractional phases are 0 or 1. `None` means the supplied footprint/phase is invalid.
    pub fn halfpel_predict_8x8(
        self,
        reference: &[u8],
        stride: usize,
        fx: u8,
        fy: u8,
    ) -> Option<[u8; 64]> {
        if !scalar::halfpel_footprint_valid(reference.len(), stride, fx, fy) {
            return None;
        }
        #[cfg(target_arch = "x86_64")]
        if matches!(self.backend, Backend::Sse42 | Backend::Avx2) {
            let mut out = [0u8; 64];
            // SAFETY: scalar validation above proves the complete interior footprint exists;
            // SSE2 is baseline on x86-64 and the kernel uses unaligned loads/stores.
            unsafe { x86::halfpel_predict_8x8_sse2(reference, stride, fx, fy, &mut out) };
            return Some(out);
        }
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        if matches!(self.backend, Backend::WasmSimd128) {
            let mut out = [0u8; 64];
            // SAFETY: scalar validation above proves the complete footprint; this backend only
            // exists in a wasm32 artifact compiled with SIMD128 enabled.
            unsafe { wasm::halfpel_predict_8x8_simd128(reference, stride, fx, fy, &mut out) };
            return Some(out);
        }
        scalar::halfpel_predict_8x8(reference, stride, fx, fy)
    }

    /// Exact SAD of one strided 8x8 source block against half-sample prediction.
    pub fn halfpel_sad_8x8(
        self,
        source: &[u8],
        source_stride: usize,
        reference: &[u8],
        reference_stride: usize,
        fx: u8,
        fy: u8,
    ) -> Option<u64> {
        if matches!(self.backend, Backend::Scalar) {
            return scalar::halfpel_sad_8x8(
                source,
                source_stride,
                reference,
                reference_stride,
                fx,
                fy,
            );
        }
        let predicted = self.halfpel_predict_8x8(reference, reference_stride, fx, fy)?;
        if source_stride < 8 {
            return None;
        }
        let need = 7usize.checked_mul(source_stride)?.checked_add(8)?;
        if need > source.len() {
            return None;
        }
        Some(self.sad_block(source, source_stride, &predicted, 8, 8, 8))
    }

    /// Sum of absolute differences for a strided rectangular block.
    ///
    /// Returns scalar semantics for general shapes. x86 SIMD backends specialize the normative
    /// 8x8 motion-search shape without exposing pointer/alignment requirements to callers.
    pub fn sad_block(
        self,
        a: &[u8],
        a_stride: usize,
        b: &[u8],
        b_stride: usize,
        width: usize,
        height: usize,
    ) -> u64 {
        if width == 0 || height == 0 {
            return 0;
        }
        let a_need = (height - 1)
            .checked_mul(a_stride)
            .and_then(|x| x.checked_add(width));
        let b_need = (height - 1)
            .checked_mul(b_stride)
            .and_then(|x| x.checked_add(width));
        if a_need.is_none_or(|n| n > a.len()) || b_need.is_none_or(|n| n > b.len()) {
            return u64::MAX;
        }
        #[cfg(target_arch = "x86_64")]
        if width == 8 && height == 8 && matches!(self.backend, Backend::Sse42 | Backend::Avx2) {
            // SAFETY: bounds for all eight strided 8-byte rows were proved above; SSE2 is a
            // baseline x86-64 feature and the kernel uses unaligned 64-bit loads.
            return unsafe { x86::sad_8x8_sse2(a, a_stride, b, b_stride) };
        }
        scalar::sad_block(a, a_stride, b, b_stride, width, height)
    }

    /// Sum of absolute differences over the common prefix of the slices.
    pub fn sad(self, a: &[u8], b: &[u8]) -> u64 {
        match self.backend {
            Backend::Avx2 => {
                #[cfg(target_arch = "x86_64")]
                {
                    // SAFETY: the variant is only constructible after AVX2 detection; the kernel
                    // performs explicit length checks before every unaligned vector load.
                    unsafe { x86::sad_avx2(a, b) }
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    scalar::sad(a, b)
                }
            }
            Backend::Aarch64Neon | Backend::Aarch64NeonCrc => {
                #[cfg(target_arch = "aarch64")]
                {
                    // SAFETY: NEON is a baseline AArch64 facility and bounds are checked in-kernel.
                    unsafe { aarch64::sad_neon(a, b) }
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    scalar::sad(a, b)
                }
            }
            Backend::WasmSimd128 => {
                #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
                {
                    // SAFETY: this variant only exists in a SIMD128-targeted module and the kernel checks bounds before each load.
                    unsafe { wasm::sad_simd128(a, b) }
                }
                #[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
                {
                    scalar::sad(a, b)
                }
            }
            Backend::Scalar | Backend::Sse42 => scalar::sad(a, b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crc_known_vector() {
        assert_eq!(KernelSet::scalar().crc32c(b"123456789"), 0xe306_9283);
    }
    #[test]
    fn available_backends_match_scalar() {
        let mut x = 0x1234_5678_9abc_def0u64;
        for n in 0..2048 {
            let mut a = vec![0; n];
            let mut b = vec![0; n];
            for (aa, bb) in a.iter_mut().zip(&mut b) {
                x ^= x << 7;
                x ^= x >> 9;
                *aa = x as u8;
                x ^= x << 7;
                x ^= x >> 9;
                *bb = x as u8;
            }
            let scalar = KernelSet::scalar();
            let auto = KernelSet::auto();
            assert_eq!(auto.crc32c(&a), scalar.crc32c(&a));
            assert_eq!(auto.sad(&a, &b), scalar.sad(&a, &b));
            if let Ok(k) = KernelSet::avx2() {
                assert_eq!(k.sad(&a, &b), scalar.sad(&a, &b));
            }
        }
    }
    #[test]
    fn strided_8x8_sad_matches_scalar() {
        let scalar = KernelSet::scalar();
        let auto = KernelSet::auto();
        let mut state = 0x243f_6a88_85a3_08d3_u64;
        for stride in 8..=39 {
            for _ in 0..256 {
                let mut a = vec![0_u8; stride * 8];
                let mut b = vec![0_u8; stride * 8];
                for (aa, bb) in a.iter_mut().zip(&mut b) {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    *aa = state as u8;
                    state = state.rotate_left(23) ^ 0x9e37_79b9_7f4a_7c15;
                    *bb = state as u8;
                }
                let expected = scalar.sad_block(&a, stride, &b, stride, 8, 8);
                assert_eq!(auto.sad_block(&a, stride, &b, stride, 8, 8), expected);
                if let Ok(k) = KernelSet::avx2() {
                    assert_eq!(k.sad_block(&a, stride, &b, stride, 8, 8), expected);
                }
            }
        }
    }

    #[test]
    fn halfpel_backends_match_scalar_for_all_phases_and_strides() {
        let scalar = KernelSet::scalar();
        let auto = KernelSet::auto();
        let mut state = 0x1319_8a2e_0370_7344_u64;
        for ref_stride in 9..=31usize {
            for src_stride in 8..=23usize {
                for fy in 0..=1u8 {
                    for fx in 0..=1u8 {
                        for _ in 0..64 {
                            let rows = 8 + usize::from(fy);
                            let mut reference = vec![0u8; ref_stride * rows];
                            let mut source = vec![0u8; src_stride * 8];
                            for v in reference.iter_mut().chain(&mut source) {
                                state ^= state << 13;
                                state ^= state >> 7;
                                state ^= state << 17;
                                *v = state as u8;
                            }
                            let expected = scalar
                                .halfpel_predict_8x8(&reference, ref_stride, fx, fy)
                                .unwrap();
                            assert_eq!(
                                auto.halfpel_predict_8x8(&reference, ref_stride, fx, fy),
                                Some(expected)
                            );
                            let sad = scalar
                                .halfpel_sad_8x8(
                                    &source, src_stride, &reference, ref_stride, fx, fy,
                                )
                                .unwrap();
                            assert_eq!(
                                auto.halfpel_sad_8x8(
                                    &source, src_stride, &reference, ref_stride, fx, fy,
                                ),
                                Some(sad)
                            );
                            if let Ok(k) = KernelSet::avx2() {
                                assert_eq!(
                                    k.halfpel_predict_8x8(&reference, ref_stride, fx, fy),
                                    Some(expected)
                                );
                                assert_eq!(
                                    k.halfpel_sad_8x8(
                                        &source, src_stride, &reference, ref_stride, fx, fy,
                                    ),
                                    Some(sad)
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(auto.halfpel_predict_8x8(&[0; 64], 8, 1, 1), None);
        assert_eq!(auto.halfpel_predict_8x8(&[0; 72], 9, 2, 0), None);
    }

    #[test]
    fn inverse_wht_backends_match_scalar() {
        let scalar = KernelSet::scalar();
        let mut state = 0x6a09_e667_f3bc_c909_u64;
        for input in [
            [33_554_431_i32; 64],
            [-33_554_431_i32; 64],
            std::array::from_fn(|i| if i & 1 == 0 { 33_554_431 } else { -33_554_431 }),
        ] {
            let expected = scalar.inverse_wht8x8(input);
            assert_eq!(KernelSet::auto().inverse_wht8x8(input), expected);
            if let Ok(k) = KernelSet::avx2() {
                assert_eq!(k.inverse_wht8x8(input), expected);
            }
        }
        for _ in 0..20_000 {
            let mut input = [0_i32; 64];
            for v in &mut input {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *v = ((state as i32) % 2_000_001) - 1_000_000;
            }
            let expected = scalar.inverse_wht8x8(input);
            assert_eq!(KernelSet::auto().inverse_wht8x8(input), expected);
            if let Ok(k) = KernelSet::avx2() {
                assert_eq!(k.inverse_wht8x8(input), expected);
            }
        }
    }
}
