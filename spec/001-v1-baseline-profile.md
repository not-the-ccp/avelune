# Avelune Draft Generation 1 Baseline profile

> **Draft status:** experimental, unfrozen, and subject to incompatible change while the software is `0.x`. `V1`/`ALV1`/`ALA1` refer to Draft Generation 1, not a stable 1.0 compatibility promise.


Status: **normative profile definition**.

The current Draft Generation 1 baseline intentionally has a bounded surface rather than untested feature flags. It is an interoperability target for this POC, not a compatibility freeze.

## Video

- codec: ALV1;
- 8-bit planar YUV 4:2:0;
- even dimensions, at most 8192x8192 and at most 8192^2 luma pixels;
- lossy and mathematically lossless (`qstep=1`);
- at most four immutable dependencies per frame;
- dependencies contained within indexed epochs;
- color matrix/transfer/primaries/range/chroma-location metadata as defined by the V1 container.

Not in Baseline V1: 4:2:2, 4:4:4, 10/12-bit, alpha, spatial scalability, HDR-specific reconstruction transforms, or normative GPU requirements. Future profiles must be explicitly versioned/profiled so a Baseline decoder rejects rather than silently misinterprets them.

## Audio

- codec: ALA1;
- 48,000 Hz;
- mono or stereo signed 16-bit PCM domain;
- reversible stereo coupling available for stereo;
- lossy and mathematically lossless (`qstep=1`);
- no decoder priming/pre-skip.

## Container

- finalized front-indexed Avelune V1 container;
- microsecond timescale for reference muxer streams;
- independently fetchable epochs suitable for HTTP Range streaming;
- CRC-32C protected header, front index, packet headers, and payloads.

## Compute backends

Conformance is defined only by decoded samples and container semantics. Scalar native Rust and scalar WASM are required implementation baselines for this project. SIMD, threads, WebGPU presentation, and WebGPU compute acceleration may be added without changing the bitstream so long as outputs remain conforming.
