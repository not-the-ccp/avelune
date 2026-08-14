use super::parser::{PacketView, decode_packet_view_with};
use super::*;

/// Computes the CRC-32C checksum used by the container with runtime-selected kernels.
pub fn crc32c(data: &[u8]) -> u32 {
    avelune_kernels::KernelSet::auto().crc32c(data)
}

/// Computes CRC-32C with an explicitly selected kernel set.
pub fn crc32c_with(kernels: avelune_kernels::KernelSet, data: &[u8]) -> u32 {
    kernels.crc32c(data)
}

/// Appends a fixed file header to `out`.
pub fn encode_header(h: &FileHeader, out: &mut Vec<u8>) {
    let s = out.len();
    out.extend(FILE_MAGIC);
    out.extend(1u16.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(h.flags.to_le_bytes());
    out.extend((FIXED_HEADER_LEN as u32).to_le_bytes());
    out.extend(h.stream_count.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(h.front_len.to_le_bytes());
    let c = crc32c(&out[s..s + 28]);
    out.extend(c.to_le_bytes());
}
/// Parses and validates a fixed file header.
pub fn decode_header(b: &[u8]) -> Result<FileHeader, ContainerError> {
    if b.len() < FIXED_HEADER_LEN {
        return Err(ContainerError::UnexpectedEof);
    }
    if b[..8] != FILE_MAGIC {
        return Err(ContainerError::BadMagic);
    }
    let maj = u16::from_le_bytes([b[8], b[9]]);
    let min = u16::from_le_bytes([b[10], b[11]]);
    if maj != 1 {
        return Err(ContainerError::UnsupportedVersion(maj));
    }
    if min != 0
        || u32::from_le_bytes(b[12..16].try_into().unwrap()) & !1 != 0
        || u32::from_le_bytes(b[16..20].try_into().unwrap()) != FIXED_HEADER_LEN as u32
        || u16::from_le_bytes([b[22], b[23]]) != 0
    {
        return Err(ContainerError::BadHeader);
    }
    if crc32c(&b[..28]) != u32::from_le_bytes(b[28..32].try_into().unwrap()) {
        return Err(ContainerError::HeaderChecksum);
    }
    Ok(FileHeader {
        flags: u32::from_le_bytes(b[12..16].try_into().unwrap()),
        stream_count: u16::from_le_bytes(b[20..22].try_into().unwrap()),
        front_len: u32::from_le_bytes(b[24..28].try_into().unwrap()),
    })
}

/// Serializes a front index.
pub fn encode_front(front: &Front) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend(FRONT_MAGIC);
    o.extend(1u16.to_le_bytes());
    o.extend((front.streams.len() as u16).to_le_bytes());
    o.extend((front.epochs.len() as u32).to_le_bytes());
    for s in &front.streams {
        o.extend(s.id.to_le_bytes());
        o.push(s.kind as u8);
        o.push(s.codec);
        o.extend(s.timescale.to_le_bytes());
        o.extend(s.param0.to_le_bytes());
        o.extend(s.param1.to_le_bytes());
        o.extend(s.flags.to_le_bytes());
        o.extend(s.meta0.to_le_bytes());
    }
    for e in &front.epochs {
        o.extend(e.id.to_le_bytes());
        o.extend(e.duration.to_le_bytes());
        o.extend(e.pts.to_le_bytes());
        o.extend(e.offset.to_le_bytes());
        o.extend(e.len.to_le_bytes());
    }
    o.extend(crc32c(&o).to_le_bytes());
    o
}
/// Parses and validates a front index for the expected stream count.
pub fn decode_front(b: &[u8], expected_streams: u16) -> Result<Front, ContainerError> {
    if b.len() < 16 {
        return Err(ContainerError::UnexpectedEof);
    }
    if b[..4] != FRONT_MAGIC {
        return Err(ContainerError::BadFront);
    }
    if u16::from_le_bytes([b[4], b[5]]) != 1 {
        return Err(ContainerError::BadFront);
    }
    let sc = u16::from_le_bytes([b[6], b[7]]) as usize;
    let ec = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    if sc != expected_streams as usize {
        return Err(ContainerError::BadFront);
    }
    let stream_bytes = sc.checked_mul(24).ok_or(ContainerError::BadFront)?;
    let epoch_bytes = ec.checked_mul(32).ok_or(ContainerError::BadFront)?;
    let need = 12usize
        .checked_add(stream_bytes)
        .and_then(|x| x.checked_add(epoch_bytes))
        .and_then(|x| x.checked_add(4))
        .ok_or(ContainerError::BadFront)?;
    if b.len() != need {
        return Err(if b.len() < need {
            ContainerError::UnexpectedEof
        } else {
            ContainerError::TrailingData
        });
    }
    let expected = u32::from_le_bytes(b[need - 4..need].try_into().unwrap());
    if crc32c(&b[..need - 4]) != expected {
        return Err(ContainerError::FrontChecksum);
    }
    let mut p = 12;
    let mut streams = Vec::with_capacity(sc);
    let mut stream_ids = std::collections::BTreeSet::new();
    for _ in 0..sc {
        let id = u16::from_le_bytes([b[p], b[p + 1]]);
        if !stream_ids.insert(id) {
            return Err(ContainerError::BadFront);
        }
        let kind = StreamKind::try_from(b[p + 2])?;
        let codec = b[p + 3];
        let timescale = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap());
        let param0 = u32::from_le_bytes(b[p + 8..p + 12].try_into().unwrap());
        let param1 = u32::from_le_bytes(b[p + 12..p + 16].try_into().unwrap());
        let flags = u32::from_le_bytes(b[p + 16..p + 20].try_into().unwrap());
        let meta0 = u32::from_le_bytes(b[p + 20..p + 24].try_into().unwrap());
        p += 24;
        // Draft Generation 1 codec 1 defines these metadata fields normatively. Keep the
        // generic container able to carry future codec IDs, but reject malformed V1 metadata.
        if codec == 1 {
            match kind {
                StreamKind::Video => {
                    VideoMeta::unpack(meta0)?;
                }
                StreamKind::Audio if flags != 0 || meta0 != 0 => {
                    return Err(ContainerError::BadFront);
                }
                StreamKind::Audio => {}
            }
        }
        streams.push(StreamDesc {
            id,
            kind,
            codec,
            timescale,
            param0,
            param1,
            flags,
            meta0,
        });
    }
    let mut epochs = Vec::with_capacity(ec);
    let mut epoch_ids = std::collections::BTreeSet::new();
    for _ in 0..ec {
        let id = u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
        if !epoch_ids.insert(id) {
            return Err(ContainerError::BadFront);
        }
        let duration = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap());
        let pts = u64::from_le_bytes(b[p + 8..p + 16].try_into().unwrap());
        let offset = u64::from_le_bytes(b[p + 16..p + 24].try_into().unwrap());
        let len = u64::from_le_bytes(b[p + 24..p + 32].try_into().unwrap());
        if offset.checked_add(len).is_none() {
            return Err(ContainerError::BadFront);
        }
        p += 32;
        epochs.push(EpochIndex {
            id,
            pts,
            offset,
            len,
            duration,
        });
    }
    Ok(Front { streams, epochs })
}

