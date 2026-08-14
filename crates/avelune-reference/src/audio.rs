//! Reference implementation of the experimental `ALA1` audio bitstream.
//!
//! `ALA1` is Draft Generation 1 and is not frozen. `qstep = 1` is mathematically
//! lossless; larger quantizers are experimental lossy modes.

// The reference implementation intentionally mirrors spec-shaped loops and argument lists.
use crate::bitstream::{
    BitstreamError, entropy_compress, entropy_decompress, get_svarint_i32, get_uvarint,
    put_svarint_i32, put_uvarint,
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
    let mut planes = vec![vec![0i32; frames]; ch];
    for f in 0..frames {
        for c in 0..ch {
            planes[c][f] = i32::from(interleaved[f * ch + c])
        }
    }
    let use_ms = opt.mid_side && ch == 2;
    if use_ms {
        for i in 0..frames {
            let l = planes[0][i];
            let r = planes[1][i];
            let side = r - l;
            let mid = l + (side >> 1);
            planes[0][i] = mid;
            planes[1][i] = side
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
                    a[i] = planes[c][start + i]
                }
            }
            fwd_haar(&mut a);
            let mut qc = [0i32; BLOCK];
            for i in 0..BLOCK {
                qc[i] = div_round(a[i], q_for(i, q))
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
    if frames == 0 || frames > MAX_FRAMES {
        return Err(AudioError::BadFrames);
    }
    let n = u32::from_le_bytes(input[16..20].try_into().unwrap()) as usize;
    if input.len() != 20 + n {
        return Err(if input.len() < 20 + n {
            AudioError::UnexpectedEof
        } else {
            AudioError::TrailingData
        });
    }
    let tok = entropy_decompress(&input[20..], 4 * 1024 * 1024)?;
    let mut p = 0;
    let blocks = get_uvarint(&tok, &mut p)? as usize;
    if blocks != (frames + 63) / 64 {
        return Err(AudioError::BadHeader);
    }
    let mut planes = vec![vec![0i32; frames]; ch];
    for c in 0..ch {
        for b in 0..blocks {
            let nz = get_uvarint(&tok, &mut p)? as usize;
            if nz > 64 {
                return Err(AudioError::BadCoefficient);
            }
            let mut a = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = get_uvarint(&tok, &mut p)? as usize;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(AudioError::BadCoefficient);
                }
                let v = get_svarint_i32(&tok, &mut p)?;
                if v.abs() > 2_000_000 {
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
                    planes[c][start + i] = a[i]
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
            let mid = planes[0][i];
            let side = planes[1][i];
            let l = mid
                .checked_sub(side >> 1)
                .ok_or(AudioError::SampleOverflow)?;
            let r = side.checked_add(l).ok_or(AudioError::SampleOverflow)?;
            planes[0][i] = l;
            planes[1][i] = r
        }
    }
    let mut out = Vec::with_capacity(frames * ch);
    for f in 0..frames {
        for c in 0..ch {
            out.push(i16::try_from(planes[c][f]).map_err(|_| AudioError::SampleOverflow)?)
        }
    }
    Ok((rate, ch as u8, out))
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
