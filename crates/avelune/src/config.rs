//! Canonical runtime configuration.

/// CPU implementation policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CpuBackend {
    /// Select the best validated implementation available at runtime.
    #[default]
    Auto,
    /// Force scalar code.
    Scalar,
    /// Force SSE4-class x86 code where available.
    Sse,
    /// Force AVX2 code where available.
    Avx2,
    /// Force AVX-512 code where available.
    Avx512,
    /// Force AArch64 NEON code where available.
    Neon,
}

/// Thread-count policy for stateful encoders/decoders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThreadPolicy {
    /// Choose a bounded count from available parallelism and workload size.
    #[default]
    Auto,
    /// Disable internal parallel execution.
    Single,
    /// Request an explicit maximum worker count.
    Max(usize),
}

/// Common runtime configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Config {
    /// CPU backend selection.
    pub cpu: CpuBackend,
    /// Internal threading policy.
    pub threads: ThreadPolicy,
    /// Defensive parser/entropy memory ceilings.
    pub limits: crate::limits::Limits,
}

/// Error produced when a forced low-level backend is unavailable in this build/runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendUnavailable(pub CpuBackend);

impl std::fmt::Display for BackendUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "requested CPU backend {:?} is unavailable", self.0)
    }
}
impl std::error::Error for BackendUnavailable {}

/// Resolves a runtime CPU policy once into the audited low-level dispatch table.
pub fn kernel_set(policy: CpuBackend) -> Result<crate::kernels::KernelSet, BackendUnavailable> {
    use crate::kernels::KernelSet;
    match policy {
        CpuBackend::Auto => Ok(KernelSet::auto()),
        CpuBackend::Scalar => Ok(KernelSet::scalar()),
        CpuBackend::Sse => KernelSet::sse42().map_err(|_| BackendUnavailable(policy)),
        CpuBackend::Avx2 => KernelSet::avx2().map_err(|_| BackendUnavailable(policy)),
        // No AVX-512 kernel is retained until a measured end-to-end win exists.
        CpuBackend::Avx512 => Err(BackendUnavailable(policy)),
        CpuBackend::Neon => KernelSet::neon().map_err(|_| BackendUnavailable(policy)),
    }
}
