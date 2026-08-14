//! Canonical ALV1 Draft Generation 1 codec implementation.
//!
//! Normative parsing and reconstruction stay in safe Rust. Coarse independent plane work is
//! parallelized only above a size threshold; architecture-specific kernels remain isolated in
//! `avelune-kernels`.

// The normative syntax has several naturally wide helper signatures and fixed-size loops.
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::manual_checked_ops,
    clippy::manual_is_multiple_of,
    clippy::manual_div_ceil,
    clippy::manual_range_contains,
    clippy::needless_lifetimes
)]
use crate::bitstream::v1::{
    BitstreamError, EntropyScratch, entropy_compress, entropy_decompress_with_scratch,
    get_svarint_i32, get_uvarint, put_svarint_i32, put_uvarint,
};

/// Four-byte packet magic for the current draft video generation.
pub const CODEC_MAGIC: [u8; 4] = *b"ALV1";
/// Maximum luma-pixel count accepted by the reference implementation.
pub const MAX_PIXELS: u64 = 8192 * 8192;
/// Transform/prediction block edge length in samples.
pub const BLOCK: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Owned 8-bit planar 4:2:0 frame backed by one contiguous allocation.
pub struct Frame420 {
    /// Luma width in pixels. Must be non-zero and even.
    pub width: u32,
    /// Luma height in pixels. Must be non-zero and even.
    pub height: u32,
    storage: crate::buffer::OwnedFrame420,
}
impl Frame420 {
    /// Allocates a zero-filled tightly packed canonical frame.
    pub fn new(width: u32, height: u32) -> Result<Self, VideoError> {
        sizes(width, height)?;
        let layout = crate::buffer::FrameLayout::new(width as usize, height as usize, 1)
            .ok_or(VideoError::BadDimensions)?;
        Ok(Self {
            width,
            height,
            storage: crate::buffer::OwnedFrame420::new(layout),
        })
    }

    /// Adopts one tightly packed Y/U/V allocation.
    pub fn from_tightly_packed(width: u32, height: u32, data: Vec<u8>) -> Result<Self, VideoError> {
        let (y_len, c_len) = sizes(width, height)?;
        let expected = y_len
            .checked_add(c_len.checked_mul(2).ok_or(VideoError::PlaneLength)?)
            .ok_or(VideoError::PlaneLength)?;
        if data.len() != expected {
            return Err(VideoError::PlaneLength);
        }
        let storage = crate::buffer::OwnedFrame420::from_tightly_packed_data(
            width as usize,
            height as usize,
            data,
        )
        .ok_or(VideoError::PlaneLength)?;
        Ok(Self {
            width,
            height,
            storage,
        })
    }

    /// Adopts three tightly packed planes into one allocation.
    pub fn from_planes(
        width: u32,
        height: u32,
        mut y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    ) -> Result<Self, VideoError> {
        let (y_len, c_len) = sizes(width, height)?;
        if y.len() != y_len || u.len() != c_len || v.len() != c_len {
            return Err(VideoError::PlaneLength);
        }
        y.reserve(u.len().saturating_add(v.len()));
        y.extend(u);
        y.extend(v);
        let storage = crate::buffer::OwnedFrame420::from_tightly_packed_data(
            width as usize,
            height as usize,
            y,
        )
        .ok_or(VideoError::PlaneLength)?;
        Ok(Self {
            width,
            height,
            storage,
        })
    }

    /// Full-resolution tightly packed luma plane.
    pub fn y(&self) -> &[u8] {
        self.storage.y()
    }
    /// Mutable full-resolution tightly packed luma plane.
    pub fn y_mut(&mut self) -> &mut [u8] {
        self.storage.y_mut()
    }
    /// Half-resolution tightly packed Cb/U plane.
    pub fn u(&self) -> &[u8] {
        self.storage.u()
    }
    /// Mutable half-resolution tightly packed Cb/U plane.
    pub fn u_mut(&mut self) -> &mut [u8] {
        self.storage.u_mut()
    }
    /// Half-resolution tightly packed Cr/V plane.
    pub fn v(&self) -> &[u8] {
        self.storage.v()
    }
    /// Mutable half-resolution tightly packed Cr/V plane.
    pub fn v_mut(&mut self) -> &mut [u8] {
        self.storage.v_mut()
    }

