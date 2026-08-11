# Avelune

Avelune is an experimental, clean-slate audiovisual codec family and streaming-oriented container
written in Rust. The repository is deliberately **spec-first** and ships a complete proof of concept:
custom video, custom audio, its own indexed container, native tooling, a WASM decoder, and a browser
demo that range-fetches and decodes Avelune media itself.

> **Status:** research / POC. Nothing is frozen. The software is `0.x`; `ALV1` and `ALA1` are Draft
> Generation 1 format identifiers, not compatibility promises.

## Why this repository exists

The goal is to explore a cleaner modern media stack while keeping three things separate:

1. **normative format design** under [`spec/`](spec/);
2. **readable reference/research implementations** under [`impl/`](impl/);
3. **optimized production backends** under [`prod/`](prod/).

If an implementation experiment disproves a codec assumption, the project revisits the codec design
instead of quietly defining the format by whatever the Rust code happened to do.

## Current POC

- **ALV1 video** — 8-bit 4:2:0, mathematically lossless and lossy modes, spatial/inter prediction,
  half-sample translational motion, immutable bounded references, palette blocks, integer WHT
  residuals, and static rANS entropy coding.
- **ALA1 audio** — custom lifting-wavelet codec with reversible stereo coupling and a true lossless
  mode.
- **Avelune container** — CRC-protected packets, a front index, independently fetchable epochs, and
  incremental parsing suitable for HTTP Range playback.
- **Rust API** — component crates plus the `avelune` facade crate.
- **CLI** — encode/decode, inspect/verify, container maintenance, conformance, benchmark and fuzz-smoke
  utilities; FFmpeg is used for foreign media formats rather than reimplementing existing codecs.
- **Web** — production scalar/SIMD128 WASM streaming decoder and Range loader, reference WASM for
  conformance, WebGPU YUV presentation, and experimental WebGPU compute-kernel prototypes.
- **Conformance** — a source-separated scalar ALV1 decoder and generated expected-output vectors.

## Reference and production implementations

The readable crates under `impl/rust/crates/avelune-*-v1` remain the specification-facing
reference/research implementations and conformance oracles. The optimized stateful backend under
[`prod/`](prod/) owns its own Draft Generation 1 container/entropy/video/audio implementation, isolates
low-level intrinsics in a small kernel crate, and supplies native and browser/WASM integration.

The CLI exposes `--backend auto|prod|reference`; `auto` currently selects production for codec work,
while the reference path remains available for diagnostics and cross-checking. This does not make
production code normative, and it does not imply that the Draft Generation 1 codec design is mature.

See [`docs/development/REFERENCE_IMPLEMENTATION.adoc`](docs/development/REFERENCE_IMPLEMENTATION.adoc)
and [`prod/README.md`](prod/README.md).

## Quick start

Requirements:

- Rust 1.97.1 or newer compatible toolchain;
- FFmpeg/ffprobe for `encode`, `decode`, and `play` with ordinary media files;
- Node.js 22.12 or newer with npm for the website build and automated browser/WASM range smoke test.

```sh
cargo build --release --workspace
./target/release/avelune --help

# Encode ordinary media through FFmpeg into Avelune.
./target/release/avelune encode input.mkv output.avl

# Inspect and deeply verify a stream.
./target/release/avelune inspect output.avl
./target/release/avelune verify output.avl

# Decode back through FFmpeg to a conventional file.
./target/release/avelune decode output.avl roundtrip.mkv
```

For raw/reference workflows:

```sh
avelune raw encode-y4m input.y4m output.avl
avelune raw decode-y4m output.avl decoded.y4m
```

## Rust facade API

The Rust API is deliberately thin, experimental, and unfrozen while Avelune is `0.x`. Consume the
facade directly from this repository when experimenting:

```toml
[dependencies]
avelune = { git = "https://github.com/not-the-ccp/avelune", tag = "v0.1.1" }
```

```rust
use avelune::audio::{encode, AudioError, EncodeOptions};

fn encode_lossless() -> Result<Vec<u8>, AudioError> {
    encode(&[0_i16, 1000, -1000, 0], EncodeOptions {
        sample_rate: 48_000,
        channels: 1,
        qstep: 1,
        mid_side: false,
    })
}
```

The component crates remain available for low-level format, decoder, and conformance work.

Generate shell completions with:

```sh
avelune completions bash > avelune.bash
avelune completions zsh  > _avelune
avelune completions fish > avelune.fish
```

See the full [CLI guide](docs/user/CLI.adoc).

## Browser demo and documentation site

Build the WASM module and Pages-ready site:

```sh
npm ci
./scripts/build-wasm.sh
./scripts/build-site.sh
```

The generated site appears under `dist/site/` and contains:

- project/user/developer documentation;
- the normative draft specs;
- generated Rust API documentation;
- the static Avelune browser demo.

The repository includes GitHub Actions workflows for CI and GitHub Pages deployment.

## Development and validation

Contributions use a feature-branch and pull-request workflow. Never push directly to `main`; see
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the branch, validation, automated-review, and human-review
rules.

```sh
./scripts/dev-check.sh
```

That checks formatting, Clippy, unit/doc tests, API docs, JavaScript/Python/shell syntax, the WASM
build when the target is installed, and specification/doc-site consistency.

The deeper codec validation harness remains available through `scripts/validate-v1.sh`; despite the
historical name, it validates the current Draft Generation 1 POC rather than a frozen V1 release.

## Repository map

```text
spec/        normative draft codec/container/conformance documents
impl/rust/   safe Rust facade, reference codecs, reference decoder, CLI, WASM ABI
prod/        safe production engine, audited SIMD kernels, WASM wrapper and benchmark lab
docs/        user, architecture, browser, development, history, IPR notes
web/         static browser player and experimental WebGPU kernels
research/    prior-art notes, experiment results, rejected ideas
scripts/     validation, site, WASM, benchmark and corpus tooling
.github/     CI, Pages workflow and issue templates
```

## Versioning

While the project is `0.x`, backwards-incompatible API and bitstream changes are allowed when they
improve the design. Once a stable major line is deliberately declared, the intended rule is:
**major = incompatible API/codec/container change; minor/patch remain compatible within that major**.
See [`docs/development/VERSIONING.adoc`](docs/development/VERSIONING.adoc).

## Limitations

ALV1 and especially lossy ALA1 remain behind mature production codecs in compression efficiency.
Lossy ALA1 is experimental and not perceptually tuned; ordinary audio encoding defaults to lossless
`q=1`.
The draft baseline omits several higher-fidelity profiles. WebGPU compute is experimental. The code
has not received a professional security or patent audit. See [`STATUS.md`](STATUS.md), benchmark
reports, and [`docs/IPR-NOTES.adoc`](docs/IPR-NOTES.adoc).

## License

Avelune source code is dual-licensed under the MIT License or Apache License 2.0, at your option.
Specification text and documentation are distributed with this repository under the same project
license unless a file says otherwise.
