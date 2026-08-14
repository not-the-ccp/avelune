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

mod parser;
mod session;
mod wire;

pub use parser::*;
pub use session::*;
pub use wire::*;

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
        let mut p = StreamParser::default();
        let mut n = 0;
        for b in f {
            n += p.push(&[b]).unwrap().len()
        }
        p.finish().unwrap();
        assert_eq!(n, 1);
    }
    #[test]
    fn stream_decoder_keeps_reference_history_per_video_stream() {
        use crate::video::v1::{EncodeOptions, Frame420, VideoEncoder};
        use std::convert::Infallible;

        fn frame(seed: u8) -> Frame420 {
            let w = 16u32;
            let h = 16u32;
            let y = (0..w * h)
                .map(|i| seed.wrapping_add((i * 7) as u8))
                .collect();
            let u = (0..w * h / 4)
                .map(|i| seed.wrapping_add((i * 3) as u8))
                .collect();
            let v = (0..w * h / 4)
                .map(|i| seed.wrapping_sub((i * 5) as u8))
                .collect();
            Frame420::from_planes(w, h, y, u, v).unwrap()
        }

        let a0 = frame(11);
        let a1 = frame(12);
        let b0 = frame(171);
        let b1 = frame(172);
        let opt = EncodeOptions {
            qstep: 1,
            motion_radius: 2,
            max_refs: 1,
            ..Default::default()
        };
        let mut a = VideoEncoder::new(opt);
        let mut b = VideoEncoder::new(opt);
        let packets = [
            (10u16, 0u64, a.encode(0, &a0).unwrap().packet),
            (20u16, 0u64, b.encode(0, &b0).unwrap().packet),
            (10u16, 1u64, a.encode(1, &a1).unwrap().packet),
            (20u16, 1u64, b.encode(1, &b1).unwrap().packet),
        ];
        let mut epoch = Vec::new();
        encode_packet_checked(
            &Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts: 0,
                duration: 4,
                payload: 7u32.to_le_bytes().to_vec(),
            },
            &mut epoch,
        )
        .unwrap();
        for (stream_id, frame_id, payload) in packets {
            encode_packet_checked(
                &Packet {
                    kind: PacketKind::VideoFrame,
                    flags: 0,
                    stream_id,
                    pts: frame_id,
                    duration: 1,
                    payload,
                },
                &mut epoch,
            )
            .unwrap();
        }
        let streams = [10u16, 20u16]
            .into_iter()
            .map(|id| StreamDesc {
                id,
                kind: StreamKind::Video,
                codec: 1,
                timescale: TIMEBASE,
                param0: 16,
                param1: 16,
                flags: 0,
                meta0: 0,
            })
            .collect();
        let file = build_file_checked(streams, vec![(7, 0, 4, epoch)]).unwrap();
        let mut decoder = ContainerStreamDecoder::new();
        let mut got = Vec::new();
        decoder
            .push_each(&file, |output| {
                if let DecodedOutput::Video {
                    stream_id,
                    frame_id,
                    frame,
                    ..
                } = output
                {
                    got.push((stream_id, frame_id, frame));
                }
                Ok::<_, Infallible>(())
            })
            .unwrap();
        decoder.finish_input().unwrap();
        assert_eq!(got.len(), 4);
        let expected = [(10, 0, &a0), (20, 0, &b0), (10, 1, &a1), (20, 1, &b1)];
        for ((sid, fid, actual), (esid, efid, source)) in got.iter().zip(expected) {
            assert_eq!((*sid, *fid), (esid, efid));
            assert_eq!(actual.as_ref(), source);
        }
    }
}
