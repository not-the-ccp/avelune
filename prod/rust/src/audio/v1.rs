//! Production-owned scalar ALA1 V1 codec implementation.
//!
//! `ALA1` is Draft Generation 1 and is not frozen. `qstep = 1` is mathematically
//! lossless; larger quantizers are experimental lossy modes.

// The normative transform/token loops intentionally stay straightforward and auditable.
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

/// Four-byte packet magic for the current draft audio generation.
pub const CODEC_MAGIC: [u8; 4] = *b"ALA1";
/// Transform block length in sample frames per channel.
pub const BLOCK: usize = 64;
/// Maximum sample frames accepted by a single codec packet.
pub const MAX_FRAMES: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq)]
/// Errors from ALA1 packet encoding or decoding.
pub enum AudioError {
    BadHeader,
    BadChannels,
    BadRate,
    BadFrames,
    BadQuantizer,
    UnexpectedEof,
    Bitstream(BitstreamError),
    BadCoefficient,
    SampleOverflow,
    TrailingData,
}
impl From<BitstreamError> for AudioError {
    fn from(e: BitstreamError) -> Self {
        Self::Bitstream(e)
    }
}
#[derive(Debug, Clone, Copy)]
/// Encoder options for one ALA1 packet.
pub struct EncodeOptions {
    /// PCM sample rate in Hz.
    pub sample_rate: u32,
    /// Interleaved channel count. The current container baseline normally uses one or two.
    pub channels: u8,
    /// Quantizer step. `1` selects mathematically lossless coding.
    pub qstep: u16,
    /// Enables reversible mid/side coupling for stereo input.
    pub mid_side: bool,
}
impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            qstep: 512,
            mid_side: true,
        }
    }
}

fn fwd_haar(v: &mut [i32; BLOCK]) {
    let mut len = BLOCK;
    let mut tmp = [0i32; BLOCK];
    while len > 1 {
        let half = len / 2;
        for i in 0..half {
            let a = v[2 * i];
            let b = v[2 * i + 1];
            let d = b - a;
            let s = a + (d >> 1);
            tmp[i] = s;
            tmp[half + i] = d
        }
        v[..len].copy_from_slice(&tmp[..len]);
        len = half;
    }
}
fn inv_haar(v: &mut [i32; BLOCK]) -> Result<(), AudioError> {
    let mut len = 2;
    let mut tmp = [0i32; BLOCK];
    while len <= BLOCK {
        let half = len / 2;
        for i in 0..half {
            let s = v[i];
            let d = v[half + i];
            let a = s.checked_sub(d >> 1).ok_or(AudioError::BadCoefficient)?;
            let b = d.checked_add(a).ok_or(AudioError::BadCoefficient)?;
            tmp[2 * i] = a;
            tmp[2 * i + 1] = b
        }
        v[..len].copy_from_slice(&tmp[..len]);
        len *= 2;
    }
    Ok(())
}
fn div_round(v: i32, d: i32) -> i32 {
    if v >= 0 {
        (v + d / 2) / d
    } else {
        -((-v + d / 2) / d)
    }
}
fn q_for(i: usize, q: i32) -> i32 {
    if q <= 1 {
        1
    } else if i < 4 {
        q
    } else if i < 16 {
        q.saturating_mul(2)
    } else {
        q.saturating_mul(3)
    }
}

