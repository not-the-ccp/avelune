# Unsafe boundary

`avelune-prod-kernels` is the only production crate permitted to contain executable unsafe Rust.
`avelune-prod` itself has `#![forbid(unsafe_code)]`, and `scripts/audit-prod-unsafe.sh` rejects unsafe
blocks/functions outside the kernel crate and rejects assembly entirely.

Current unsafe surface:

- x86-64 SSE4.2 CRC-32C target-feature kernel;
- x86-64 AVX2 bulk byte-SAD target-feature kernel;
- x86-64 SSE2 8×8 strided motion-SAD target-feature kernel;
- x86-64 SSE2 exact interior 8×8 half-sample prediction kernel;
- x86-64 AVX2 inverse 8×8 WHT target-feature kernel;
- AArch64 NEON byte-SAD source kernel;
- AArch64 CRC-extension CRC-32C source kernel;
- WASM SIMD128 byte-SAD kernel in the SIMD-targeted artifact;
- WASM SIMD128 exact interior 8×8 half-sample prediction kernel.

The public API is always safe. `KernelSet` feature-specific variants can only be obtained after the
corresponding runtime feature checks (or, for `wasm32 +simd128`, by compiling the entire artifact for
that required feature). The unsafe calls therefore establish CPU-feature preconditions before
entering `#[target_feature]` functions.

Half-sample interpolation has an additional explicit footprint invariant: the safe `KernelSet` wrapper first executes the scalar validator, which proves the complete 8×8 plus right/bottom fractional tap footprint exists for the supplied stride and phase. Only then may the x86/WASM implementation perform its eight-byte unaligned row loads. Fractional phases are restricted to `{0,1}` in each axis, and the diagonal phase performs one exact `(a+b+c+d+2)>>2` rounding step rather than composing two rounded averages.

Memory invariants are local and narrow:

- SIMD loads/stores are explicitly unaligned where alignment is not guaranteed;
- every pointer-derived vector load is preceded by a slice/fixed-array bounds argument;
- no raw pointer, reference with extended lifetime, allocator object, or mutable alias escapes a
  kernel;
- there is no unsafe shared-memory concurrency;
- there is no custom allocator and no assembly.

Permanent scalar equivalents define the implementation-level kernel oracle. Randomized release tests
compare all locally available backends against scalar, including edge/non-vector lengths; half-sample coverage randomizes source/reference strides and all four fractional phases, while inverse-WHT coverage includes 20,000 randomized coefficient blocks in addition to end-to-end codec differential tests. The Node and Chromium WASM probes independently recompute all four half-sample phases in JavaScript.

AArch64 source has the same local safety structure but is explicitly **not** compile- or
runtime-validated in the supplied environment because its Rust target standard library is absent.
That is a platform gate, not a pass.