    /// Returns the tightly packed Y/U/V backing allocation.
    pub fn into_tightly_packed(self) -> Vec<u8> {
        self.storage.into_data()
    }
    /// Explicit-stride immutable frame view for bulk kernels/embedders.
    pub fn view(&self) -> crate::buffer::Frame420View<'_> {
        self.storage.view()
    }
    /// Explicit-stride mutable frame view for bulk kernels/embedders.
    pub fn view_mut(&mut self) -> crate::buffer::Frame420ViewMut<'_> {
        self.storage.view_mut()
    }
    /// Number of bytes in the single backing allocation.
    pub fn storage_len(&self) -> usize {
        self.storage.data().len()
    }
    /// Validates dimensions and backing layout.
    pub fn validate(&self) -> Result<(), VideoError> {
        let (y, c) = sizes(self.width, self.height)?;
        if self.y().len() != y || self.u().len() != c || self.v().len() != c {
            return Err(VideoError::PlaneLength);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Non-normative encoder search policy.
pub enum EncoderPreset {
    /// Minimize search work; intended for interactive/throughput-sensitive encoding.
    Fast,
    /// Coarse-to-fine integer motion search plus half-sample refinement.
    #[default]
    Balanced,
    /// Exhaustive configured-radius integer search before half-sample refinement.
    Quality,
}

#[derive(Debug, Clone, Copy)]
/// Encoder search options. These are non-normative policy knobs.
pub struct EncodeOptions {
    /// Quantizer step. `1` selects mathematically lossless residual coding.
    pub qstep: u16,
    /// Integer-pixel motion-search radius before half-sample refinement.
    pub motion_radius: u8,
    /// Maximum reference frames considered by the encoder.
    pub max_refs: u8,
    /// Motion/mode search policy.
    pub preset: EncoderPreset,
    /// Whether exact small-palette blocks may be considered.
    pub allow_palette: bool,
}
impl EncodeOptions {
    /// Builds the standard search configuration for one preset and caller-selected quantizer.
    pub fn for_preset(qstep: u16, preset: EncoderPreset) -> Self {
        let (motion_radius, max_refs) = match preset {
            EncoderPreset::Fast => (2, 1),
            EncoderPreset::Balanced => (4, 1),
            EncoderPreset::Quality => (5, 4),
        };
        Self {
            qstep,
            motion_radius,
            max_refs,
            preset,
            allow_palette: true,
        }
    }
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            qstep: 96,
            motion_radius: 4,
            max_refs: 1,
            preset: EncoderPreset::Balanced,
            allow_palette: true,
        }
    }
}
#[derive(Debug, Clone)]
/// Result of encoding one ALV1 frame.
pub struct EncodedFrame {
    /// Complete ALV1 frame packet payload.
    pub packet: Vec<u8>,
    /// Encoder reconstruction, suitable for future reference prediction.
    pub reconstructed: Frame420,
    /// Exact immutable frame IDs needed to decode this packet.
    pub dependencies: Vec<u64>,
}

