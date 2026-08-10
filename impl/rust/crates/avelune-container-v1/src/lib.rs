//! Reference implementation of the experimental Avelune streaming container.
//!
//! The container is front-indexed, packetized, CRC-protected, and designed so epochs can
//! be fetched independently with HTTP byte ranges. The normative draft lives under `spec/`.

#![forbid(unsafe_code)]

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
}

/// Computes the CRC-32C checksum used by the container.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82f63b78 & mask)
        }
    }
    !crc
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
    let need = 12usize
        .checked_add(sc * 24)
        .and_then(|x| x.checked_add(ec * 32))
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
    for _ in 0..sc {
        let id = u16::from_le_bytes([b[p], b[p + 1]]);
        let kind = StreamKind::try_from(b[p + 2])?;
        let codec = b[p + 3];
        let timescale = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap());
        let param0 = u32::from_le_bytes(b[p + 8..p + 12].try_into().unwrap());
        let param1 = u32::from_le_bytes(b[p + 12..p + 16].try_into().unwrap());
        let flags = u32::from_le_bytes(b[p + 16..p + 20].try_into().unwrap());
        let meta0 = u32::from_le_bytes(b[p + 20..p + 24].try_into().unwrap());
        p += 24;
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
    for _ in 0..ec {
        let id = u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
        let duration = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap());
        let pts = u64::from_le_bytes(b[p + 8..p + 16].try_into().unwrap());
        let offset = u64::from_le_bytes(b[p + 16..p + 24].try_into().unwrap());
        let len = u64::from_le_bytes(b[p + 24..p + 32].try_into().unwrap());
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

/// Appends one framed, checksummed packet.
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
/// Parses one packet and returns it together with the consumed byte count.
pub fn decode_packet(b: &[u8], max_packet: usize) -> Result<(Packet, usize), ContainerError> {
    if b.len() < PACKET_HEADER_LEN {
        return Err(ContainerError::UnexpectedEof);
    }
    if b[..4] != PACKET_MAGIC {
        return Err(ContainerError::BadMagic);
    }
    if crc32c(&b[..24]) != u32::from_le_bytes(b[24..28].try_into().unwrap()) {
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
        .and_then(|x| x.checked_add(4))
        .ok_or(ContainerError::PacketTooLarge(len))?;
    if b.len() < total {
        return Err(ContainerError::UnexpectedEof);
    }
    let payload = b[28..28 + len as usize].to_vec();
    if crc32c(&payload) != u32::from_le_bytes(b[28 + len as usize..total].try_into().unwrap()) {
        return Err(ContainerError::PayloadChecksum);
    }
    Ok((
        Packet {
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

/// Incremental packet parser for arbitrarily fragmented byte streams.
pub struct IncrementalParser {
    buf: Vec<u8>,
    prefix_done: bool,
    max_packet: usize,
}
impl Default for IncrementalParser {
    fn default() -> Self {
        Self {
            buf: Vec::new(),
            prefix_done: false,
            max_packet: DEFAULT_MAX_PACKET,
        }
    }
}
impl IncrementalParser {
    /// Appends arbitrary byte fragments and returns every complete packet decoded from them.
    pub fn push(&mut self, c: &[u8]) -> Result<Vec<Packet>, ContainerError> {
        self.buf.extend(c);
        let mut out = Vec::new();
        if !self.prefix_done {
            if self.buf.len() < FIXED_HEADER_LEN {
                return Ok(out);
            }
            let h = decode_header(&self.buf[..32])?;
            let n = 32 + h.front_len as usize;
            if self.buf.len() < n {
                return Ok(out);
            }
            decode_front(&self.buf[32..n], h.stream_count)?;
            self.buf.drain(..n);
            self.prefix_done = true;
        }
        loop {
            if self.buf.is_empty() {
                break;
            }
            match decode_packet(&self.buf, self.max_packet) {
                Ok((p, n)) => {
                    self.buf.drain(..n);
                    out.push(p)
                }
                Err(ContainerError::UnexpectedEof) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }
    pub fn finish(self) -> Result<(), ContainerError> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(ContainerError::TrailingData)
        }
    }
}

impl std::fmt::Display for ContainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ContainerError {}

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
