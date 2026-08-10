# Production implementation stub

`prod/` is reserved for optimized Avelune implementations. It is intentionally a stub.
The code under `impl/rust/crates/avelune-*-v1` is the **reference/research implementation**;
it prioritizes specification clarity, differential testing, and debuggability over production
throughput.

A production backend may use SIMD, threads, GPU compute, specialized allocation strategies,
or different internal data structures, but it must not define codec behavior. Normative decoded
semantics live under [`spec/`](../spec/), and conformance must be checked against the vectors and
source-separated reference decoder.

Before this directory grows into a real implementation, write a backend design that covers:

- native CPU scalar/SIMD dispatch;
- WASM SIMD and optional threading;
- WebGPU compute boundaries and synchronization costs;
- incremental/container streaming integration;
- bounded memory and hostile-input behavior;
- differential conformance against the reference implementation;
- benchmark methodology on sustained real media.

If optimized backends later need their own release cadence or platform-specific maintainers, they
can move to separate repositories without splitting the normative spec/reference hub prematurely.