#[derive(Debug, Clone)]
/// Stateful-encoder result that shares reconstruction storage with the reference history.
pub struct SharedEncodedFrame {
    /// Complete ALV1 frame packet payload.
    pub packet: Vec<u8>,
    /// Reconstruction retained without a second full-frame copy.
    pub reconstructed: std::sync::Arc<Frame420>,
    /// Exact immutable frame IDs needed to decode this packet.
    pub dependencies: Vec<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Errors from ALV1 frame validation, encoding, or decoding.
pub enum VideoError {
    /// Frame dimensions are zero, odd for 4:2:0, overflow addressable storage, or exceed limits.
    BadDimensions,
    /// One or more Y/U/V planes do not have the exact length implied by the frame dimensions.
    PlaneLength,
    /// An ALV1 packet ended before all required syntax could be read.
    UnexpectedEof,
    /// The ALV1 frame header, reserved fields, IDs, or other header-level invariants are invalid.
    BadHeader,
    /// A block uses an unknown or invalid prediction/encoding mode.
    BadMode,
    /// The encoded quantizer is outside the Draft Generation 1 valid range.
    BadQuantizer,
    /// A referenced frame ID is not present in the decoder's current epoch history.
    ReferenceMissing(u64),
    /// A supplied/reference frame has dimensions incompatible with the frame being processed.
    ReferenceShape,
    /// Entropy/varint syntax failed while parsing or writing the ALV1 bitstream.
    Bitstream(BitstreamError),
    /// Transform coefficient syntax is malformed or outside defensive numeric bounds.
    BadCoefficient,
    /// Palette-block syntax is malformed or contains invalid palette/index data.
    BadPalette,
    /// A complete ALV1 packet contains bytes after the expected syntax.
    TrailingData,
    /// Decoding would exceed a configured frame/entropy output ceiling.
    OutputTooLarge,
}
impl From<BitstreamError> for VideoError {
    fn from(e: BitstreamError) -> Self {
        Self::Bitstream(e)
    }
}
fn sizes(w: u32, h: u32) -> Result<(usize, usize), VideoError> {
    if w == 0
        || h == 0
        || w > 8192
        || h > 8192
        || w % 2 != 0
        || h % 2 != 0
        || u64::from(w) * u64::from(h) > MAX_PIXELS
    {
        return Err(VideoError::BadDimensions);
    }
    let y = (w as usize)
        .checked_mul(h as usize)
        .ok_or(VideoError::BadDimensions)?;
    Ok((y, y / 4))
}

mod decoder;
mod encoder;
mod prediction;

pub use decoder::decode;
use decoder::{PlaneEntropyScratch, decode_with_threads};
pub use encoder::encode;
use encoder::encode_with_threads;

/// Computes full-frame sample-domain PSNR across the Y, U, and V planes.
pub fn psnr(a: &Frame420, b: &Frame420) -> Result<f64, VideoError> {
    a.validate()?;
    b.validate()?;
    if a.width != b.width || a.height != b.height {
        return Err(VideoError::ReferenceShape);
    }
    let mut se = 0f64;
    let mut n = 0usize;
    for (x, y) in a
        .y()
        .iter()
        .chain(a.u())
        .chain(a.v())
        .zip(b.y().iter().chain(b.u()).chain(b.v()))
    {
        let d = f64::from(*x) - f64::from(*y);
        se += d * d;
        n += 1
    }
    if se == 0.0 {
        Ok(f64::INFINITY)
    } else {
        let mse = se / n as f64;
        Ok(10.0 * (255.0 * 255.0 / mse).log10())
    }
}

/// Stateful decoder retaining at most four immutable reconstructed references.
#[derive(Debug)]
pub struct VideoDecoder {
    references: std::collections::VecDeque<(u64, std::sync::Arc<Frame420>)>,
    threads: crate::config::ThreadPolicy,
    kernels: crate::kernels::KernelSet,
    scheduler: crate::scheduler::Scheduler,
    max_frame_pixels: u64,
    max_entropy_bytes: usize,
    expected_dimensions: Option<(u32, u32)>,
    entropy: [PlaneEntropyScratch; 3],
    frame_pool: Vec<Frame420>,
}
impl Default for VideoDecoder {
    fn default() -> Self {
        Self::new()
    }
}
impl VideoDecoder {
    /// Creates an empty decoder state for one container epoch.
    pub fn new() -> Self {
        Self {
            references: std::collections::VecDeque::with_capacity(4),
            threads: crate::config::ThreadPolicy::Auto,
            kernels: crate::kernels::KernelSet::auto(),
            scheduler: crate::scheduler::Scheduler::new(crate::config::ThreadPolicy::Auto),
            max_frame_pixels: crate::limits::Limits::default().max_frame_pixels,
            max_entropy_bytes: crate::limits::Limits::default().max_entropy_bytes,
            expected_dimensions: None,
            entropy: std::array::from_fn(|_| PlaneEntropyScratch::default()),
            frame_pool: Vec::with_capacity(4),
        }
    }
    /// Creates a decoder with an explicit configuration.
    pub fn with_config(
        config: crate::config::Config,
    ) -> Result<Self, crate::config::BackendUnavailable> {
        Ok(Self {
            references: std::collections::VecDeque::with_capacity(4),
            threads: config.threads,
            kernels: crate::config::kernel_set(config.cpu)?,
            scheduler: crate::scheduler::Scheduler::new(config.threads),
            max_frame_pixels: config.limits.max_frame_pixels,
            max_entropy_bytes: config.limits.max_entropy_bytes,
            expected_dimensions: None,
            entropy: std::array::from_fn(|_| PlaneEntropyScratch::default()),
            frame_pool: Vec::with_capacity(4),
        })
    }
    pub(crate) fn for_stream(
        mut config: crate::config::Config,
        width: u32,
        height: u32,
    ) -> Result<Self, crate::config::BackendUnavailable> {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        config.limits.max_frame_pixels = config.limits.max_frame_pixels.min(pixels);
        let mut decoder = Self::with_config(config)?;
        decoder.expected_dimensions = Some((width, height));
        Ok(decoder)
    }

