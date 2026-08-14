# Project status

**Avelune is experimental. Nothing is frozen.**

The software release line is `0.x`. `ALV1` and `ALA1` identify **Draft Generation 1** bitstreams;
the `1` is a generation label, not a compatibility promise. Incompatible specification, container,
codec, and API changes remain allowed when evidence supports them.

## Implemented POC

- custom ALV1 lossy/lossless 8-bit 4:2:0 video;
- custom ALA1 lossy/lossless audio;
- bounded immutable video dependencies, half-sample motion, palette blocks, integer WHT residuals,
  and static rANS entropy coding;
- front-indexed CRC-protected container with independently addressable epochs;
- one canonical safe Rust implementation with per-stream state and explicit input finalization;
- a separate audited SIMD/unsafe kernel crate;
- one independent scalar conformance oracle for differential testing;
- native CLI and FFmpeg bridge;
- canonical scalar/SIMD128 WASM decoder plus a video-only raw encoder ABI;
- browser HTTP Range and local-file playback with cancellable decode generations;
- local 8-bit 4:2:0 Y4M → Avelune browser encoding without upload;
- Canvas2D and WebGPU YUV presentation;
- property, hostile-input, mutation, differential, benchmark, and format-research tooling.

## Implementation model

Applications use `crates/avelune`. `crates/avelune-reference` exists only to provide an independently
implemented oracle for conformance and differential tests; it is not selectable as a runtime
backend. Unsafe/intrinsic code is isolated in `crates/avelune-kernels`.

This distinction is deliberate: sharing parser/reconstruction implementation with the oracle would
make agreement less meaningful, while maintaining two complete application stacks created needless
duplication and divergent semantics.

## Known limitations

- general-video rate/distortion is behind mature x264/x265/VP9/AV1 encoders on tested material;
- lossy ALA1 needs substantially more codec research and perceptual tuning;
- Draft Generation 1 supports only the current baseline video/audio profiles;
- `avelune play` is a convenience FFmpeg/ffplay path, not the browser-style indexed player;
- WebGPU compute kernels remain experimental; WebGPU presentation is separate from codec semantics;
- mutation/property testing does not replace sustained coverage-guided fuzzing;
- no professional security audit or patent-freedom/legal-clearance claim exists.

See [`docs/development/REFERENCE_ORACLE.adoc`](docs/development/REFERENCE_ORACLE.adoc) and
[`docs/development/VERSIONING.adoc`](docs/development/VERSIONING.adoc).