pub(super) type StreamIndex = std::collections::HashMap<u16, usize>;
const MAX_INDEXED_STREAMS: usize = u16::MAX as usize;

pub(super) fn build_stream_index(streams: &[StreamDesc]) -> Result<StreamIndex, ContainerError> {
    // The on-wire stream count is u16 and incremental parsing additionally bounds the complete
    // front index by the configured stream-buffer ceiling.
    if streams.len() > MAX_INDEXED_STREAMS {
        return Err(ContainerError::ResourceLimit);
    }
    let mut index = StreamIndex::with_capacity(streams.len());
    for (position, stream) in streams.iter().enumerate() {
        if index.insert(stream.id, position).is_some() {
            return Err(ContainerError::BadFront);
        }
    }
    Ok(index)
}

pub(super) fn packet_stream_indexed<'a>(
    front: &'a Front,
    index: &StreamIndex,
    packet: PacketView<'_>,
) -> Result<Option<&'a StreamDesc>, ContainerError> {
    let expected_kind = match packet.kind {
        PacketKind::VideoFrame => Some(StreamKind::Video),
        PacketKind::AudioFrame => Some(StreamKind::Audio),
        PacketKind::EpochStart | PacketKind::Metadata => None,
    };
    if let Some(expected) = expected_kind {
        let position = *index
            .get(&packet.stream_id)
            .ok_or(ContainerError::BadStream)?;
        let stream = front
            .streams
            .get(position)
            .ok_or(ContainerError::BadFront)?;
        if stream.kind != expected {
            return Err(ContainerError::BadStream);
        }
        Ok(Some(stream))
    } else {
        Ok(None)
    }
}

