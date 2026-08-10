# Contributing

Avelune is experimental and welcomes design criticism, independent decoder attempts, test vectors,
codec research, documentation fixes, and implementation work.

## Start here

- [`STATUS.md`](STATUS.md) — what exists and what is not stable;
- [`spec/README.md`](spec/README.md) — normative-draft hierarchy;
- [`docs/development/REFERENCE_IMPLEMENTATION.md`](docs/development/REFERENCE_IMPLEMENTATION.md) —
  what the Rust implementation is for;
- [`docs/development/VERSIONING.md`](docs/development/VERSIONING.md) — compatibility policy;
- [`AGENTS.md`](AGENTS.md) — concise repository workflow for coding agents and humans.

## Development

Rust 1.97.1 is the reference toolchain. Then run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --doc
cargo doc --workspace --no-deps
```

`./scripts/dev-check.sh` runs the supported repository checks in one command.

## Format changes

A syntax or decoded-semantics change is not complete until the draft spec, implementation(s),
conformance tests/vectors, changelog, and affected docs agree. During `0.x`, backwards compatibility
is not a goal by itself; a cleaner design can justify an incompatible change.

## Pull requests

Keep changes focused. Include the evidence that motivated codec-design changes and report failed
experiments as well as successful ones. Do not present reference-implementation throughput as the
architectural limit of the format.