/// Encodes one interleaved signed-16 PCM packet into ALA1.
pub fn encode(interleaved: &[i16], opt: EncodeOptions) -> Result<Vec<u8>, AudioError> {
    let ch = opt.channels as usize;
    if ch == 0 || ch > 8 {
        return Err(AudioError::BadChannels);
    }
    if opt.sample_rate < 8_000 || opt.sample_rate > 384_000 {
        return Err(AudioError::BadRate);
    }
    if opt.qstep == 0 {
        return Err(AudioError::BadQuantizer);
    }
    if interleaved.len() % ch != 0 {
        return Err(AudioError::BadFrames);
    }
    let frames = interleaved.len() / ch;
    if frames == 0 || frames > MAX_FRAMES {
        return Err(AudioError::BadFrames);
    }
    let mut planes = vec![0i32; frames * ch];
    for f in 0..frames {
        for c in 0..ch {
            planes[c * frames + f] = i32::from(interleaved[f * ch + c])
        }
    }
    let use_ms = opt.mid_side && ch == 2;
    if use_ms {
        for i in 0..frames {
            let l = planes[i];
            let r = planes[frames + i];
            let side = r - l;
            let mid = l + (side >> 1);
            planes[i] = mid;
            planes[frames + i] = side
        }
    }
    let q = i32::from(opt.qstep);
    let blocks = (frames + BLOCK - 1) / BLOCK;
    let mut tok = Vec::new();
    put_uvarint(blocks as u64, &mut tok);
    for c in 0..ch {
        for b in 0..blocks {
            let mut a = [0i32; BLOCK];
            let start = b * BLOCK;
            for i in 0..BLOCK {
                if start + i < frames {
                    a[i] = planes[c * frames + start + i]
                }
            }
            fwd_haar(&mut a);
            let mut qc = [0i32; BLOCK];
            for i in 0..BLOCK {
                qc[i] = div_round(a[i], q_for(i, q))
            }
            // Lossy coefficient rounding can overshoot signed-16 sample range after the
            // normative inverse transform. A production encoder must not emit a packet that a
            // conforming decoder is required to reject. Validate the exact reconstruction before
            // serializing it. For mid/side, reconstructed transform-domain planes are retained
            // here and the coupled L/R range check runs after both channels are available.
            if q > 1 {
                let mut reconstructed = [0i32; BLOCK];
                for i in 0..BLOCK {
                    reconstructed[i] = qc[i]
                        .checked_mul(q_for(i, q))
                        .ok_or(AudioError::BadCoefficient)?;
                }
                inv_haar(&mut reconstructed)?;
                for i in 0..BLOCK {
                    if start + i >= frames {
                        break;
                    }
                    if use_ms {
                        planes[c * frames + start + i] = reconstructed[i];
                    } else if i16::try_from(reconstructed[i]).is_err() {
                        return Err(AudioError::SampleOverflow);
                    }
                }
            }
            let nz = qc.iter().filter(|&&v| v != 0).count();
            put_uvarint(nz as u64, &mut tok);
            for (i, &v) in qc.iter().enumerate() {
                if v != 0 {
                    put_uvarint(i as u64, &mut tok);
                    put_svarint_i32(v, &mut tok)
                }
            }
        }
    }
    if use_ms && q > 1 {
        for i in 0..frames {
            let mid = planes[i];
            let side = planes[frames + i];
            let l = mid
                .checked_sub(side >> 1)
                .ok_or(AudioError::SampleOverflow)?;
            let r = side.checked_add(l).ok_or(AudioError::SampleOverflow)?;
            if i16::try_from(l).is_err() || i16::try_from(r).is_err() {
                return Err(AudioError::SampleOverflow);
            }
        }
    }
    let coded = entropy_compress(&tok);
    let mut out = Vec::new();
    out.extend(CODEC_MAGIC);
    out.push(opt.channels);
    out.push(if use_ms { 1 } else { 0 });
    out.extend(opt.qstep.to_le_bytes());
    out.extend(opt.sample_rate.to_le_bytes());
    out.extend((frames as u16).to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend((coded.len() as u32).to_le_bytes());
    out.extend(coded);
    Ok(out)
}

/// Decodes one complete ALA1 packet to `(sample_rate, channels, interleaved_pcm)`.
pub fn decode(input: &[u8]) -> Result<(u32, u8, Vec<i16>), AudioError> {
    let mut scratch = EntropyScratch::new();
    decode_with_scratch(
        input,
        &mut scratch,
        crate::limits::Limits::default().max_entropy_bytes,
    )
}

fn decode_with_scratch(
    input: &[u8],
    scratch: &mut EntropyScratch,
    max_entropy_bytes: usize,
) -> Result<(u32, u8, Vec<i16>), AudioError> {
    let mut planes = Vec::new();
    let mut out = Vec::new();
    let (rate, channels) =
        decode_with_buffers(input, scratch, max_entropy_bytes, &mut planes, &mut out)?;
    Ok((rate, channels, out))
}

fn decode_with_buffers(
    input: &[u8],
    scratch: &mut EntropyScratch,
    max_entropy_bytes: usize,
    planes: &mut Vec<i32>,
    out: &mut Vec<i16>,
) -> Result<(u32, u8), AudioError> {
    if input.len() < 20 {
        return Err(AudioError::UnexpectedEof);
    }
    if input[..4] != CODEC_MAGIC {
        return Err(AudioError::BadHeader);
    }
    let ch = input[4] as usize;
    if ch == 0 || ch > 8 {
        return Err(AudioError::BadChannels);
    }
    let flags = input[5];
    if flags & !1 != 0 {
        return Err(AudioError::BadHeader);
    }
    let q = u16::from_le_bytes(input[6..8].try_into().unwrap());
    if q == 0 {
        return Err(AudioError::BadQuantizer);
    }
    let rate = u32::from_le_bytes(input[8..12].try_into().unwrap());
    if rate < 8_000 || rate > 384_000 {
        return Err(AudioError::BadRate);
    }
    let frames = u16::from_le_bytes(input[12..14].try_into().unwrap()) as usize;
    if input[14] != 0 || input[15] != 0 {
        return Err(AudioError::BadHeader);
    }
    if frames == 0 || frames > MAX_FRAMES {
        return Err(AudioError::BadFrames);
    }
    let n = u32::from_le_bytes(input[16..20].try_into().unwrap()) as usize;
    let end = 20usize.checked_add(n).ok_or(AudioError::BadHeader)?;
    if input.len() != end {
        return Err(if input.len() < end {
            AudioError::UnexpectedEof
        } else {
            AudioError::TrailingData
        });
    }
    let tok = entropy_decompress_with_scratch(&input[20..], max_entropy_bytes, scratch)?;
    let mut p = 0;
    let blocks = usize::try_from(get_uvarint(tok, &mut p)?).map_err(|_| AudioError::BadHeader)?;
    if blocks != (frames + 63) / 64 {
        return Err(AudioError::BadHeader);
    }
    planes.clear();
    planes.resize(frames * ch, 0);
    for c in 0..ch {
        for b in 0..blocks {
            let nz = usize::try_from(get_uvarint(tok, &mut p)?)
                .map_err(|_| AudioError::BadCoefficient)?;
            if nz > 64 {
                return Err(AudioError::BadCoefficient);
            }
            let mut a = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = usize::try_from(get_uvarint(tok, &mut p)?)
                    .map_err(|_| AudioError::BadCoefficient)?;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(AudioError::BadCoefficient);
                }
                let v = get_svarint_i32(tok, &mut p)?;
                if v.unsigned_abs() > 2_000_000 {
                    return Err(AudioError::BadCoefficient);
                }
                a[i] = v
                    .checked_mul(q_for(i, i32::from(q)))
                    .ok_or(AudioError::BadCoefficient)?;
                last = Some(i)
            }
            inv_haar(&mut a)?;
            let start = b * 64;
            for i in 0..64 {
                if start + i < frames {
                    planes[c * frames + start + i] = a[i]
                }
            }
        }
    }
    if p != tok.len() {
        return Err(AudioError::TrailingData);
    }
    if flags & 1 != 0 {
        if ch != 2 {
            return Err(AudioError::BadHeader);
        }
        for i in 0..frames {
            let mid = planes[i];
            let side = planes[frames + i];
            let l = mid
                .checked_sub(side >> 1)
                .ok_or(AudioError::SampleOverflow)?;
            let r = side.checked_add(l).ok_or(AudioError::SampleOverflow)?;
            planes[i] = l;
            planes[frames + i] = r
        }
    }
    out.clear();
    out.reserve(frames * ch);
    for f in 0..frames {
        for c in 0..ch {
            out.push(i16::try_from(planes[c * frames + f]).map_err(|_| AudioError::SampleOverflow)?)
        }
    }
    Ok((rate, ch as u8))
}