/// Appends one framed, checksummed packet after validating fields that cannot be represented
/// safely by the Draft Generation 1 packet header.
pub fn encode_packet_checked(p: &Packet, out: &mut Vec<u8>) -> Result<(), ContainerError> {
    if p.flags != 0 {
        return Err(ContainerError::BadHeader);
    }
    let payload_len = u32::try_from(p.payload.len()).map_err(|_| ContainerError::ResourceLimit)?;
    let s = out.len();
    out.extend(PACKET_MAGIC);
    out.push(p.kind as u8);
    out.push(p.flags);
    out.extend(p.stream_id.to_le_bytes());
    out.extend(p.pts.to_le_bytes());
    out.extend(p.duration.to_le_bytes());
    out.extend(payload_len.to_le_bytes());
    out.extend(crc32c(&out[s..s + 24]).to_le_bytes());
    out.extend(&p.payload);
    out.extend(crc32c(&p.payload).to_le_bytes());
    Ok(())
}

/// Appends one framed, checksummed packet. This convenience API assumes the packet fields were
/// already validated; checked muxing should prefer [`encode_packet_checked`].
pub fn encode_packet(p: &Packet, out: &mut Vec<u8>) {
    let s = out.len();
    out.extend(PACKET_MAGIC);
    out.push(p.kind as u8);
    out.push(p.flags);
    out.extend(p.stream_id.to_le_bytes());
    out.extend(p.pts.to_le_bytes());
    out.extend(p.duration.to_le_bytes());
    out.extend((p.payload.len() as u32).to_le_bytes());
    out.extend(crc32c(&out[s..s + 24]).to_le_bytes());
    out.extend(&p.payload);
    out.extend(crc32c(&p.payload).to_le_bytes());
}
/// Parses one packet and returns an owned packet together with the consumed byte count.
pub fn decode_packet(b: &[u8], max_packet: usize) -> Result<(Packet, usize), ContainerError> {
    let (view, n) = decode_packet_view_with(b, max_packet, avelune_kernels::KernelSet::auto())?;
    Ok((view.to_owned(), n))
}

/// Parses the fixed header plus complete front index from a file prefix.
pub fn parse_file_prefix(b: &[u8]) -> Result<(FileHeader, Front, usize), ContainerError> {
    let h = decode_header(b)?;
    let end = FIXED_HEADER_LEN
        .checked_add(h.front_len as usize)
        .ok_or(ContainerError::BadHeader)?;
    if b.len() < end {
        return Err(ContainerError::UnexpectedEof);
    }
    let f = decode_front(&b[FIXED_HEADER_LEN..end], h.stream_count)?;
    Ok((h, f, end))
}

