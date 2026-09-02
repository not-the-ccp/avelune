# Support

Avelune is a research/POC project. Bug reports and reproducible test cases are useful; production SLA,
compatibility guarantees, and real-time performance are not currently offered.

## Where to report

Use the [GitHub issue chooser](https://github.com/not-the-ccp/avelune/issues/new/choose) for bugs,
reproducible media problems, feature requests, and project questions. Search existing issues first when
possible so results and workarounds stay in one place.

Do not report a suspected vulnerability as a public issue. Follow [`SECURITY.md`](SECURITY.md) instead.

## What to include

When reporting a media problem, include:

- `avelune --version`;
- exact command line or browser steps;
- `ffprobe` summary of the source where relevant;
- platform and browser;
- whether the issue reproduces with a short redistributable sample;
- for browser conversion problems, whether OPFS was available and whether scalar or SIMD128 WASM was selected;
- for performance reports, resolution, frame rate, clip duration, preset/quantizers, elapsed time,
  CPU/GPU, and whether the path is native Rust, browser/WASM, or experimental WebGPU compute.