    /// Drops all reference pictures, as required at an epoch boundary or seek reset.
    pub fn reset_epoch(&mut self) {
        while let Some((_, frame)) = self.references.pop_front() {
            self.recycle_frame(frame);
        }
    }
    fn recycle_frame(&mut self, frame: std::sync::Arc<Frame420>) {
        if self.frame_pool.len() >= 4 {
            return;
        }
        if let Ok(frame) = std::sync::Arc::try_unwrap(frame) {
            self.frame_pool.push(frame);
        }
    }
    /// Number of reusable frame allocations currently retained by this decoder.
    pub fn pooled_frame_count(&self) -> usize {
        self.frame_pool.len()
    }
    /// Number of retained reference pictures.
    pub fn reference_count(&self) -> usize {
        self.references.len()
    }
    /// Decodes one frame and shares its allocation with the retained reference history.
    pub fn decode_shared(
        &mut self,
        packet: &[u8],
    ) -> Result<(u64, std::sync::Arc<Frame420>, Vec<u64>), VideoError> {
        let refs: Vec<(u64, &Frame420)> = self
            .references
            .iter()
            .map(|(id, f)| (*id, f.as_ref()))
            .collect();
        let (id, frame, deps) = decode_with_threads(
            packet,
            &refs,
            self.threads,
            &mut self.entropy,
            self.kernels,
            Some(&self.scheduler),
            Some(&mut self.frame_pool),
            self.max_frame_pixels,
            self.max_entropy_bytes,
            self.expected_dimensions,
        )?;
        if self.references.iter().any(|(rid, _)| *rid == id) {
            return Err(VideoError::BadHeader);
        }
        let frame = std::sync::Arc::new(frame);
        self.references.push_back((id, frame.clone()));
        while self.references.len() > 4 {
            if let Some((_, old)) = self.references.pop_front() {
                self.recycle_frame(old);
            }
        }
        Ok((id, frame, deps))
    }
    /// Convenience owned decode. Prefer [`Self::decode_shared`] in zero-copy/stateful integrations.
    pub fn decode(&mut self, packet: &[u8]) -> Result<(u64, Frame420, Vec<u64>), VideoError> {
        let (id, frame, deps) = self.decode_shared(packet)?;
        Ok((id, frame.as_ref().clone(), deps))
    }
}

/// Stateful encoder retaining reconstructed reference pictures across calls.
#[derive(Debug)]
pub struct VideoEncoder {
    references: std::collections::VecDeque<(u64, std::sync::Arc<Frame420>)>,
    options: EncodeOptions,
    threads: crate::config::ThreadPolicy,
    kernels: crate::kernels::KernelSet,
    scheduler: crate::scheduler::Scheduler,
    frame_pool: Vec<Frame420>,
    max_frame_pixels: u64,
}
impl VideoEncoder {
    /// Creates an encoder using the supplied non-normative search/quantization options.
    pub fn new(options: EncodeOptions) -> Self {
        Self {
            references: std::collections::VecDeque::with_capacity(4),
            options,
            threads: crate::config::ThreadPolicy::Auto,
            kernels: crate::kernels::KernelSet::auto(),
            scheduler: crate::scheduler::Scheduler::new(crate::config::ThreadPolicy::Auto),
            frame_pool: Vec::with_capacity(4),
            max_frame_pixels: crate::limits::Limits::default().max_frame_pixels,
        }
    }
    /// Creates an encoder with explicit threading/backend configuration.
    pub fn with_config(
        options: EncodeOptions,
        config: crate::config::Config,
    ) -> Result<Self, crate::config::BackendUnavailable> {
        Ok(Self {
            references: std::collections::VecDeque::with_capacity(4),
            options,
            threads: config.threads,
            kernels: crate::config::kernel_set(config.cpu)?,
            scheduler: crate::scheduler::Scheduler::new(config.threads),
            frame_pool: Vec::with_capacity(4),
            max_frame_pixels: config.limits.max_frame_pixels,
        })
    }
    /// Drops reference history at an explicit epoch boundary.
    pub fn reset_epoch(&mut self) {
        while let Some((_, frame)) = self.references.pop_front() {
            self.recycle_frame(frame);
        }
    }
    fn recycle_frame(&mut self, frame: std::sync::Arc<Frame420>) {
        if self.frame_pool.len() >= 4 {
            return;
        }
        if let Ok(frame) = std::sync::Arc::try_unwrap(frame) {
            self.frame_pool.push(frame);
        }
    }
    /// Number of reusable reconstruction allocations retained by the encoder.
    pub fn pooled_frame_count(&self) -> usize {
        self.frame_pool.len()
    }
    /// Updates encoder policy for future frames without rewriting already reconstructed references.
    pub fn set_options(&mut self, options: EncodeOptions) {
        self.options = options;
    }
    /// Encodes a frame and shares its reconstruction allocation with reference history.
    pub fn encode_shared(
        &mut self,
        frame_id: u64,
        frame: &Frame420,
    ) -> Result<SharedEncodedFrame, VideoError> {
        frame.validate()?;
        if u64::from(frame.width) * u64::from(frame.height) > self.max_frame_pixels {
            return Err(VideoError::OutputTooLarge);
        }
        let refs: Vec<(u64, &Frame420)> = self
            .references
            .iter()
            .rev()
            .take(4)
            .map(|(id, f)| (*id, f.as_ref()))
            .collect();
        let encoded = encode_with_threads(
            frame_id,
            frame,
            &refs,
            self.options,
            self.threads,
            self.kernels,
            Some(&self.scheduler),
            Some(&mut self.frame_pool),
        )?;
        let reconstructed = std::sync::Arc::new(encoded.reconstructed);
        self.references.push_back((frame_id, reconstructed.clone()));
        while self.references.len() > 4 {
            if let Some((_, old)) = self.references.pop_front() {
                self.recycle_frame(old);
            }
        }
        Ok(SharedEncodedFrame {
            packet: encoded.packet,
            reconstructed,
            dependencies: encoded.dependencies,
        })
    }
    /// Convenience owned result. Prefer [`Self::encode_shared`] when retaining reconstruction.
    pub fn encode(&mut self, frame_id: u64, frame: &Frame420) -> Result<EncodedFrame, VideoError> {
        let encoded = self.encode_shared(frame_id, frame)?;
        Ok(EncodedFrame {
            packet: encoded.packet,
            reconstructed: encoded.reconstructed.as_ref().clone(),
            dependencies: encoded.dependencies,
        })
    }
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for VideoError {}

#[cfg(test)]
mod tests {
    use super::decoder::decode_plane_single;
    use super::*;
    fn frame(seed: u8) -> Frame420 {
        let w = 18;
        let h = 10;
        let mut y = Vec::new();
        for j in 0..h {
            for i in 0..w {
                y.push(seed.wrapping_add((i * 7 + j * 11) as u8))
            }
        }
        Frame420::from_planes(
            w,
            h,
            y,
            vec![seed; w as usize * h as usize / 4],
            vec![seed.wrapping_add(30); w as usize * h as usize / 4],
        )
        .unwrap()
    }
    #[test]
    fn stateful_encoder_reuses_uniquely_owned_evicted_frame() {
        let source = frame(23);
        let mut encoder = VideoEncoder::new(EncodeOptions {
            qstep: 1,
            max_refs: 0,
            ..Default::default()
        });
        let first = encoder.encode_shared(0, &source).unwrap();
        let first_ptr = first.reconstructed.y().as_ptr();
        drop(first);
        for id in 1..5u64 {
            let encoded = encoder.encode_shared(id, &source).unwrap();
            drop(encoded);
        }
        assert_eq!(encoder.pooled_frame_count(), 1);
        let sixth = encoder.encode_shared(5, &source).unwrap();
        assert_eq!(sixth.reconstructed.y().as_ptr(), first_ptr);
        assert_eq!(sixth.reconstructed.as_ref(), &source);
    }

