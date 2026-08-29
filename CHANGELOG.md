# Changelog

All notable public-tree changes are recorded here. The project is pre-stable; format and API changes
may be incompatible until a stable major line is explicitly declared.

## Unreleased

### Added

- Added a one-line source installer (`scripts/install.sh`, published as `install.sh` on the site)
  and a landing-page install command with a copy control.
- Added restrained landing-page motion: one-shot section reveals, a typed install command, and
  data-flow dashes on the architecture diagram, all disabled under `prefers-reduced-motion`.
- Added full-text documentation search, sitemap/robots metadata, canonical and social metadata,
  responsive publication navigation, mobile tables of contents, and previous/next reading links.
- Added generated-site semantic, link, fragment, responsive-overflow, keyboard, accessibility-tree,
  theme, print, search, and bundled-player browser checks to the regular development gate.
- Added one independent `avelune-reference` conformance oracle, with a scalar ALV1 decoder and a
  simple independent encoder used only for differential testing.
- Added stream-aware canonical decode sessions with per-stream codec state, explicit input
  finalization, and adversarial HTTP Range/seek-generation browser tests.
- Added source-neutral browser playback for exact HTTP ranges and local `Blob`/`File` sources,
  plus motion and screen-content demo fixtures, a local Y4M video-encoder lab, and a technical
  request/decoder inspector.

### Changed

- Reworked the Pages information architecture, responsive layout, typography, controls, landing
  page, documentation/specification indexes, API entry point, demo, and error page around clearer
  task-based paths and a consistent content hierarchy.
- Restructured Draft Generation 1 documentation with explicit status, conventions, reading order,
  stable anchors, cross-references, and more complete first-file CLI guidance.
- Removed obsolete entropy mode `1`; Draft Generation 1 decoders now accept only raw mode `0` and
  static byte rANS mode `2` with canonical-uvarint frequencies, with all other mode values rejected
  as invalid input.
- Corrected the independent-encoder interoperability lab to compare decoded reconstruction rather
  than requiring byte-identical packets from encoders whose policy choices may legitimately differ.
- Replaced the parallel reference/production application stacks with one canonical `avelune`
  implementation. The workspace now has six intentional packages: the canonical library, kernel
  boundary, reference oracle, WASM adapter, CLI, and lab tooling.
- Rewrote the CLI around typed command handlers and the canonical session decoder; removed the
  `--backend` switch and development-only benchmark/conformance/fuzz commands from the user CLI.
- Consolidated unsafe/SIMD code into `avelune-kernels`, and split the canonical ALV1/container
  implementations into cohesive source modules instead of monolithic implementation files.
- Reworked the browser player around owned playback generations, cancellable transport/audio,
  bounded presentation backpressure, reusable WebGPU resources, and explicit play/pause/seek.
- Bumped the browser WASM ABI to `0x0002_0000` for the canonical decoder/encoder interface,
  including explicit input finalization and retained encoder-creation diagnostics.
- Simplified CI, benchmark, WASM, rustdoc, and validation tooling around the canonical
  implementation while retaining the independent oracle for differential tests.
- Replaced the Pandoc Pages pipeline with the existing custom static Astro/AsciiDoc publication
  system and updated public architecture/CLI/browser documentation to match the canonical model.

### Removed

- Removed the `impl/rust` and `prod/rust` package trees, the five single-purpose reference component
  crates, the selectable production/reference runtime backends, and obsolete production-specific
  build/validation scripts.

## 0.1.1 - 2026-08-10

### Changed

- Made ordinary and raw audio encoding lossless by default (`ALA1 q=1`); lossy ALA1 remains an
  explicit experimental, non-perceptually-tuned option.
- Clarified public Draft Generation 1 wording, added facade-crate onboarding, and polished Pages and
  browser-demo navigation.
- Added a pinned Rust 1.97.1 toolchain file and locked dependency resolution to validation/build
  workflows; updated the Pages artifact action to v4.
- Restored canonical MIT license text and added a top-level dual-license notice.

## 0.1.0 - 2026-08-10

### Changed

- Reframed the previous internal “1.0” POC as an unstable public development tree.
- Declared `ALV1`/`ALA1` as Draft Generation 1 rather than frozen formats.
- Added a public Rust facade crate and a separate production-backend stub.
- Reworked repository documentation, contributor/security guidance, Pages build, API docs, and CI.
- Polished the CLI surface and added generated shell completions.

### Important limitations

- The Rust codec implementation is reference/research code, not an optimized production backend.
- Draft bitstream/API compatibility is not guaranteed.
- Lossy ALA1 and ALV1 compression efficiency remain research-grade rather than competitive with
  mature codecs on all content.
