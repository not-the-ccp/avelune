> HISTORICAL / NON-NORMATIVE: superseded by the Draft Generation 1 `001-*` specification.

# Candidate conformance requirements

Status: EXPERIMENTAL.

A decoder must eventually be implementable from specification documents alone. Candidate implementations are tested for:

- deterministic reconstruction;
- exact lossless round trips;
- rejection of truncated and overlong varints;
- bounded allocation from attacker-controlled lengths;
- rejection of impossible dimensions and arithmetic overflow;
- epoch dependency isolation;
- no panic for malformed input at public decoding boundaries;
- reference/optimized decoder agreement once both exist.

Every discovered malformed-stream bug gets a permanent regression vector.
