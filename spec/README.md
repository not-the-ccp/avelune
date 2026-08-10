# Avelune specification drafts

**Status: experimental and unfrozen.**

The normative draft documents in this directory define current decoded behavior. They are the source
of truth when they disagree with the reference implementation. The project is still `0.x`, so syntax,
identifiers, constraints, and decoded semantics may change incompatibly when evidence supports a
better design.

`ALV1` and `ALA1` mean **Draft Generation 1**. They do not mean the format has reached stable 1.0.

## Current normative draft

- [`001-v1-baseline-profile.md`](001-v1-baseline-profile.md) — common baseline/profile boundaries;
- [`common/001-entropy-v1.md`](common/001-entropy-v1.md) — canonical varints and static rANS;
- [`container/001-container-v1.md`](container/001-container-v1.md) — file/front-index/packet syntax;
- [`video/001-video-v1.md`](video/001-video-v1.md) — ALV1 decoded semantics;
- [`audio/001-audio-v1.md`](audio/001-audio-v1.md) — ALA1 decoded semantics;
- [`conformance/001-v1.md`](conformance/001-v1.md) — decoder/conformance expectations.

Files prefixed `000-` are historical experiments and are non-normative.

## Change discipline

A format change is complete only when the normative draft, affected implementations, conformance
vectors/tests, public docs, and `spec/CHANGELOG.md` agree. During the unstable phase, do not retain an
inferior design solely to decode an earlier development artifact.

See [`../docs/development/VERSIONING.md`](../docs/development/VERSIONING.md).
