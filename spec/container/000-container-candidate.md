> HISTORICAL / NON-NORMATIVE: superseded by the Draft Generation 1 `001-*` specification.

# Avelune Container Candidate 0.0

Status: **EXPERIMENTAL / NOT FROZEN**

## Purpose

The container is designed for bounded-memory incremental parsing, HTTP byte-range seeking, deterministic A/V epoch restart, and straightforward inspection/repair.

## Integer encoding

Unless specified otherwise, fixed-width integers are unsigned little-endian. Variable unsigned integers use unsigned LEB128 with a maximum of ten bytes. Decoders MUST reject overlong encodings and values outside the field's stated range.

## File structure

A file is a sequence:

1. fixed file header;
2. stream descriptors;
3. optional front index reservation/data;
4. zero or more epochs;
5. optional final index;
6. optional footer.

The candidate implementation currently exercises the packet/epoch rules before the full front-index serialization is frozen.

## Fixed header candidate

```
magic[8] = 41 56 45 4c 55 4e 45 00   # "AVELUNE\0"
container_major: u16
container_minor: u16
flags: u32
header_bytes: u32
stream_count: u16
reserved: u16 = 0
```

A decoder MUST reject unknown major versions. It MAY accept a newer minor version only when all encountered structures are explicitly skippable.

## Packet envelope

Every packet is self-bounded:

```
sync: u32 = 0x4B505641   # bytes "AVPK"
kind: u8
flags: u8
stream_id: u16
pts: u64
payload_len: u32
header_crc32c: u32       # candidate; algorithm not frozen
payload[payload_len]
payload_crc32c: u32      # candidate
```

`payload_len` MUST be checked against implementation resource limits before allocation. A conforming decoder profile SHALL publish its maximum accepted packet size.

## Epochs

An epoch begins with an `EPOCH_START` packet. All media dependency state required by media packets in that epoch MUST either be contained in the epoch or be declared static stream configuration.

Video reference pictures MUST NOT cross epochs. Audio adaptive/predictive state MUST reset at an epoch boundary.

This property is normative even if the exact epoch packet payload changes during candidate development.

## Time

Each stream descriptor defines a rational time base. Packet `pts` is expressed in that stream time base. Integer timestamp arithmetic MUST be used for demux synchronization; floating-point conversion is presentation-layer behavior.

## Streaming requirement

A conforming demuxer MUST be able to accept arbitrary non-empty byte fragments. It MUST NOT require the complete file to determine packet boundaries after the required header/descriptor bytes have arrived.

## Future index contract

The frozen format will provide an index mapping presentation-time intervals to byte offsets of independently decodable epochs. Finalized files should carry a front index; live streams may omit it. The index representation is intentionally not frozen in Candidate 0.0.