/// Computes sample-domain SNR in dB for equal-length PCM sequences.
pub fn snr_db(a: &[i16], b: &[i16]) -> Result<f64, AudioError> {
    if a.len() != b.len() {
        return Err(AudioError::BadFrames);
    }
    let mut sig = 0f64;
    let mut err = 0f64;
    for (&x, &y) in a.iter().zip(b) {
        sig += f64::from(x) * f64::from(x);
        let d = f64::from(x) - f64::from(y);
        err += d * d
    }
    if err == 0.0 {
        Ok(f64::INFINITY)
    } else {
        Ok(10.0 * (sig.max(1.0) / err).log10())
    }
}

/// Reusable ALA1 decoder façade. ALA1 has no cross-packet predictor state.
#[derive(Debug)]
pub struct AudioDecoder {
    entropy: EntropyScratch,
    planes: Vec<i32>,
    max_entropy_bytes: usize,
}
impl Default for AudioDecoder {
    fn default() -> Self {
        Self {
            entropy: EntropyScratch::default(),
            planes: Vec::new(),
            max_entropy_bytes: crate::limits::Limits::default().max_entropy_bytes,
        }
    }
}
impl AudioDecoder {
    /// Creates a decoder instance with default resource limits.
    pub fn new() -> Self {
        Self::default()
    }
    /// Creates a decoder with explicit defensive resource limits.
    pub fn with_limits(limits: crate::limits::Limits) -> Self {
        Self {
            entropy: EntropyScratch::default(),
            planes: Vec::new(),
            max_entropy_bytes: limits.max_entropy_bytes,
        }
    }
    /// Decodes one complete ALA1 packet.
    pub fn decode(&mut self, input: &[u8]) -> Result<(u32, u8, Vec<i16>), AudioError> {
        let mut out = Vec::new();
        let (rate, channels) = decode_with_buffers(
            input,
            &mut self.entropy,
            self.max_entropy_bytes,
            &mut self.planes,
            &mut out,
        )?;
        Ok((rate, channels, out))
    }
    /// Decodes into caller-owned PCM storage, retaining entropy and planar scratch allocations.
    pub fn decode_into(
        &mut self,
        input: &[u8],
        out: &mut Vec<i16>,
    ) -> Result<(u32, u8), AudioError> {
        decode_with_buffers(
            input,
            &mut self.entropy,
            self.max_entropy_bytes,
            &mut self.planes,
            out,
        )
    }
    /// Capacity of the reusable planar i32 scratch buffer.
    pub fn scratch_sample_capacity(&self) -> usize {
        self.planes.capacity()
    }
}