/// Validates complete indexed epoch byte ranges, including every packet CRC/framing boundary.
/// The first packet must be the matching EpochStart and no second EpochStart may appear inside
/// the same indexed range.
pub fn validate_epoch_ranges(epochs: &[(u32, u64, u32, Vec<u8>)]) -> Result<(), ContainerError> {
    let mut seen_ids = std::collections::BTreeSet::new();
    for (id, _pts, _duration, bytes) in epochs {
        if bytes.is_empty() || !seen_ids.insert(*id) {
            return Err(ContainerError::BadEpoch);
        }
        let mut pos = 0usize;
        let mut first = true;
        while pos < bytes.len() {
            let (packet, used) = decode_packet_view_with(
                &bytes[pos..],
                DEFAULT_MAX_PACKET,
                avelune_kernels::KernelSet::auto(),
            )?;
            if used == 0 {
                return Err(ContainerError::BadEpoch);
            }
            if first {
                if packet.kind != PacketKind::EpochStart
                    || packet.payload.len() != 4
                    || u32::from_le_bytes(packet.payload.try_into().unwrap()) != *id
                {
                    return Err(ContainerError::BadEpoch);
                }
                first = false;
            } else if packet.kind == PacketKind::EpochStart {
                return Err(ContainerError::BadEpoch);
            }
            pos = pos.checked_add(used).ok_or(ContainerError::BadEpoch)?;
        }
        if first || pos != bytes.len() {
            return Err(ContainerError::BadEpoch);
        }
    }
    Ok(())
}

fn validate_mux_streams(
    streams: &[StreamDesc],
    epochs: &[(u32, u64, u32, Vec<u8>)],
) -> Result<(), ContainerError> {
    let stream_index = build_stream_index(streams)?;
    let front = Front {
        streams: streams.to_vec(),
        epochs: Vec::new(),
    };
    for (_, _, _, bytes) in epochs {
        let mut pos = 0usize;
        while pos < bytes.len() {
            let (packet, used) = decode_packet_view_with(
                &bytes[pos..],
                DEFAULT_MAX_PACKET,
                avelune_kernels::KernelSet::auto(),
            )?;
            packet_stream_indexed(&front, &stream_index, packet)?;
            pos = pos.checked_add(used).ok_or(ContainerError::BadEpoch)?;
        }
    }
    Ok(())
}

/// Checked builder used by application integrations.
pub fn build_file_checked(
    streams: Vec<StreamDesc>,
    epochs: Vec<(u32, u64, u32, Vec<u8>)>,
) -> Result<Vec<u8>, ContainerError> {
    if streams.len() > u16::MAX as usize || epochs.len() > u32::MAX as usize {
        return Err(ContainerError::ResourceLimit);
    }
    validate_epoch_ranges(&epochs)?;
    validate_mux_streams(&streams, &epochs)?;
    Ok(build_file(streams, epochs))
}

/// Build a front-indexed Draft Generation 1 file from already packetized epochs.
/// Builds a complete file from stream descriptors and pre-encoded epoch byte ranges.
pub fn build_file(streams: Vec<StreamDesc>, epochs: Vec<(u32, u64, u32, Vec<u8>)>) -> Vec<u8> {
    let dummy: Vec<EpochIndex> = epochs
        .iter()
        .map(|(id, pts, dur, _)| EpochIndex {
            id: *id,
            pts: *pts,
            duration: *dur,
            offset: 0,
            len: 0,
        })
        .collect();
    let front0 = encode_front(&Front {
        streams: streams.clone(),
        epochs: dummy,
    });
    let base = (FIXED_HEADER_LEN + front0.len()) as u64;
    let mut off = base;
    let mut idx = Vec::new();
    for (id, pts, dur, bytes) in &epochs {
        idx.push(EpochIndex {
            id: *id,
            pts: *pts,
            duration: *dur,
            offset: off,
            len: bytes.len() as u64,
        });
        off += bytes.len() as u64;
    }
    let front = encode_front(&Front {
        streams: streams.clone(),
        epochs: idx,
    });
    let mut out = Vec::with_capacity(off as usize);
    encode_header(
        &FileHeader {
            flags: 1,
            stream_count: streams.len() as u16,
            front_len: front.len() as u32,
        },
        &mut out,
    );
    out.extend(front);
    for (_, _, _, e) in epochs {
        out.extend(e)
    }
    out
}
