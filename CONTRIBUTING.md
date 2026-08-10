# Contributing

Avelune is experimental and welcomes design criticism, independent decoder attempts, test vectors,
codec research, documentation fixes, and implementation work.

## Contribution workflow

Use the same review-first workflow for code, specification, documentation, tests, and tooling
changes:

1. Start from an up-to-date `main` branch and create a focused feature branch. For example:

   ```sh
   git switch main
   git pull --ff-only origin main
   git switch -c docs/clarify-contributing-workflow
   ```

2. Make the change on that branch, committing small coherent milestones as you go rather than
   waiting until the entire task is complete. Run the narrowest relevant checks, followed by
   `./scripts/dev-check.sh` for repository-wide changes.
3. Push the feature branch and open a pull request targeting `main`:

   ```sh
   git push --set-upstream origin docs/clarify-contributing-workflow
   gh pr create --base main --fill
   ```

4. Wait for CI and automated review. Resolve every CodeRabbit comment or piece of feedback; either
   make the requested change or leave a clear explanation when the suggestion is not applicable.
   Do not leave review feedback unresolved merely because CI is green.
5. Wait for the appropriate review before merging. Ordinary changes may proceed once CI passes and
   automated feedback is handled. Large, security-sensitive, architectural, release, or
   spec/format changes also require explicit human review.

CodeRabbit is the repository's automated reviewer. If CodeRabbit does not run or is unavailable,
treat a human review as the fallback for the pull request.

Never push directly to `main`, force-push it, or bypass the pull-request workflow. The repository's
GitHub ruleset enforces pull requests and required CI for `main`; there is no documented typo
exception. Small fixes use the same workflow.

Contributors and coding agents should stop after opening the pull request and report its URL,
validation results, and any review questions. Do not merge your own pull request unless the project
maintainer explicitly asks you to do so.

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

Keep changes focused and use the pull-request template. Include the evidence that motivated
codec-design changes and report failed experiments as well as successful ones. Do not present
reference-implementation throughput as the architectural limit of the format.