/// Reusable ALA1 encoder façade carrying stable packet policy.
#[derive(Debug)]
pub struct AudioEncoder {
    options: EncodeOptions,
}
impl AudioEncoder {
    /// Creates an encoder with packet options.
    pub const fn new(options: EncodeOptions) -> Self {
        Self { options }
    }
    /// Updates packet policy.
    pub fn set_options(&mut self, options: EncodeOptions) {
        self.options = options;
    }
    /// Encodes one interleaved PCM packet.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, AudioError> {
        encode(pcm, self.options)
    }
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn samples() -> Vec<i16> {
        (0..960 * 2)
            .map(|i| ((((i * 37) % 1000) - 500) * 40) as i16)
            .collect()
    }
    #[test]
    fn exact_q1() {
        let s = samples();
        let e = encode(
            &s,
            EncodeOptions {
                qstep: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, _, d) = decode(&e).unwrap();
        assert_eq!(s, d)
    }
    #[test]
    fn lossy_decodes() {
        let s = samples();
        let e = encode(
            &s,
            EncodeOptions {
                qstep: 128,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, _, d) = decode(&e).unwrap();
        assert_eq!(d.len(), s.len());
        assert!(snr_db(&s, &d).unwrap() > 20.0)
    }
    #[test]
    fn stateful_decode_into_reuses_planar_and_output_capacity() {
        let s = samples();
        let e = encode(
            &s,
            EncodeOptions {
                qstep: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut decoder = AudioDecoder::new();
        let mut out = Vec::new();
        let meta = decoder.decode_into(&e, &mut out).unwrap();
        assert_eq!(meta, (48_000, 2));
        assert_eq!(out, s);
        let planar_capacity = decoder.scratch_sample_capacity();
        let out_capacity = out.capacity();
        out.clear();
        decoder.decode_into(&e, &mut out).unwrap();
        assert_eq!(decoder.scratch_sample_capacity(), planar_capacity);
        assert_eq!(out.capacity(), out_capacity);
        assert_eq!(out, s);
    }

    #[test]
    fn mono_exact() {
        let s: Vec<i16> = (0..960).map(|i| (i as i16).wrapping_mul(31)).collect();
        let e = encode(
            &s,
            EncodeOptions {
                channels: 1,
                qstep: 1,
                mid_side: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(decode(&e).unwrap().2, s)
    }
}
