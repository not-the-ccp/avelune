# Production backend

`prod/` now contains the optimized, stateful Draft Generation 1 backend. It remains subordinate to
Avelune's normative specifications under [`spec/`](../spec/): optimized code is never the source of
format semantics, and the [reference implementation documentation](../docs/development/REFERENCE_IMPLEMENTATION.md)
remains the guide to the readable conformance/debugging oracles.

The production workspace is split into three deliberately different trust/performance layers:

- `rust/` — safe production engine (`#![forbid(unsafe_code)]`) owning container, entropy, ALV1,
  ALA1, resource limits, reusable state, and scheduling;
- `rust/kernels/` — small audited unsafe boundary for stable architecture intrinsics only;
- `rust/wasm/` — safe handle-based incremental WebAssembly ABI; separate scalar and SIMD128
  artifacts are built for browser feature selection.

There is also `rust/lab/`, an internal machine-readable benchmark/experiment harness.

## Validated in the implementation environment

- Linux x86-64 scalar, SSE4.2 CRC, and AVX2 kernels;
- production/reference/source-separated ALV1 differential reconstruction;
- production/reference ALA1 cross-decode;
- truncation, malformed-varint, resource-limit, mutation, and arbitrary-fragmentation tests;
- scalar and SIMD128 WASM under Node;
- scalar and SIMD128 WASM instantiated and exercised in actual headless Chromium;
- browser loader HTTP Range scheduling with Rust-owned container parsing;
- CLI `--backend auto|prod|reference` integration.

AArch64 NEON/CRC source is present, but the supplied offline Rust kit does not contain the
`aarch64-unknown-linux-gnu` standard library, so neither cross-compilation nor runtime validation is
claimed here. WebGPU remains presentation/experimental work, not a validated codec backend.

Run the deeper production gate with:

```sh
./scripts/validate-prod.sh
```

The ordinary repository gate also includes production safety/WASM checks when the relevant target is
installed:

```sh
./scripts/dev-check.sh
```

See the packaged production-backend reports for measured performance, unsafe review, platform gates,
and format experiments. The backend is substantially beyond the old stub, but Draft Generation 1 is
still an unfrozen research format; "production" here names the optimized implementation layer, not a
claim that ALV1/ALA1 are mature replacements for established codecs.
