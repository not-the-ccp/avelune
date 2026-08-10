# Security policy

Avelune is experimental media software that parses attacker-controlled binary data. Treat malformed
input handling as security-sensitive even though the project is not yet production-ready.

## Reporting

For a public repository, use the hosting platform's private vulnerability-reporting feature when
available. Until a maintainer-specific security address is configured, do not publish exploit details
before maintainers have had a reasonable chance to assess the issue.

Useful reports include the affected commit/version, minimal reproducer, expected vs actual behavior,
and whether the issue reproduces in the scalar reference decoder, production stub, browser/WASM path,
or container parser.

## Current security posture

Reference crates forbid unsafe Rust. Parsers enforce explicit size limits and the repository contains
malformed-input and mutation smoke tests. This is **not** a claim that the codecs have received a
professional security audit or sustained coverage-guided fuzzing campaign.
