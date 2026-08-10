# Avelune V1 — common integer and entropy coding

> **Draft status:** experimental, unfrozen, and subject to incompatible change while the software is `0.x`. `V1`/`ALV1`/`ALA1` refer to Draft Generation 1, not a stable 1.0 compatibility promise.


Status: **normative draft for Draft Generation 1**. It defines the current entropy contract, but compatibility is not frozen.

This document defines byte-level primitives used by ALV1 video and ALA1 audio. Encoder model-selection heuristics are non-normative; decoding is normative.

## Integer byte order

Every fixed-width integer is little-endian unless stated otherwise.

## Canonical unsigned varint

An unsigned varint is base-128 little-endian (LEB128-style): seven payload bits per byte, bit 7 indicates another byte follows. A decoder MUST reject values that overflow `u64`, encodings longer than ten bytes, and overlong encodings (for example `80 00` for zero).

Signed 32-bit values use ZigZag mapping:

`z = (v << 1) XOR (v >> 31)` using 32-bit two's-complement semantics. Decode with `(z >> 1) XOR -(z & 1)`. The resulting unsigned value is then coded as the canonical unsigned varint.

## Entropy block

A V1-conforming encoder emits mode 0 or mode 2. Mode 1 exists only for decoding pre-release development material and is not part of the V1 conformance surface.

### Mode 0 — raw

```
u8   mode = 0
u32  raw_len
u8   raw[raw_len]
```

No trailing bytes are permitted.

### Mode 2 — static byte rANS

Constants:

- precision `B = 12`
- scale `M = 4096`
- mask `4095`
- lower normalization bound `L = 2^23`

Layout:

```
u8      mode = 2
u32     raw_len
u16     used_symbols
repeat used_symbols times:
    u8      symbol
    uvarint frequency
u32     coded_len
u8      coded[coded_len]
```

Requirements:

- `1 <= used_symbols <= 256`;
- each listed symbol is unique;
- every frequency is in `1..=4096`;
- the sum of all frequencies is exactly 4096;
- symbols not listed have frequency zero;
- the model is ordered by symbol value when produced by the reference encoder, but a decoder MUST NOT depend on order;
- `coded_len >= 4` unless `raw_len == 0` is rejected by the containing syntax before entropy decode;
- no trailing bytes are permitted.

Define cumulative frequency `C[s]` as the sum of all frequencies for symbols numerically below `s`. Build a lookup table `symbol_at[x]`, `0 <= x < 4096`, covering `[C[s], C[s]+F[s])` with symbol `s`.

The first four coded bytes are a little-endian rANS state `state`, which MUST be at least `L`. Remaining coded bytes are the renormalization byte stream in forward decoder order.

For each of `raw_len` output bytes:

1. `x = state & 4095`.
2. `s = symbol_at[x]`.
3. `state = F[s] * (state >> 12) + (x - C[s])`.
4. While `state < L`, consume one coded byte `b` and set `state = (state << 8) | b`.
5. Output `s`.

The decoder MUST consume exactly `coded_len` bytes after producing `raw_len` bytes.

## Non-normative encoder behavior

The V1 encoder normalizes observed byte frequencies to sum to 4096, uses at least frequency one for every observed symbol, and chooses raw mode when the complete rANS representation is not smaller. Other encoders may use any valid static model or choose raw mode differently.
