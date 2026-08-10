> **Historical note:** this document records an internal POC snapshot that was prematurely labeled “1.0”. The public project now treats these formats as unfrozen Draft Generation 1.

# Historical internal Avelune “1.0.0” snapshot (superseded)

Release date: 2026-08-10

This internal snapshot was once described as a frozen **V1 Baseline** bitstream/container release.
That description is superseded: the public project is experimental `0.1.0`, and Draft Generation 1
formats are not frozen or compatibility promises.

The freeze applies to normative V1 syntax, invalid-stream rules, integer decoding semantics, and decoded output. It does **not** freeze encoder search policy. Later compatible encoders may improve rate control, motion/reference search, palette selection, epoch placement, entropy-layout selection, and implementation performance without changing the V1 format.

## Included media formats

- **ALV1 video:** 8-bit planar 4:2:0, lossy or mathematically lossless, 8x8 integer WHT residuals, spatial prediction, half-sample inter prediction, up to four immutable frame dependencies, per-block reference selection, exact small palettes, adaptive per-plane entropy layout, and static rANS coding.
- **ALA1 audio:** custom reversible lifting-wavelet codec, reversible stereo coupling, 48 kHz mono/stereo V1 Baseline profile, mathematically lossless `qstep=1`, and lossy scalar quantization for higher qsteps.
- **Avelune container:** front-indexed independent epochs, CRC-32C protected headers/payloads, explicit stream/color metadata, incremental parsing, and byte-range-friendly seeking.

## Implementations and tools

The release contains a native Rust encoder/decoder and CLI, a scalar WASM decoder, a static range-streaming browser player, a source-separated scalar ALV1 reference decoder used for differential conformance testing, conformance vectors, repair/reindex/inspect/verify utilities, deterministic malformed-input mutation tests, and reproducible benchmark scripts.

The browser player decodes ALV1 and ALA1 itself. Browser-native codecs are not used for Avelune playback. Foreign input/output in the native workflow is delegated to FFmpeg rather than reimplemented.

## WebGPU

WebGPU is optional. V1 ships a WebGPU presentation path plus experimental compute prototypes for encoder motion-search SAD and normative inverse-WHT/reconstruction. Parsing, entropy coding, reference bookkeeping, and codec control remain in Rust/WASM. The intended long-term backend is hybrid: batchable image/block computation can remain GPU-resident, which is particularly useful for browser-based editing, while irregular bitstream work remains on the CPU/WASM side.

The release environment could not initialize Chromium's WebGPU GPU process, so the compute smoke harness is shipped but its runtime result is **not claimed as passed**. The ordinary WASM/Canvas path and HTTP-range streaming contract are validated independently.

## Validation snapshot

The release validator passes workspace tests, release builds, V1 conformance-vector generation, production/reference-decoder differential checks, intentionally corrupt-vector rejection, native and WASM builds, actual HTTP Range playback through the WASM decoder, reindex verification, deterministic container/raw-codec mutation smoke testing, and the available local 1080p throughput fixture.

The final range-stream fixture decodes 60 video frames and 95,999 stereo audio sample-frames after fetching only the front metadata plus two indexed epoch byte ranges. The six-frame 1920x1080 local fixture measures about 30 fps scalar native decode; the separate scalar Node/V8 WASM measurement is about 23 fps and excludes rendering.

## Real-media benchmarks

V1 was evaluated on actual packaged video/audio assets in addition to synthetic regression patterns. The executed video set includes an Earth/CGI clip, a 720p UI/timed-text movie, a real-photograph pan/zoom sequence, and a native 1080p performance fixture. Results are compared from the same decoded YUV source against x264, x265, VP9, and AV1.

ALV1 is functional but does not match mature codecs. On the photograph-derived motion sequence near 29.3 dB, the current Avelune point is about 57.5 KB versus roughly 21-38 KB for the mature encoders tested at similar quality. The gap is substantially larger on the executed Earth/CGI and low-complexity UI material.

ALA1 `qstep=1` is exactly lossless. Lossy ALA1 is substantially behind Opus and should be viewed as a clean custom codec baseline rather than a competitive general-purpose audio codec.

Scripts for the standard Xiph/Derf Bus and Foreman sequences are included, but those binary assets could not be downloaded into the release sandbox; no result for those sequences is claimed.

## Security and interoperability

V1 treats input as hostile. The container rejects unknown required flags and malformed lengths; ALV1 bounds dimensions, reference counts, dependencies, motion vectors, entropy data, and inverse-transform values so normative signed-32-bit arithmetic cannot overflow; ALA1 checks inverse lifting/stereo arithmetic and final sample ranges. Raw codec payload mutation is exercised separately from container CRC rejection.

The scalar reference ALV1 decoder is source-separated and follows literal normative arithmetic, but it was developed within the same project and is **not** claimed to be a clean-room independent implementation.

## Known limitations

V1 Baseline intentionally does not define 4:2:2, 4:4:4, >8-bit video, alpha, or baseline multichannel audio. Hand-written SIMD is not shipped. Scalar WASM falls below 1080p30 on the local Node/V8 fixture. WebGPU codec compute remains experimental. Release fuzzing is deterministic mutation smoke testing rather than a long coverage-guided campaign. No claim of patent freedom is made; see `docs/IPR-NOTES.md`.
