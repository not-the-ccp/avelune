# Prior-art reading notes — initial pass

These notes are non-normative and exist to prevent accidental reinvention. Every adopted mechanism still needs an independent design rationale and, where appropriate, IPR review.

## AV1 / AV2

Primary references:

- AV1 bitstream/decoding specification: https://aomediacodec.github.io/av1-spec/
- AV2 specification: https://av2.aomedia.org/

Takeaways for this project:

- mature video codecs accumulate many individually useful tools; complexity has to be judged as a system cost, not only per-tool BD-rate gain;
- tiles/regions and normative filtering boundaries affect parallelism and independence in non-obvious ways;
- screen-content coding deserves explicit mechanisms rather than assuming camera-oriented residual coding will cover it;
- AV2's separation between normative specification and AVM reference software is a useful process model.

## ANS / rANS

Primary paper:

- Jarek Duda, "Asymmetric numeral systems: entropy coding combining speed of Huffman coding with compression rate of arithmetic coding", https://arxiv.org/abs/1311.2540

Related interleaving paper:

- Fabian Giesen, "Interleaved entropy coders", https://arxiv.org/abs/1402.3392

Takeaway: rANS is promising specifically because decode throughput and state interleaving can align well with region-level parallelism. This is still an experiment: measured rate, table overhead, context selection cost and WASM performance decide whether it survives.

## Opus / CELT / SILK

Primary standard:

- RFC 6716: https://datatracker.ietf.org/doc/html/rfc6716

Takeaway: a single mechanism does not automatically cover both speech and music well. Opus explicitly combines LP-based and MDCT-based approaches. Our cleaner "optional prediction/whitening feeding one transform path" is therefore a hypothesis to test, not an aesthetic principle to defend.

## Daala

Xiph development material remains useful as a record of experimental ideas, particularly lapped transforms and frequency-domain intra prediction. The lesson is not "lapping is bad"; it is that a local coding gain can create expensive dependencies elsewhere in prediction/partition design.
