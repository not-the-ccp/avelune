//! Production-owned Avelune Draft Generation 1 container implementation.
//!
//! The slice parser borrows payloads; the streaming parser uses a read cursor and amortized
//! compaction instead of draining the front of a `Vec` after every packet.
//!
//! The container is front-indexed, packetized, CRC-protected, and designed so epochs can
//! be fetched independently with HTTP byte ranges. The normative draft lives under `spec/`.

/// Eight-byte file magic.
pub const FILE_MAGIC: [u8; 8] = *b"AVELUNE\0";
/// Front-index section magic.
pub const FRONT_MAGIC: [u8; 4] = *b"AVFR";
/// Packet framing magic.
pub const PACKET_MAGIC: [u8; 4] = *b"AVPK";
/// Fixed file-header length in bytes.
pub const FIXED_HEADER_LEN: usize = 32;
/// Fixed packet-header length in bytes.
pub const PACKET_HEADER_LEN: usize = 28;
/// Packet trailer length in bytes.
pub const PACKET_TRAILER_LEN: usize = 4;
/// Default defensive maximum packet size used by tools.
pub const DEFAULT_MAX_PACKET: usize = 128 * 1024 * 1024;
/// Default container timebase: microseconds per second.
pub const TIMEBASE: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// Stream media category.
pub enum StreamKind {
    Video = 1,
    Audio = 2,
}
impl TryFrom<u8> for StreamKind {
    type Error = ContainerError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(Self::Video),
            2 => Ok(Self::Audio),
            _ => Err(ContainerError::BadFront),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// Packet semantic category.
pub enum PacketKind {
    EpochStart = 1,
    VideoFrame = 2,
    AudioFrame = 3,
    Metadata = 4,
}
impl TryFrom<u8> for PacketKind {
    type Error = ContainerError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(Self::EpochStart),
            2 => Ok(Self::VideoFrame),
            3 => Ok(Self::AudioFrame),
            4 => Ok(Self::Metadata),
            _ => Err(ContainerError::UnknownPacketKind(v)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// Signaled YUV matrix coefficients.
pub enum ColorMatrix {
    Unspecified = 0,
    Bt601 = 1,
    Bt709 = 2,
    Bt2020Ncl = 3,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// Signaled transfer characteristic.
pub enum ColorTransfer {
    Unspecified = 0,
    Bt709 = 1,
    Srgb = 2,
    Pq = 3,
    Hlg = 4,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// Signaled color primaries.
pub enum ColorPrimaries {
    Unspecified = 0,
    Bt601 = 1,
    Bt709 = 2,
    Bt2020 = 3,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// Chroma sample siting metadata.
pub enum ChromaLocation {
    Unspecified = 0,
    Left = 1,
    Center = 2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Packed video color/range/chroma metadata stored in a stream descriptor.
pub struct VideoMeta {
    /// YUV conversion matrix.
    pub matrix: ColorMatrix,
    /// Transfer characteristic.
    pub transfer: ColorTransfer,
    /// Color primaries.
    pub primaries: ColorPrimaries,
    /// Whether sample values use full rather than studio/limited range.
    pub full_range: bool,
    /// Chroma sample location.
    pub chroma: ChromaLocation,
}
impl Default for VideoMeta {
    fn default() -> Self {
        Self {
            matrix: ColorMatrix::Unspecified,
            transfer: ColorTransfer::Unspecified,
            primaries: ColorPrimaries::Unspecified,
            full_range: false,
            chroma: ChromaLocation::Unspecified,
        }
    }
}
impl VideoMeta {
    /// Packs this metadata into the Draft Generation 1 stream-descriptor field.
    pub fn pack(self) -> u32 {
        (self.matrix as u32)
            | ((self.transfer as u32) << 4)
            | ((self.primaries as u32) << 8)
            | ((self.full_range as u32) << 12)
            | ((self.chroma as u32) << 13)
    }
    /// Unpacks and validates a Draft Generation 1 metadata field.
    pub fn unpack(v: u32) -> Result<Self, ContainerError> {
        let matrix = match v & 15 {
            0 => ColorMatrix::Unspecified,
            1 => ColorMatrix::Bt601,
            2 => ColorMatrix::Bt709,
            3 => ColorMatrix::Bt2020Ncl,
            _ => return Err(ContainerError::BadFront),
        };
        let transfer = match (v >> 4) & 15 {
            0 => ColorTransfer::Unspecified,
            1 => ColorTransfer::Bt709,
            2 => ColorTransfer::Srgb,
            3 => ColorTransfer::Pq,
            4 => ColorTransfer::Hlg,
            _ => return Err(ContainerError::BadFront),
        };
        let primaries = match (v >> 8) & 15 {
            0 => ColorPrimaries::Unspecified,
            1 => ColorPrimaries::Bt601,
            2 => ColorPrimaries::Bt709,
            3 => ColorPrimaries::Bt2020,
            _ => return Err(ContainerError::BadFront),
        };
        let chroma = match (v >> 13) & 7 {
            0 => ChromaLocation::Unspecified,
            1 => ChromaLocation::Left,
            2 => ChromaLocation::Center,
            _ => return Err(ContainerError::BadFront),
        };
        if v >> 16 != 0 {
            return Err(ContainerError::BadFront);
        }
        Ok(Self {
            matrix,
            transfer,
            primaries,
            full_range: (v & (1 << 12)) != 0,
            chroma,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Front-index description of one logical media stream.
pub struct StreamDesc {
    /// Stable stream identifier within the file.
    pub id: u16,
    /// Media category.
    pub kind: StreamKind,
    /// Codec-generation identifier defined by the container draft.
    pub codec: u8,
    /// Timestamp units per second.
    pub timescale: u32,
    /// Codec-specific parameter 0.
    pub param0: u32,
    /// Codec-specific parameter 1.
    pub param1: u32,
    /// Stream flags; unknown required bits are rejected by the draft parser.
    pub flags: u32,
    /// Codec/media metadata word.
    pub meta0: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Byte-range and timeline entry for one independently fetchable epoch.
pub struct EpochIndex {
    /// Epoch identifier.
    pub id: u32,
    /// Presentation timestamp.
    pub pts: u64,
    /// Absolute file byte offset.
    pub offset: u64,
    /// Byte length of the complete epoch.
    pub len: u64,
    /// Epoch timeline duration.
    pub duration: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed front index containing stream descriptors and epoch locations.
pub struct Front {
    /// Stream descriptors.
    pub streams: Vec<StreamDesc>,
    /// Indexed independently fetchable epochs.
    pub epochs: Vec<EpochIndex>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// One decoded container packet.
pub struct Packet {
    /// Packet category.
    pub kind: PacketKind,
    /// Packet flags.
    pub flags: u8,
    /// Target logical stream.
    pub stream_id: u16,
    /// Presentation timestamp.
    pub pts: u64,
    /// Packet duration.
    pub duration: u32,
    /// Codec/metadata payload bytes.
    pub payload: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Fixed file header fields.
pub struct FileHeader {
    /// File-level flags.
    pub flags: u32,
    /// Number of stream descriptors in the front index.
    pub stream_count: u16,
    /// Serialized front-index length in bytes.
    pub front_len: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Errors returned while parsing or validating the container.
pub enum ContainerError {
    UnexpectedEof,
    BadMagic,
    UnsupportedVersion(u16),
    BadHeader,
    HeaderChecksum,
    BadFront,
    FrontChecksum,
    UnknownPacketKind(u8),
    PacketTooLarge(u32),
    PacketHeaderChecksum,
    PayloadChecksum,
    TrailingData,
    /// The caller violated the incremental parser reserve/commit protocol.
    ParserState,
    /// A configured memory/resource ceiling would be exceeded.
    ResourceLimit,
    /// Epoch framing is malformed or does not match the requested indexed epoch.
    BadEpoch,
    /// A media packet references no declared stream or a stream of the wrong media kind.
    BadStream,
}

/// Computes the CRC-32C checksum used by the container with runtime-selected kernels.
pub fn crc32c(data: &[u8]) -> u32 {
    avelune_prod_kernels::KernelSet::auto().crc32c(data)
}

/// Computes CRC-32C with an explicitly selected kernel set.
pub fn crc32c_with(kernels: avelune_prod_kernels::KernelSet, data: &[u8]) -> u32 {
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

fn packet_stream<'a>(
    front: &'a Front,
    packet: PacketView<'_>,
) -> Result<Option<&'a StreamDesc>, ContainerError> {
    let expected_kind = match packet.kind {
        PacketKind::VideoFrame => Some(StreamKind::Video),
        PacketKind::AudioFrame => Some(StreamKind::Audio),
        PacketKind::EpochStart | PacketKind::Metadata => None,
    };
    if let Some(expected) = expected_kind {
        let stream = front
            .streams
            .iter()
            .find(|stream| stream.id == packet.stream_id)
            .ok_or(ContainerError::BadStream)?;
        if stream.kind != expected {
            return Err(ContainerError::BadStream);
        }
        Ok(Some(stream))
    } else {
        Ok(None)
    }
}

fn validate_packet_stream(front: &Front, packet: PacketView<'_>) -> Result<(), ContainerError> {
    packet_stream(front, packet).map(|_| ())
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
/// already validated; production muxing should prefer [`encode_packet_checked`].
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
    let (view, n) =
        decode_packet_view_with(b, max_packet, avelune_prod_kernels::KernelSet::auto())?;
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
            let (packet, used) = decode_packet(&bytes[pos..], DEFAULT_MAX_PACKET)?;
            if used == 0 {
                return Err(ContainerError::BadEpoch);
            }
            if first {
                if packet.kind != PacketKind::EpochStart
                    || packet.payload.len() != 4
                    || u32::from_le_bytes(packet.payload.as_slice().try_into().unwrap()) != *id
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
    let mut ids = std::collections::BTreeSet::new();
    for stream in streams {
        if !ids.insert(stream.id) {
            return Err(ContainerError::BadFront);
        }
    }
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
                avelune_prod_kernels::KernelSet::auto(),
            )?;
            validate_packet_stream(&front, packet)?;
            pos = pos.checked_add(used).ok_or(ContainerError::BadEpoch)?;
        }
    }
    Ok(())
}

/// Checked builder used by production integrations.
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

/// Borrowed view of one validated packet in a contiguous input range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketView<'a> {
    /// Packet category.
    pub kind: PacketKind,
    /// Packet flags (zero in V1).
    pub flags: u8,
    /// Target stream identifier.
    pub stream_id: u16,
    /// Presentation timestamp.
    pub pts: u64,
    /// Packet duration.
    pub duration: u32,
    /// Borrowed codec/metadata payload.
    pub payload: &'a [u8],
}
impl PacketView<'_> {
    /// Copies this view into an owned packet, used only when ownership crosses a stream-buffer mutation.
    pub fn to_owned(self) -> Packet {
        Packet {
            kind: self.kind,
            flags: self.flags,
            stream_id: self.stream_id,
            pts: self.pts,
            duration: self.duration,
            payload: self.payload.to_vec(),
        }
    }
}

/// Zero-copy parser for one packet already resident in a contiguous byte range.
pub fn decode_packet_view_with(
    b: &[u8],
    max_packet: usize,
    kernels: avelune_prod_kernels::KernelSet,
) -> Result<(PacketView<'_>, usize), ContainerError> {
    if b.len() < PACKET_HEADER_LEN {
        return Err(ContainerError::UnexpectedEof);
    }
    if b[..4] != PACKET_MAGIC {
        return Err(ContainerError::BadMagic);
    }
    if kernels.crc32c(&b[..24]) != u32::from_le_bytes(b[24..28].try_into().unwrap()) {
        return Err(ContainerError::PacketHeaderChecksum);
    }
    if b[5] != 0 {
        return Err(ContainerError::BadHeader);
    }
    let len = u32::from_le_bytes(b[20..24].try_into().unwrap());
    if len as usize > max_packet {
        return Err(ContainerError::PacketTooLarge(len));
    }
    let total = PACKET_HEADER_LEN
        .checked_add(len as usize)
        .and_then(|x| x.checked_add(PACKET_TRAILER_LEN))
        .ok_or(ContainerError::PacketTooLarge(len))?;
    if b.len() < total {
        return Err(ContainerError::UnexpectedEof);
    }
    let payload = &b[PACKET_HEADER_LEN..PACKET_HEADER_LEN + len as usize];
    if kernels.crc32c(payload)
        != u32::from_le_bytes(
            b[PACKET_HEADER_LEN + len as usize..total]
                .try_into()
                .unwrap(),
        )
    {
        return Err(ContainerError::PayloadChecksum);
    }
    Ok((
        PacketView {
            kind: PacketKind::try_from(b[4])?,
            flags: b[5],
            stream_id: u16::from_le_bytes([b[6], b[7]]),
            pts: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            duration: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            payload,
        },
        total,
    ))
}

/// Validates the semantic epoch boundary carried by framed packets.
///
/// The low-level parsers intentionally only frame/checksum packets. Stateful container/codec
/// integrations should feed every packet through this tracker so codec reference state cannot be
/// used before an `EpochStart`, and an indexed range can be tied to its advertised epoch ID.
#[derive(Debug, Clone, Default)]
pub struct EpochTracker {
    active: bool,
    expected: Option<u32>,
    current: Option<u32>,
}
impl EpochTracker {
    /// Starts validation of a complete file/packet sequence.
    pub fn reset_file(&mut self) {
        self.active = false;
        self.expected = None;
        self.current = None;
    }
    /// Starts validation of one independently fetched epoch range.
    pub fn reset_range(&mut self, expected_epoch: Option<u32>) {
        self.active = false;
        self.expected = expected_epoch;
        self.current = None;
    }
    /// Validates one packet. Returns the epoch ID when the packet starts a new epoch.
    pub fn observe(&mut self, packet: PacketView<'_>) -> Result<Option<u32>, ContainerError> {
        match packet.kind {
            PacketKind::EpochStart => {
                let id = u32::from_le_bytes(
                    packet
                        .payload
                        .try_into()
                        .map_err(|_| ContainerError::BadEpoch)?,
                );
                if self.expected.is_some_and(|expected| expected != id) {
                    return Err(ContainerError::BadEpoch);
                }
                self.active = true;
                self.current = Some(id);
                Ok(Some(id))
            }
            _ if !self.active => Err(ContainerError::BadEpoch),
            _ => Ok(None),
        }
    }
    /// Currently active epoch ID after a validated `EpochStart`.
    pub const fn current(&self) -> Option<u32> {
        self.current
    }
}

/// Borrowed slice parser for complete file prefixes and epoch ranges.
#[derive(Debug, Clone, Copy)]
pub struct SliceParser {
    kernels: avelune_prod_kernels::KernelSet,
    max_packet: usize,
}
impl Default for SliceParser {
    fn default() -> Self {
        Self {
            kernels: avelune_prod_kernels::KernelSet::auto(),
            max_packet: DEFAULT_MAX_PACKET,
        }
    }
}
impl SliceParser {
    /// Creates a slice parser with an explicit dispatch table and packet ceiling.
    pub const fn new(kernels: avelune_prod_kernels::KernelSet, max_packet: usize) -> Self {
        Self {
            kernels,
            max_packet,
        }
    }
    /// Parses one packet without copying its payload.
    pub fn packet<'a>(&self, bytes: &'a [u8]) -> Result<(PacketView<'a>, usize), ContainerError> {
        decode_packet_view_with(bytes, self.max_packet, self.kernels)
    }
}

/// Failure from [`StreamParser::push_each`] or [`StreamParser::commit_reserved`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamPushError<E> {
    /// Container framing or resource-limit failure.
    Container(ContainerError),
    /// Error returned by the packet consumer.
    Consumer(E),
}
impl<E> From<ContainerError> for StreamPushError<E> {
    fn from(value: ContainerError) -> Self {
        Self::Container(value)
    }
}

/// Incremental parser for arbitrary fragment boundaries without per-packet front drains.
#[derive(Debug)]
pub struct StreamParser {
    buf: Vec<u8>,
    cursor: usize,
    prefix_done: bool,
    max_packet: usize,
    max_buffer: usize,
    kernels: avelune_prod_kernels::KernelSet,
    reserved_start: Option<usize>,
    header: Option<FileHeader>,
    front: Option<Front>,
}
impl Default for StreamParser {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_PACKET,
            160 * 1024 * 1024,
            avelune_prod_kernels::KernelSet::auto(),
        )
    }
}
impl StreamParser {
    /// Creates a parser from common production limits and an explicit kernel set.
    pub fn with_limits(
        limits: crate::limits::Limits,
        kernels: avelune_prod_kernels::KernelSet,
    ) -> Self {
        Self::new(
            limits.max_packet_bytes,
            limits.max_stream_buffer_bytes,
            kernels,
        )
    }
    /// Creates a bounded incremental parser.
    pub fn new(
        max_packet: usize,
        max_buffer: usize,
        kernels: avelune_prod_kernels::KernelSet,
    ) -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
            prefix_done: false,
            max_packet,
            max_buffer: max_buffer.max(FIXED_HEADER_LEN),
            kernels,
            reserved_start: None,
            header: None,
            front: None,
        }
    }
    fn unread_len(&self) -> usize {
        self.buf.len().saturating_sub(self.cursor)
    }
    fn compact(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let unread = self.unread_len();
        self.buf.copy_within(self.cursor.., 0);
        self.buf.truncate(unread);
        self.cursor = 0;
    }
    fn make_room(&mut self, incoming: usize) -> Result<(), ContainerError> {
        if self.cursor > 64 * 1024
            && (self.cursor >= self.buf.len() / 2
                || self.buf.len().saturating_add(incoming) > self.max_buffer)
        {
            self.compact();
        }
        if self
            .unread_len()
            .checked_add(incoming)
            .ok_or(ContainerError::ResourceLimit)?
            > self.max_buffer
        {
            return Err(ContainerError::ResourceLimit);
        }
        Ok(())
    }
    /// Reserves writable tail storage for a transport fragment without an intermediate copy.
    /// The caller must follow with [`Self::commit_reserved`] before reserving again.
    pub fn reserve_fragment(&mut self, len: usize) -> Result<&mut [u8], ContainerError> {
        if self.reserved_start.is_some() {
            return Err(ContainerError::ParserState);
        }
        self.make_room(len)?;
        let start = self.buf.len();
        let end = start
            .checked_add(len)
            .ok_or(ContainerError::ResourceLimit)?;
        self.buf.resize(end, 0);
        self.reserved_start = Some(start);
        Ok(&mut self.buf[start..end])
    }

    fn parse_available_each_context<E>(
        &mut self,
        mut consume: impl FnMut(PacketView<'_>, Option<&StreamDesc>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        if !self.prefix_done {
            if self.unread_len() < FIXED_HEADER_LEN {
                return Ok(());
            }
            let start = self.cursor;
            let h = decode_header(&self.buf[start..start + FIXED_HEADER_LEN])?;
            self.header = Some(h.clone());
            let n = FIXED_HEADER_LEN
                .checked_add(h.front_len as usize)
                .ok_or(ContainerError::BadHeader)?;
            if n > self.max_buffer {
                return Err(ContainerError::ResourceLimit.into());
            }
            if self.unread_len() < n {
                return Ok(());
            }
            let front = decode_front(
                &self.buf[start + FIXED_HEADER_LEN..start + n],
                h.stream_count,
            )?;
            self.front = Some(front);
            self.cursor += n;
            self.prefix_done = true;
        }
        loop {
            if self.unread_len() == 0 {
                break;
            }
            let start = self.cursor;
            match decode_packet_view_with(&self.buf[start..], self.max_packet, self.kernels) {
                Ok((view, n)) => {
                    let stream = if let Some(front) = &self.front {
                        packet_stream(front, view)?
                    } else {
                        None
                    };
                    consume(view, stream).map_err(StreamPushError::Consumer)?;
                    self.cursor += n;
                }
                Err(ContainerError::UnexpectedEof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        if self.cursor > 1024 * 1024 && self.cursor >= self.buf.len() / 2 {
            self.compact();
        }
        Ok(())
    }

    /// Commits at most `written` bytes from the most recent reservation and streams packet views
    /// synchronously to `consume`. Packet payloads are borrowed directly from parser storage and
    /// are valid only for the duration of the callback.
    pub fn commit_reserved<E>(
        &mut self,
        written: usize,
        mut consume: impl FnMut(PacketView<'_>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        let start = self
            .reserved_start
            .take()
            .ok_or(ContainerError::ParserState)?;
        let reserved = self.buf.len().saturating_sub(start);
        if written > reserved {
            self.buf.truncate(start);
            return Err(ContainerError::ParserState.into());
        }
        self.buf.truncate(start + written);
        self.parse_available_each_context(|packet, _stream| consume(packet))
    }

    fn commit_reserved_context<E>(
        &mut self,
        written: usize,
        consume: impl FnMut(PacketView<'_>, Option<&StreamDesc>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        let start = self
            .reserved_start
            .take()
            .ok_or(ContainerError::ParserState)?;
        let reserved = self.buf.len().saturating_sub(start);
        if written > reserved {
            self.buf.truncate(start);
            return Err(ContainerError::ParserState.into());
        }
        self.buf.truncate(start + written);
        self.parse_available_each_context(consume)
    }

    /// Copies an arbitrary fragment into parser storage and synchronously exposes validated packet
    /// views. Integrations that can write into caller-provided storage should prefer
    /// [`Self::reserve_fragment`] plus [`Self::commit_reserved`] to avoid this copy.
    pub fn push_each<E>(
        &mut self,
        fragment: &[u8],
        consume: impl FnMut(PacketView<'_>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        self.reserve_fragment(fragment.len())?
            .copy_from_slice(fragment);
        self.commit_reserved(fragment.len(), consume)
    }

    /// Appends an arbitrary fragment and returns owned packets for convenience.
    pub fn push(&mut self, fragment: &[u8]) -> Result<Vec<Packet>, ContainerError> {
        let mut out = Vec::new();
        match self.push_each(fragment, |view| {
            out.push(view.to_owned());
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(()) => Ok(out),
            Err(StreamPushError::Container(e)) => Err(e),
            Err(StreamPushError::Consumer(never)) => match never {},
        }
    }
    /// Returns the validated fixed header once at least the complete fixed header has arrived.
    pub fn file_header(&self) -> Option<&FileHeader> {
        self.header.as_ref()
    }
    /// Returns the validated front index once the complete file prefix has arrived.
    pub fn front_index(&self) -> Option<&Front> {
        self.front.as_ref()
    }

    /// Resets parser state for a new complete file while preserving limits and kernel dispatch.
    pub fn reset_file(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.prefix_done = false;
        self.reserved_start = None;
        self.header = None;
        self.front = None;
    }

    /// Resets the parser to accept a standalone epoch range rather than a full file prefix.
    pub fn reset_epoch_range(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.prefix_done = true;
        self.reserved_start = None;
    }
    /// Finishes the stream, rejecting any incomplete trailing bytes.
    pub fn finish(self) -> Result<(), ContainerError> {
        if self.reserved_start.is_some() {
            return Err(ContainerError::ParserState);
        }
        if self.unread_len() == 0 {
            Ok(())
        } else {
            Err(ContainerError::TrailingData)
        }
    }
    /// Number of unread bytes currently buffered.
    pub fn buffered_bytes(&self) -> usize {
        self.unread_len()
    }
}

/// Backwards-compatible name for the production stream parser.
pub type IncrementalParser = StreamParser;

impl std::fmt::Display for ContainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ContainerError {}

/// One decoded output emitted by [`ContainerStreamDecoder`].
#[derive(Debug)]
pub enum DecodedOutput {
    /// Validated epoch boundary. Video reference state has already been reset.
    EpochStart {
        /// Epoch identifier carried by the EpochStart payload.
        id: u32,
        /// Container presentation timestamp.
        pts: u64,
        /// Indexed epoch duration when supplied by the packet.
        duration: u32,
    },
    /// Reconstructed ALV1 video frame.
    Video {
        /// Declared container stream identifier.
        stream_id: u16,
        /// Container presentation timestamp.
        pts: u64,
        /// Packet duration.
        duration: u32,
        /// Immutable ALV1 frame identifier.
        frame_id: u64,
        /// Shared reconstruction; the decoder may retain the same allocation as a reference.
        frame: std::sync::Arc<crate::video::v1::Frame420>,
        /// Immutable reference frame IDs required by this packet.
        dependencies: Vec<u64>,
    },
    /// Decoded ALA1 PCM packet.
    Audio {
        /// Declared container stream identifier.
        stream_id: u16,
        /// Container presentation timestamp.
        pts: u64,
        /// Packet duration.
        duration: u32,
        /// Decoded PCM rate.
        sample_rate: u32,
        /// Decoded channel count.
        channels: u8,
        /// Interleaved signed-16 PCM.
        pcm: Vec<i16>,
    },
    /// Metadata packet. Metadata semantics are not otherwise interpreted by Draft Generation 1.
    Metadata {
        /// Declared stream identifier (zero may denote container-global metadata).
        stream_id: u16,
        /// Container presentation timestamp.
        pts: u64,
        /// Packet duration.
        duration: u32,
        /// Owned metadata payload because parser storage is reused after the callback.
        payload: Vec<u8>,
    },
}

/// Error from the combined streaming container/codec decoder.
#[derive(Debug)]
pub enum StreamDecodeError<E> {
    /// Container framing, checksum, routing, epoch, or resource failure.
    Container(ContainerError),
    /// ALV1 syntax/reconstruction failure.
    Video(crate::video::v1::VideoError),
    /// ALA1 syntax/reconstruction failure.
    Audio(crate::audio::v1::AudioError),
    /// Error returned by the caller's decoded-output consumer.
    Consumer(E),
}

impl<E: std::fmt::Display> std::fmt::Display for StreamDecodeError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container(e) => write!(f, "container: {e}"),
            Self::Video(e) => write!(f, "video: {e}"),
            Self::Audio(e) => write!(f, "audio: {e}"),
            Self::Consumer(e) => write!(f, "consumer: {e}"),
        }
    }
}

#[derive(Debug)]
enum DecodeConsumerError<E> {
    ContainerEpoch(ContainerError),
    Video(crate::video::v1::VideoError),
    Audio(crate::audio::v1::AudioError),
    Consumer(E),
}

/// Stateful end-to-end Draft Generation 1 streaming decoder.
///
/// This owns the incremental container parser, epoch validator, ALV1 reference history and ALA1
/// entropy scratch. It is the preferred embedding surface when input arrives incrementally.
/// Transport code may either copy fragments with [`Self::push_each`] or write directly into the
/// parser-owned tail returned by [`Self::reserve_fragment`] and then call [`Self::commit_reserved`].
#[derive(Debug)]
pub struct ContainerStreamDecoder {
    parser: StreamParser,
    epoch: EpochTracker,
    video: crate::video::v1::VideoDecoder,
    audio: crate::audio::v1::AudioDecoder,
}

impl Default for ContainerStreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainerStreamDecoder {
    /// Creates a decoder using automatic CPU/thread selection and default resource ceilings.
    pub fn new() -> Self {
        Self {
            parser: StreamParser::default(),
            epoch: EpochTracker::default(),
            video: crate::video::v1::VideoDecoder::new(),
            audio: crate::audio::v1::AudioDecoder::new(),
        }
    }

    /// Creates a decoder from one common production configuration.
    pub fn with_config(
        config: crate::config::Config,
    ) -> Result<Self, crate::config::BackendUnavailable> {
        let kernels = crate::config::kernel_set(config.cpu)?;
        Ok(Self {
            parser: StreamParser::with_limits(config.limits, kernels),
            epoch: EpochTracker::default(),
            video: crate::video::v1::VideoDecoder::with_config(config)?,
            audio: crate::audio::v1::AudioDecoder::with_limits(config.limits),
        })
    }

    /// Reserves parser-owned writable storage for a transport fragment.
    pub fn reserve_fragment(&mut self, len: usize) -> Result<&mut [u8], ContainerError> {
        self.parser.reserve_fragment(len)
    }

    /// Commits bytes from the latest reservation and synchronously emits decoded outputs.
    pub fn commit_reserved<E>(
        &mut self,
        written: usize,
        mut consume: impl FnMut(DecodedOutput) -> Result<(), E>,
    ) -> Result<(), StreamDecodeError<E>> {
        let Self {
            parser,
            epoch,
            video,
            audio,
        } = self;
        let result = parser.commit_reserved_context(written, |packet, stream| {
            // Epoch semantics sit above packet framing; carry tracker failures through the local
            // consumer channel and normalize them back to a container error below.
            let started = epoch
                .observe(packet)
                .map_err(DecodeConsumerError::<E>::ContainerEpoch)?;
            match packet.kind {
                PacketKind::EpochStart => {
                    let id = started.ok_or(DecodeConsumerError::ContainerEpoch(
                        ContainerError::BadEpoch,
                    ))?;
                    video.reset_epoch();
                    consume(DecodedOutput::EpochStart {
                        id,
                        pts: packet.pts,
                        duration: packet.duration,
                    })
                    .map_err(DecodeConsumerError::Consumer)
                }
                PacketKind::VideoFrame => {
                    let stream = stream.ok_or(DecodeConsumerError::ContainerEpoch(
                        ContainerError::BadStream,
                    ))?;
                    if stream.codec != 1 {
                        return Err(DecodeConsumerError::ContainerEpoch(
                            ContainerError::BadStream,
                        ));
                    }
                    let (frame_id, frame, dependencies) = video
                        .decode_shared(packet.payload)
                        .map_err(DecodeConsumerError::Video)?;
                    if frame.width != stream.param0 || frame.height != stream.param1 {
                        return Err(DecodeConsumerError::ContainerEpoch(
                            ContainerError::BadStream,
                        ));
                    }
                    consume(DecodedOutput::Video {
                        stream_id: packet.stream_id,
                        pts: packet.pts,
                        duration: packet.duration,
                        frame_id,
                        frame,
                        dependencies,
                    })
                    .map_err(DecodeConsumerError::Consumer)
                }
                PacketKind::AudioFrame => {
                    let stream = stream.ok_or(DecodeConsumerError::ContainerEpoch(
                        ContainerError::BadStream,
                    ))?;
                    if stream.codec != 1 {
                        return Err(DecodeConsumerError::ContainerEpoch(
                            ContainerError::BadStream,
                        ));
                    }
                    let (sample_rate, channels, pcm) = audio
                        .decode(packet.payload)
                        .map_err(DecodeConsumerError::Audio)?;
                    if sample_rate != stream.param0 || u32::from(channels) != stream.param1 {
                        return Err(DecodeConsumerError::ContainerEpoch(
                            ContainerError::BadStream,
                        ));
                    }
                    consume(DecodedOutput::Audio {
                        stream_id: packet.stream_id,
                        pts: packet.pts,
                        duration: packet.duration,
                        sample_rate,
                        channels,
                        pcm,
                    })
                    .map_err(DecodeConsumerError::Consumer)
                }
                PacketKind::Metadata => consume(DecodedOutput::Metadata {
                    stream_id: packet.stream_id,
                    pts: packet.pts,
                    duration: packet.duration,
                    payload: packet.payload.to_vec(),
                })
                .map_err(DecodeConsumerError::Consumer),
            }
        });
        match result {
            Ok(()) => Ok(()),
            Err(StreamPushError::Container(e)) => Err(StreamDecodeError::Container(e)),
            Err(StreamPushError::Consumer(DecodeConsumerError::Video(e))) => {
                Err(StreamDecodeError::Video(e))
            }
            Err(StreamPushError::Consumer(DecodeConsumerError::Audio(e))) => {
                Err(StreamDecodeError::Audio(e))
            }
            Err(StreamPushError::Consumer(DecodeConsumerError::Consumer(e))) => {
                Err(StreamDecodeError::Consumer(e))
            }
            Err(StreamPushError::Consumer(DecodeConsumerError::ContainerEpoch(e))) => {
                Err(StreamDecodeError::Container(e))
            }
        }
    }

    /// Copies one arbitrary transport fragment and emits every complete decoded output available.
    pub fn push_each<E>(
        &mut self,
        fragment: &[u8],
        consume: impl FnMut(DecodedOutput) -> Result<(), E>,
    ) -> Result<(), StreamDecodeError<E>> {
        self.reserve_fragment(fragment.len())
            .map_err(StreamDecodeError::Container)?
            .copy_from_slice(fragment);
        self.commit_reserved(fragment.len(), consume)
    }

    /// Resets to a new complete file stream, clearing codec state and previously parsed index data.
    pub fn reset_file(&mut self) {
        self.parser.reset_file();
        self.epoch.reset_file();
        self.video.reset_epoch();
    }

    /// Resets to one independently fetched indexed epoch range.
    pub fn reset_epoch_range(&mut self, expected_epoch: Option<u32>) {
        self.parser.reset_epoch_range();
        self.epoch.reset_range(expected_epoch);
        self.video.reset_epoch();
    }

    /// Validated fixed header when the complete header has arrived.
    pub fn file_header(&self) -> Option<&FileHeader> {
        self.parser.file_header()
    }

    /// Validated front index when the complete prefix has arrived.
    pub fn front_index(&self) -> Option<&Front> {
        self.parser.front_index()
    }

    /// Current validated epoch identifier.
    pub const fn current_epoch(&self) -> Option<u32> {
        self.epoch.current()
    }

    /// Rejects incomplete trailing container bytes.
    pub fn finish(self) -> Result<(), ContainerError> {
        self.parser.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ep() -> Vec<u8> {
        let mut v = Vec::new();
        encode_packet(
            &Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts: 0,
                duration: 1000,
                payload: 1u32.to_le_bytes().to_vec(),
            },
            &mut v,
        );
        v
    }
    #[test]
    fn checked_builder_rejects_epoch_without_matching_start() {
        let s = vec![StreamDesc {
            id: 1,
            kind: StreamKind::Video,
            codec: 1,
            timescale: TIMEBASE,
            param0: 2,
            param1: 2,
            flags: 0,
            meta0: 0,
        }];
        let bytes = ep();
        assert_eq!(
            build_file_checked(s, vec![(7, 0, 1000, bytes)]),
            Err(ContainerError::BadEpoch)
        );
    }

    #[test]
    fn checked_builder_validates_entire_epoch_and_stream_routes() {
        let streams = vec![StreamDesc {
            id: 2,
            kind: StreamKind::Audio,
            codec: 1,
            timescale: TIMEBASE,
            param0: 48_000,
            param1: 2,
            flags: 0,
            meta0: 0,
        }];
        let mut bytes = Vec::new();
        encode_packet(
            &Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts: 0,
                duration: 1,
                payload: 0u32.to_le_bytes().to_vec(),
            },
            &mut bytes,
        );
        encode_packet(
            &Packet {
                kind: PacketKind::VideoFrame,
                flags: 0,
                stream_id: 2,
                pts: 0,
                duration: 1,
                payload: vec![1, 2, 3],
            },
            &mut bytes,
        );
        assert_eq!(
            build_file_checked(streams, vec![(0, 0, 1, bytes)]),
            Err(ContainerError::BadStream)
        );

        let mut nested = Vec::new();
        encode_packet(
            &Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts: 0,
                duration: 1,
                payload: 0u32.to_le_bytes().to_vec(),
            },
            &mut nested,
        );
        encode_packet(
            &Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts: 1,
                duration: 1,
                payload: 1u32.to_le_bytes().to_vec(),
            },
            &mut nested,
        );
        assert_eq!(
            validate_epoch_ranges(&[(0, 0, 1, nested)]),
            Err(ContainerError::BadEpoch)
        );
    }

    #[test]
    fn file_index_roundtrip() {
        let s = vec![StreamDesc {
            id: 1,
            kind: StreamKind::Video,
            codec: 1,
            timescale: TIMEBASE,
            param0: 320,
            param1: 180,
            flags: 0,
            meta0: 0,
        }];
        let f = build_file(s.clone(), vec![(0, 0, 1000, ep()), (1, 1000, 1000, ep())]);
        let (h, front, n) = parse_file_prefix(&f).unwrap();
        assert_eq!(h.stream_count, 1);
        assert_eq!(front.streams, s);
        assert_eq!(front.epochs.len(), 2);
        assert_eq!(front.epochs[0].offset, n as u64);
        assert_eq!(
            front.epochs[1].offset,
            front.epochs[0].offset + front.epochs[0].len
        );
    }
    #[test]
    fn video_meta_roundtrip() {
        let m = VideoMeta {
            matrix: ColorMatrix::Bt709,
            transfer: ColorTransfer::Srgb,
            primaries: ColorPrimaries::Bt709,
            full_range: true,
            chroma: ChromaLocation::Center,
        };
        assert_eq!(VideoMeta::unpack(m.pack()).unwrap(), m);
        assert!(VideoMeta::unpack(0x1_0000).is_err());
    }
    #[test]
    fn one_byte_incremental() {
        let s = vec![StreamDesc {
            id: 1,
            kind: StreamKind::Video,
            codec: 1,
            timescale: TIMEBASE,
            param0: 2,
            param1: 2,
            flags: 0,
            meta0: 0,
        }];
        let f = build_file(s, vec![(0, 0, 1, ep())]);
        let mut p = IncrementalParser::default();
        let mut n = 0;
        for b in f {
            n += p.push(&[b]).unwrap().len()
        }
        p.finish().unwrap();
        assert_eq!(n, 1);
    }
}
