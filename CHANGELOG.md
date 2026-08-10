# Changelog

All notable public-tree changes are recorded here. The project is pre-stable; format and API changes
may be incompatible until a stable major line is explicitly declared.

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
