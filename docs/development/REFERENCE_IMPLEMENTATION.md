# Reference implementation policy

The Rust implementation under `impl/rust/` is a reference/research implementation.

Its priorities are:

1. match the normative specification;
2. be readable enough to audit against the specification;
3. reject malformed input deterministically and safely;
4. support conformance and experimentation;
5. remain reasonably usable as a POC.

Peak encode/decode performance is **not** a design requirement for this implementation. Do not add
SIMD, threading, GPU-specific code, aggressive buffering, or opaque performance tricks merely to
make benchmark numbers look better. Such work belongs in `prod/` unless an optimization is both
semantics-preserving and clearly improves the reference implementation without obscuring it.

If implementation experience disproves a codec assumption, revisit the format design. Do not change
the spec just to mirror an implementation accident, and do not silently work around an architectural
format flaw in code.
