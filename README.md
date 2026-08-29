# Avelune

Avelune is an experimental, clean-slate audiovisual codec family and indexed streaming container
written in Rust. The repository is deliberately **spec-first** and contains a working proof of
concept: custom video and audio codecs, an indexed container, native tooling, a canonical WASM
decoder/video-encoder adapter, and a browser player that uses HTTP Range or local files.

> **Status:** research / POC. Nothing is frozen. The software is `0.x`; `ALV1` and `ALA1` are Draft
> Generation 1 format identifiers, not compatibility promises.

## Architecture

Avelune intentionally has one application implementation, not competing “reference” and
“production” stacks:

- [`spec/`](spec/) is the normative Draft Generation 1 source.
- [`crates/avelune`](crates/avelune/) is the canonical safe Rust implementation used by the CLI,
  WASM binding, tests, and applications.
- [`crates/avelune-kernels`](crates/avelune-kernels/) is the small audited unsafe/SIMD boundary.
- [`crates/avelune-reference`](crates/avelune-reference/) is an **independent conformance oracle**,
  not an application backend. It deliberately duplicates decode math so differential tests can
  detect canonical implementation mistakes.
- [`crates/avelune-wasm`](crates/avelune-wasm/) exposes the canonical incremental decoder and a video-only raw encoder to the
  browser.
- [`tools/`](tools/) contains the user CLI and development-only measurement tooling.

The canonical implementation is not normative: if implementation evidence exposes a format flaw,
the design should be reconsidered rather than defining the format by accident.

## Current POC

- **ALV1 video** — 8-bit 4:2:0, mathematically lossless and lossy modes, spatial/inter prediction,
  half-sample translational motion, immutable bounded references, palette blocks, integer WHT
  residuals, and static rANS entropy coding.
- **ALA1 audio** — custom lifting-wavelet codec with reversible stereo coupling and a true lossless
  mode.
- **Avelune container** — CRC-protected packets, a front index, independently fetchable epochs, and
  incremental parsing suitable for HTTP Range playback.
- **Canonical session decoder** — declared-stream validation, per-stream codec state, explicit epoch
  reset/finalization, and arbitrary input fragmentation.
- **CLI** — ordinary-media encode/decode through FFmpeg, inspect/verify, repair/reindex, raw Y4M and
  PCM workflows, and completions.
- **Browser** — scalar/SIMD128 WASM artifacts, exact Range validation, local-file playback,
  cancellable decode generations, Canvas2D/WebGPU presentation, a local Y4M→Avelune encoder lab,
  and a technical event inspector.
- **Conformance** — independent scalar ALV1/ALA1 decoding plus differential/property/hostile-input
  tests.

## Quick start

Install the CLI from source (one line; needs `git` and Rust 1.97.1, installs to
`/usr/local/bin` or `~/.local/bin`):

```sh
curl -fsSL https://not-the-ccp.github.io/avelune/install.sh | sh
```

Requirements:

- Rust 1.97.1;
- FFmpeg/ffprobe for ordinary media `encode`, `decode`, and `play`;
- Node.js 22.12+ with npm for the documentation site.

Or build manually from the repository:

```sh
cargo build --release --workspace
./target/release/avelune --help

./target/release/avelune encode input.mkv output.avl
./target/release/avelune inspect output.avl
./target/release/avelune verify output.avl
./target/release/avelune decode output.avl roundtrip.mkv
```

Raw video workflows use 8-bit 4:2:0 Y4M:

```sh
avelune raw encode-y4m input.y4m output.avl --q 96
avelune raw decode-y4m output.avl decoded.y4m
```

## Rust API

The Rust API remains experimental and unfrozen while Avelune is `0.x`.

```toml
[dependencies]
avelune = { git = "https://github.com/not-the-ccp/avelune", tag = "v0.1.1" }
```

```rust
use avelune::audio::v1::{encode, AudioError, EncodeOptions};

fn encode_lossless() -> Result<Vec<u8>, AudioError> {
    encode(&[0_i16, 1000, -1000, 0], EncodeOptions {
        sample_rate: 48_000,
        channels: 1,
        qstep: 1,
        mid_side: false,
    })
}
```

Deep API documentation is generated for the canonical `avelune` crate. The oracle is development
infrastructure, not an alternative public API surface.

## Browser demo and site

```sh
npm ci
./scripts/build-wasm.sh
./scripts/build-site.sh
```

`build-wasm.sh` creates separate scalar and SIMD128 modules. The demo can load a bundled sample, an
HTTP Range URL, or a local `.avl` file without uploading it. A deliberately narrow encoder lab can
also encode local 8-bit 4:2:0 Y4M video in WASM and immediately load/save the resulting `.avl`;
arbitrary MP4/WebM ingestion is not claimed without a proper demux/decode stack.

The Pages-ready site is written to `dist/site/`.

## Development and validation

```sh
./scripts/dev-check.sh
./scripts/validate-release.sh
```

The fast gate covers formatting, warnings-as-errors Clippy, workspace tests, canonical rustdoc,
script syntax, the unsafe boundary, CLI fixture decoding, WASM scalar/SIMD decoding, and adversarial
Range/local-file tests when the WASM target is available. Site checks run when the exact npm
dependency tree has been installed.

The deeper release gate adds release-mode tests, instruction-selection checks, actual Chromium WASM
execution when Chromium is available, format-design measurements, portability checks, and the
implementation lab.

## Repository map

```text
spec/                 normative Draft Generation documents
crates/avelune/       canonical safe Rust codec/container/session implementation
crates/avelune-kernels/ audited low-level SIMD/unsafe boundary
crates/avelune-reference/ independent conformance oracle
crates/avelune-wasm/  canonical browser ABI
tools/                CLI and development measurement tools
docs/                 user, architecture, browser, development, history, and IPR notes
web/                  browser player, demo fixtures, and experimental WebGPU kernels
research/             prior-art notes and format experiments
scripts/              validation, site, WASM, benchmark, and regression tooling
.github/              CI, depth testing, benchmark history, and Pages deployment
```

## Limitations

ALV1 and especially lossy ALA1 remain behind mature production codecs in compression efficiency (see the [Draft Gen 1 benchmark report](benchmarks/v1/REPORT.md)).
Lossy ALA1 is experimental and not perceptually tuned; ordinary audio encoding defaults to `q=1`.
Draft Generation 1 is intentionally unfrozen. The code has not received a professional
security or patent audit, and no patent-freedom claim is made. See [`STATUS.md`](STATUS.md) and
[`docs/IPR-NOTES.adoc`](docs/IPR-NOTES.adoc).

## License

Avelune source code is dual-licensed under the MIT License or Apache License 2.0, at your option.
Specification text and documentation are distributed under the same project license unless a file
says otherwise.
