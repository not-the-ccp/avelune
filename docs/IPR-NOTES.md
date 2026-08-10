# IPR / prior-art notes — Draft Generation 1

This is an engineering risk register, **not legal advice or a patent clearance opinion**. Avelune does not claim to be patent-free.

The project prefers old/openly described mechanisms where performance is comparable. If a technically superior mechanism with material IPR uncertainty is ever retained, that fact and the cost of avoiding it belong here rather than being hidden in implementation code.

| V1 mechanism | Prior-art neighborhood | V1 decision / risk note |
|---|---|---|
| fixed 8x8 Walsh-Hadamard residual transform | classical Hadamard/Walsh signal transforms | retained; simple, old mathematical transform; no codec-specific patent clearance claimed |
| translational block motion compensation | decades of predictive video coding | retained in a deliberately simple half-sample bilinear form; broad technique is old, but this register is not a freedom-to-operate opinion |
| three spatial predictors (DC/left/top) | longstanding image/video prediction | retained; deliberately minimal |
| static byte rANS | ANS/rANS literature (Jarek Duda and subsequent work) | retained after implementation testing; exact V1 model/syntax independently specified |
| exact <=4-color block palette | longstanding indexed/palette image coding and screen-content coding | retained with a simple literal per-block syntax |
| immutable explicit reference IDs | project container/codec organization | retained; avoids mutable reference-slot semantics |
| reversible lifting Haar audio transform | classical Haar/lifting construction | retained; q=1 gives exact integer reconstruction |
| reversible mid/side stereo | longstanding lossless stereo decorrelation | retained |
| CRC-32C | standardized checksum family | retained for corruption detection, not cryptographic integrity |

Pre-freeze ideas that are **not in V1** include intra-block-copy, patch dictionaries, motion meshes/lattices, lapped transforms, DCT/DST alternatives, normative loop filters, LPC/pitch speech coding, and MDCT audio. Their earlier appearance in research notes does not make them V1 dependencies.

A legal review is still appropriate before commercial deployment or any strong public licensing/patent claim.
