# Contributing

Avelune is experimental. Contributions are welcome across codec research, implementation, independent
decoders, test material, documentation, and tooling.

## Contribution workflow

1. Start from an up-to-date `main` branch and create a focused feature branch.

   ```sh
   git switch main
   git pull --ff-only origin main
   git switch -c my-change
   ```

2. Make the change and run the checks that cover it. Use `./scripts/dev-check.sh` for changes that
   affect multiple parts of the repository.
3. Push the branch and open a pull request targeting `main`.

   ```sh
   git push --set-upstream origin my-change
   gh pr create --base main --fill
   ```

4. Address CI failures and review feedback before merging. Large format, architectural, release, or
   security-sensitive changes require explicit human review.

Do not push or force-push directly to `main`. Repository rules require the pull-request and CI path.

## Start here

- [`STATUS.md`](STATUS.md) — implemented scope and known limitations;
- [`spec/README.adoc`](spec/README.adoc) — specification structure and reading order;
- [`docs/development/REFERENCE_ORACLE.adoc`](docs/development/REFERENCE_ORACLE.adoc) — conformance oracle design;
- [`docs/development/VERSIONING.adoc`](docs/development/VERSIONING.adoc) — compatibility policy;
- [`docs/development/TESTING.adoc`](docs/development/TESTING.adoc) — validation strategy.

## Development

Rust 1.97.1 is the reference toolchain. The main Rust checks are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --doc
cargo doc -p avelune --no-deps
```

`./scripts/dev-check.sh` runs the repository-wide development checks, including browser/site checks
when the required local dependencies are available.

## Format changes

A change to syntax or decoded semantics is not complete until the draft specification, affected
implementations, conformance tests/vectors, changelog, and public documentation agree. During `0.x`,
backwards compatibility is not by itself a reason to retain an inferior draft design.

Codec-design changes should include the evidence that motivated them, including failed experiments
when those results affect the decision. Performance results should identify the implementation,
hardware, input, settings, and measurement method rather than treating one reference implementation
as the limit of the format.
