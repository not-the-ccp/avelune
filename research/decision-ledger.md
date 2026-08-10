# Avelune design decision ledger

## V1 accepted

- **D-0001 — Specification is authoritative.** Rust implementations are not normative.
- **D-0002 — Immutable explicit frame dependencies.** Frame headers list IDs; no mutable reference slots.
- **D-0003 — Epoch isolation.** Reference state resets at indexed epoch boundaries.
- **D-0004 — Web threads are optional.** Single-thread WASM is a correctness baseline.
- **D-0005 — Non-lapped integer transform baseline.** V1 uses fixed 8x8 WHT.
- **D-0006 — Static byte rANS.** Accepted with self-contained frame/plane-local models and canonical compact frequency headers.
- **D-0007 — Adaptive entropy layout.** Each video plane writes mixed or control/data-separated streams, whichever is smaller.
- **D-0008 — Screen palette competes with prediction.** Palette is exact but never privileged over a cheap temporal/spatial prediction.
- **D-0009 — Bounded multi-reference search.** Syntax permits four immutable dependencies; default encoder uses one, quality preset may search four.
- **D-0010 — Scene cuts are encoder policy.** Conservative hard cuts may start new epochs; no syntax dependency.
- **D-0011 — ALA1 uses reversible lifting Haar.** q=1 is true mathematical lossless mode; lossy scalar quantization is intentionally simple.
- **D-0012 — WebGPU is hybrid/optional.** Presentation supported, compute prototypes for bulk work, never bitstream-required.

## V1 rejected experiments

- lapped transforms as the baseline;
- explicit inter-skip symbols (measured slightly larger than existing rANS tokenization);
- dedicated low/high coefficient entropy lanes (model overhead exceeded gain);
- quarter-pixel motion for V1 (insufficient or negative rate/quality value in tested corpus);
- motion-vector delta coding (static rANS compressed raw vectors better in the tested syntax);
- partial luma-only inter RDO estimator (rate regression);
- making WebGPU, WASM threads, or SIMD mandatory for correctness.

Rejected ideas may be revisited when new evidence justifies doing so. Draft Generation 1 is intentionally unfrozen, so incompatible syntax changes remain possible while they are documented and accompanied by updated conformance material.
