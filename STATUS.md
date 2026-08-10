# Project status

**Avelune is experimental. Nothing is frozen.**

The software release line is currently `0.x`. `ALV1` and `ALA1` identify **Draft Generation 1**
bitstreams; the `1` is a format-generation label, not a compatibility promise. Incompatible spec,
container, codec, or API changes are allowed while evidence supports them.

## Implemented POC

- custom ALV1 lossy/lossless 8-bit 4:2:0 video;
- custom ALA1 lossy/lossless audio;
- immutable bounded frame dependencies and half-sample motion;
- palette blocks, integer WHT residuals, static rANS entropy coding;
- front-indexed CRC-protected container with incremental parsing and HTTP Range epochs;
- safe Rust reference encoder/decoder crates;
- source-separated scalar ALV1 decoder for differential checks;
- native CLI and FFmpeg-facing foreign-format bridge;
- scalar WASM decoder and browser streaming demo;
- optional WebGPU presentation plus experimental compute-kernel prototypes;
- conformance vectors, malformed-input tests, benchmark tooling, and format research notes.

## Important interpretation

The implementation under `impl/rust/` is a **reference/research implementation**. Slow encoding or
playback in that implementation is not automatically a codec-architecture defect. Optimized SIMD,
threaded, GPU, and editor-oriented backends belong under `prod/` and must conform to the spec.

Conversely, weak compression caused by limited normative coding tools *is* a format-design concern.
The project should continue to distinguish implementation limits from bitstream limits through
measurement and experimentation.

## Known limitations

- general-video rate/distortion is behind mature x264/x265/VP9/AV1 encoders on tested material;
- lossy ALA1 is substantially behind Opus and needs further codec research;
- Draft Generation 1 is limited to the current baseline video/audio profiles;
- the native `play` path is a reference convenience path, not a low-latency production player;
- browser/WASM reference decoding is not guaranteed real-time at high resolutions;
- WebGPU compute kernels are experimental and were not validated on hardware in the original
  headless environment;
- mutation smoke testing is not a substitute for a sustained coverage-guided fuzz campaign;
- no claim of patent freedom or legal clearance is made.

See [`docs/development/VERSIONING.adoc`](docs/development/VERSIONING.adoc) and
[`docs/development/REFERENCE_IMPLEMENTATION.adoc`](docs/development/REFERENCE_IMPLEMENTATION.adoc).
