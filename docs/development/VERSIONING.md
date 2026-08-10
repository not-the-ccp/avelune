# Versioning and compatibility

Avelune currently has **no stable codec, container, or public API**. The public software tree uses
`0.x` versions until the project intentionally declares a stable compatibility line.

Two version axes are deliberately separate:

1. **Software version** — Cargo crates, CLI, web player, and tooling. During `0.x`, incompatible API
   changes may occur in a minor release.
2. **Format generation** — identifiers such as `ALV1` and `ALA1`. “1” currently means Draft
   Generation 1; it does **not** mean that the format has been frozen or promised permanent
   compatibility.

Once a stable major line is declared, the intended policy is:

- patch: compatible fixes only;
- minor: compatible features and encoder improvements; existing valid streams remain decodable;
- major: incompatible public API, codec/container syntax, or decoded-semantics change.

A new stable major may replace the previous one or coexist with it. Long-term maintenance of old
major lines will be decided explicitly rather than promised in advance.

Until stabilization, format-breaking changes must update the spec, conformance vectors, decoder(s),
CLI/docs, and changelog together. Do not preserve draft compatibility merely because an earlier POC
happened to emit that syntax.
