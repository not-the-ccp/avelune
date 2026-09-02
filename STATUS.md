# Project status

**Avelune is experimental. Nothing is frozen.**

The software release line is `0.x`. `ALV1` and `ALA1` identify **Draft Generation 1** bitstreams;
the `1` is a generation label, not a compatibility promise. Incompatible specification, container,
codec, and API changes are allowed while the draft is under active development.

## Implemented POC

- custom ALV1 lossy/lossless 8-bit 4:2:0 video;
- custom ALA1 lossy/lossless audio;
- bounded immutable video dependencies, half-sample motion, palette blocks, integer WHT residuals,
  and static rANS entropy coding;
- front-indexed CRC-protected container with independently addressable epochs;
- one canonical safe Rust implementation with per-stream state and explicit input finalization;
- a separate audited SIMD/unsafe kernel crate;
- one independent scalar conformance oracle for differential testing;
- native CLI with FFmpeg-based ordinary-media input/output;
- scalar and SIMD128 WASM decoding plus batch and streaming browser A/V encoder APIs;
- browser HTTP Range and local-file playback with cancellable decode generations;
- browser conversion of FFmpeg-readable media to `.avl`, with bounded raw video/PCM sinks and
  incremental Avelune epoch output;
- OPFS spooling of completed compressed epochs when supported, with an in-memory compressed-output
  fallback;
- Canvas2D and WebGPU YUV presentation;
- property, hostile-input, mutation, differential, benchmark, and format-research tooling.

## Implementation model

Applications use `crates/avelune`. `crates/avelune-reference` is an independently implemented oracle
for conformance and differential tests, not an application backend. Unsafe/intrinsic code is isolated
in `crates/avelune-kernels`.

Keeping the oracle independent makes differential agreement meaningful without maintaining a second
application stack.

## Known limitations

- general-video rate/distortion is behind mature x264/x265/VP9/AV1 encoders on tested material;
- lossy ALA1 needs substantially more codec research and perceptual tuning;
- Draft Generation 1 supports only the current baseline video/audio profiles;
- `avelune play` is a convenience FFmpeg/ffplay path, not the browser-style indexed player;
- the browser's embedded FFmpeg importer is single-threaded, and browsers without OPFS retain
  completed compressed output chunks in memory until final assembly;
- WebGPU compute kernels are experimental; WebGPU presentation is separate from codec semantics;
- mutation/property testing does not replace sustained coverage-guided fuzzing;
- no professional security audit or patent-freedom/legal-clearance claim exists.

See [`docs/development/REFERENCE_ORACLE.adoc`](docs/development/REFERENCE_ORACLE.adoc) and
[`docs/development/VERSIONING.adoc`](docs/development/VERSIONING.adoc).