    #[test]
    fn stateful_decoder_reuses_uniquely_owned_evicted_frame() {
        let source = frame(17);
        let mut packets = Vec::new();
        for id in 0..6u64 {
            packets.push(
                encode(
                    id,
                    &source,
                    &[],
                    EncodeOptions {
                        qstep: 1,
                        ..Default::default()
                    },
                )
                .unwrap()
                .packet,
            );
        }
        let mut decoder = VideoDecoder::new();
        let (_, first, _) = decoder.decode_shared(&packets[0]).unwrap();
        let first_ptr = first.y().as_ptr();
        drop(first);
        for packet in &packets[1..5] {
            let (_, decoded, _) = decoder.decode_shared(packet).unwrap();
            drop(decoded);
        }
        assert_eq!(decoder.pooled_frame_count(), 1);
        let (_, sixth, _) = decoder.decode_shared(&packets[5]).unwrap();
        assert_eq!(sixth.y().as_ptr(), first_ptr);
        assert_eq!(sixth.as_ref(), &source);
    }

    #[test]
    fn lossless_exact() {
        let f = frame(3);
        let e = encode(
            0,
            &f,
            &[],
            EncodeOptions {
                qstep: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, d, _) = decode(&e.packet, &[]).unwrap();
        assert_eq!(d, f)
    }
    #[test]
    fn lossy_encoder_matches_decoder() {
        let a = frame(3);
        let e0 = encode(
            0,
            &a,
            &[],
            EncodeOptions {
                qstep: 64,
                ..Default::default()
            },
        )
        .unwrap();
        let b = frame(5);
        let e1 = encode(
            1,
            &b,
            &[(0, &e0.reconstructed)],
            EncodeOptions {
                qstep: 64,
                max_refs: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, d, _) = decode(&e1.packet, &[(0, &e0.reconstructed)]).unwrap();
        assert_eq!(d, e1.reconstructed)
    }
    #[test]
    fn extreme_signed_tokens_are_rejected_without_abs_overflow() {
        let reference = [0u8; 64];
        let refs: [&[u8]; 1] = [&reference];

        let mut motion = vec![3, 0];
        put_svarint_i32(i32::MIN, &mut motion);
        put_svarint_i32(0, &mut motion);
        put_uvarint(0, &mut motion);
        assert!(matches!(
            decode_plane_single(&motion, &refs, 8, 8, 1, crate::kernels::KernelSet::scalar()),
            Err(VideoError::BadMode)
        ));

        let mut coeff = vec![0];
        put_uvarint(1, &mut coeff);
        put_uvarint(0, &mut coeff);
        put_svarint_i32(i32::MIN, &mut coeff);
        assert!(matches!(
            decode_plane_single(&coeff, &[], 8, 8, 1, crate::kernels::KernelSet::scalar()),
            Err(VideoError::BadCoefficient)
        ));
    }

    #[test]
    fn multiple_refs_decode() {
        let a = frame(1);
        let b = frame(80);
        let cur = frame(2);
        let e = encode(
            2,
            &cur,
            &[(0, &a), (1, &b)],
            EncodeOptions {
                qstep: 1,
                max_refs: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, d, _) = decode(&e.packet, &[(0, &a), (1, &b)]).unwrap();
        assert_eq!(d, cur)
    }
}
