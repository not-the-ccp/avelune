# AGENTS.md

## Repository invariants

- `spec/` is authoritative for Draft Generation 1 semantics. Implementation code is not normative.
- `crates/avelune` is the one canonical application implementation.
- `crates/avelune-reference` is an independent conformance oracle, not a second runtime backend.
  Do not make the canonical crate depend on it or share codec reconstruction helpers with it.
- Unsafe Rust and architecture intrinsics belong only in `crates/avelune-kernels` unless a reviewed
  architectural change explicitly establishes another trust boundary.
- CLI, WASM, verification, and browser integrations should consume the same canonical
  container/session semantics rather than reimplement stream routing or epoch validation.
- Do not preserve obsolete architecture through compatibility aliases inside this young `0.x` tree.

If implementation evidence disproves a format assumption, record the evidence and revisit the
format. Do not silently change normative meaning during implementation cleanup.

## Validation

```sh
./scripts/dev-check.sh
./scripts/validate-release.sh
```

Treat environment-dependent skipped checks as skips, never passes. For browser/container changes,
exercise the adversarial Range/local-file smoke. For oracle changes, preserve implementation
independence and run differential tests.

## Code rules

- Rust 1.97.1, edition 2024.
- Keep hostile-input bounds explicit.
- Avoid dependencies that do not materially simplify a real boundary.
- Public Rust items need useful rustdoc.
- Do not claim compression, quality, performance, patent status, or browser support beyond measured
  evidence.
- Prefer deleting duplicate paths over formalizing them behind abstractions.

## Documentation

For `docs/`, `spec/`, Pages, or demo presentation, follow
`docs/development/DOCUMENT_AUTHORING.adoc` and `docs/development/WEB_STYLE.adoc` as applicable.
Keep this file lean; do not duplicate those guides here.
