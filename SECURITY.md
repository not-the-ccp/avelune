# Security policy

Avelune is experimental media software that parses attacker-controlled binary data. Treat malformed
input handling as security-sensitive even though the project is not yet production-ready.

## Reporting

For a public repository, use the hosting platform's private vulnerability-reporting feature when
available. Until a maintainer-specific security address is configured, do not publish exploit details
before maintainers have had a reasonable chance to assess the issue.

Useful reports include the affected commit/version, a minimal reproducer, expected versus actual
behavior, and the affected path: canonical native Rust, independent conformance oracle,
browser/WASM, embedded-FFmpeg import boundary, or container parser.

## Current security posture

The canonical implementation is safe Rust; unsafe/intrinsic code is isolated in the kernel crate.
Parsers enforce explicit size limits and the repository contains malformed-input and mutation tests.
The independent reference implementation provides differential coverage for codec semantics.

The codecs have not received a professional security audit or a sustained coverage-guided fuzzing
campaign suitable for a production-security claim.
