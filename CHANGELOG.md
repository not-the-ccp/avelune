# Changelog

All notable public-tree changes are recorded here. The project is pre-stable; format and API changes
may be incompatible until a stable major line is explicitly declared.

## Unreleased

- Reworked browser playback around bounded decode-ahead instead of pacing the decoder from video
  presentation callbacks. Audio is the playback clock for A/V streams, decoded PCM is scheduled
  ahead on its exact media timeline, late audio is reported as an underrun instead of being silently
  clamped to the current WebAudio time, and video presentation consumes the newest due decoded frame.
- Added deterministic playback-buffer tests plus real Chromium coverage with delayed HTTP ranges,
  pause/resume, seeking, and replay so transport jitter and clock resets cannot silently reintroduce
  audio gaps.
- Added the playback buffer module to the published demo assembly.

## 0.1.1

- Re-architected the project around one canonical safe Rust codec/container implementation, one
  independent scalar conformance oracle, and an isolated audited SIMD/unsafe kernel crate.
- Added the Draft Generation 1 ALV1 video, ALA1 audio, AVL container, entropy, baseline-profile and
  conformance specifications with explicit unfrozen `0.x` status.
- Added stateful canonical audio/video decode paths, reusable scratch storage, SIMD128/scalar WASM,
  WebGPU/Canvas browser rendering, indexed HTTP Range playback and strict stale-generation
  cancellation.
- Added ordinary-media browser conversion through embedded FFmpeg, streaming raw video/audio into
  the Avelune WASM encoder and spooling completed compressed epochs through OPFS when available.
- Added a static Astro documentation/specification site, generated Rust API docs, local search,
  browser demo/workbench, installation helper and release/support/security documentation.
- Added differential, hostile, metamorphic, property, mutation, scenario, cross-platform, browser,
  corpus/regression, reproducibility and unsafe-boundary validation.
