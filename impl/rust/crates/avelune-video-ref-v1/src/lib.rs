#![forbid(unsafe_code)]
// The reference implementation intentionally mirrors spec-shaped loops and argument lists.
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
//! Scalar, specification-shaped ALV1 decoder.
//!
//! This crate intentionally does not depend on `avelune-video-v1` in normal
//! builds. It duplicates normative reconstruction math so differential tests can
//! catch accidental dependencies on production-decoder implementation details.

use avelune_bitstream::{BitstreamError, entropy_decompress, get_svarint_i32, get_uvarint};

/// Four-byte ALV1 packet magic.
pub const CODEC_MAGIC: [u8; 4] = *b"ALV1";
const BLOCK: usize = 8;
const MAX_PIXELS: u64 = 8192 * 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Owned decoded 8-bit planar 4:2:0 frame.
pub struct Frame420 {
    /// Luma width in pixels.
    pub width: u32,
    /// Luma height in pixels.
    pub height: u32,
    /// Luma plane.
    pub y: Vec<u8>,
    /// Cb/U plane.
    pub u: Vec<u8>,
    /// Cr/V plane.
    pub v: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Errors returned by the scalar reference decoder.
pub enum Error {
    BadDimensions,
    UnexpectedEof,
    BadHeader,
    BadMode,
    BadQuantizer,
    ReferenceMissing(u64),
    ReferenceShape,
    Bitstream(BitstreamError),
    BadCoefficient,
    BadPalette,
    TrailingData,
}
impl From<BitstreamError> for Error {
    fn from(value: BitstreamError) -> Self {
        Self::Bitstream(value)
    }
}

fn sizes(w: u32, h: u32) -> Result<(usize, usize), Error> {
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(Error::BadDimensions);
    }
    let y = (w as usize)
        .checked_mul(h as usize)
        .ok_or(Error::BadDimensions)?;
    Ok((y, y / 4))
}
fn plane(f: &Frame420, p: usize) -> &[u8] {
    match p {
        0 => &f.y,
        1 => &f.u,
        _ => &f.v,
    }
}

fn hadamard8(v: &mut [i32; 8]) {
    let mut span = 1;
    while span < 8 {
        for i in (0..8).step_by(span * 2) {
            for j in i..i + span {
                let a = v[j];
                let b = v[j + span];
                v[j] = a + b;
                v[j + span] = a - b;
            }
        }
        span *= 2;
    }
}
fn wht2(mut a: [i32; 64]) -> [i32; 64] {
    for y in 0..8 {
        let mut row = [0i32; 8];
        row.copy_from_slice(&a[y * 8..y * 8 + 8]);
        hadamard8(&mut row);
        a[y * 8..y * 8 + 8].copy_from_slice(&row);
    }
    for x in 0..8 {
        let mut col = [0i32; 8];
        for y in 0..8 {
            col[y] = a[y * 8 + x];
        }
        hadamard8(&mut col);
        for y in 0..8 {
            a[y * 8 + x] = col[y];
        }
    }
    a
}
fn round_div_64(v: i32) -> i32 {
    if v >= 0 {
        (v + 32) / 64
    } else {
        -((-v + 32) / 64)
    }
}
fn inverse_transform(a: [i32; 64]) -> [i32; 64] {
    let b = wht2(a);
    let mut out = [0i32; 64];
    for i in 0..64 {
        out[i] = round_div_64(b[i]);
    }
    out
}
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn intra(
    recon: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    x: usize,
    y: usize,
    mode: u8,
) -> u8 {
    match mode {
        1 => {
            if bx > 0 {
                recon[(by + y).min(h - 1) * w + bx - 1]
            } else {
                128
            }
        }
        2 => {
            if by > 0 {
                recon[(by - 1) * w + (bx + x).min(w - 1)]
            } else {
                128
            }
        }
        _ => {
            let mut sum = 0u32;
            let mut n = 0u32;
            if by > 0 {
                for xx in 0..BLOCK {
                    if bx + xx < w {
                        sum += u32::from(recon[(by - 1) * w + bx + xx]);
                        n += 1;
                    }
                }
            }
            if bx > 0 {
                for yy in 0..BLOCK {
                    if by + yy < h {
                        sum += u32::from(recon[(by + yy) * w + bx - 1]);
                        n += 1;
                    }
                }
            }
            if n == 0 {
                128
            } else {
                ((sum + n / 2) / n) as u8
            }
        }
    }
}

/// Literal normative half-sample bilinear interpolation. The production decoder
/// is free to optimize this as long as it produces identical samples.
fn sample_half(src: &[u8], w: usize, h: usize, x2: i32, y2: i32) -> u8 {
    let x0 = x2.div_euclid(2);
    let y0 = y2.div_euclid(2);
    let fx = x2.rem_euclid(2);
    let fy = y2.rem_euclid(2);
    let at = |x: i32, y: i32| -> i32 {
        let xx = x.clamp(0, w as i32 - 1) as usize;
        let yy = y.clamp(0, h as i32 - 1) as usize;
        i32::from(src[yy * w + xx])
    };
    let a = at(x0, y0);
    let b = at(x0 + 1, y0);
    let c = at(x0, y0 + 1);
    let d = at(x0 + 1, y0 + 1);
    ((a * (2 - fx) * (2 - fy) + b * fx * (2 - fy) + c * (2 - fx) * fy + d * fx * fy + 2) / 4) as u8
}

fn decode_plane_single(
    tokens: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
) -> Result<Vec<u8>, Error> {
    let mut pos = 0usize;
    let mut recon = vec![128u8; w * h];
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let mode = *tokens.get(pos).ok_or(Error::UnexpectedEof)?;
            pos += 1;
            if mode == 4 {
                let n = *tokens.get(pos).ok_or(Error::UnexpectedEof)? as usize;
                pos += 1;
                if n == 0 || n > 4 || tokens.len() < pos + n + 1 {
                    return Err(Error::BadPalette);
                }
                let colors = &tokens[pos..pos + n];
                pos += n;
                let count = *tokens.get(pos).ok_or(Error::UnexpectedEof)? as usize;
                pos += 1;
                let expected = (w - bx).min(8) * (h - by).min(8);
                if count != expected {
                    return Err(Error::BadPalette);
                }
                let bytes = (count + 3) / 4;
                if tokens.len() < pos + bytes {
                    return Err(Error::UnexpectedEof);
                }
                let packed = &tokens[pos..pos + bytes];
                pos += bytes;
                let mut k = 0;
                for y in 0..8 {
                    if by + y >= h {
                        break;
                    }
                    for x in 0..8 {
                        if bx + x >= w {
                            break;
                        }
                        let ci = (packed[k / 4] >> ((k % 4) * 2)) & 3;
                        if ci as usize >= n {
                            return Err(Error::BadPalette);
                        }
                        recon[(by + y) * w + bx + x] = colors[ci as usize];
                        k += 1
                    }
                }
                continue;
            }
            if mode > 3 {
                return Err(Error::BadMode);
            }
            let (mut ri, mut dx, mut dy) = (0usize, 0i32, 0i32);
            if mode == 3 {
                ri = *tokens.get(pos).ok_or(Error::UnexpectedEof)? as usize;
                pos += 1;
                if ri >= refs.len() {
                    return Err(Error::BadMode);
                }
                dx = get_svarint_i32(tokens, &mut pos)?;
                dy = get_svarint_i32(tokens, &mut pos)?;
                if dx.abs() > 64 || dy.abs() > 64 {
                    return Err(Error::BadMode);
                }
            }
            let nz = get_uvarint(tokens, &mut pos)? as usize;
            if nz > 64 {
                return Err(Error::BadCoefficient);
            }
            let mut qc = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = get_uvarint(tokens, &mut pos)? as usize;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(Error::BadCoefficient);
                }
                let v = get_svarint_i32(tokens, &mut pos)?;
                if v.abs() > 1_000_000 {
                    return Err(Error::BadCoefficient);
                }
                qc[i] = v;
                last = Some(i)
            }
            let mut deq = [0i32; 64];
            for i in 0..64 {
                deq[i] = qc[i].checked_mul(q).ok_or(Error::BadCoefficient)?;
                if deq[i].unsigned_abs() > 33_554_431 {
                    return Err(Error::BadCoefficient);
                }
            }
            let rr = inverse_transform(deq);
            for y in 0..8 {
                for x in 0..8 {
                    if bx + x >= w || by + y >= h {
                        continue;
                    }
                    let pred = if mode == 3 {
                        sample_half(
                            refs[ri],
                            w,
                            h,
                            ((bx + x) as i32) * 2 + dx,
                            ((by + y) as i32) * 2 + dy,
                        )
                    } else {
                        intra(&recon, w, h, bx, by, x, y, mode)
                    };
                    recon[(by + y) * w + bx + x] = clip(i32::from(pred) + rr[y * 8 + x]);
                }
            }
        }
    }
    if pos != tokens.len() {
        return Err(Error::TrailingData);
    }
    Ok(recon)
}

fn decode_plane(
    control: &[u8],
    data: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
) -> Result<Vec<u8>, Error> {
    let mut cp = 0usize;
    let mut dp = 0usize;
    let mut recon = vec![128u8; w * h];
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let mode = *control.get(cp).ok_or(Error::UnexpectedEof)?;
            cp += 1;
            if mode == 4 {
                let n = *data.get(dp).ok_or(Error::UnexpectedEof)? as usize;
                dp += 1;
                if n == 0 || n > 4 || data.len() < dp + n + 1 {
                    return Err(Error::BadPalette);
                }
                let colors = &data[dp..dp + n];
                dp += n;
                let count = *data.get(dp).ok_or(Error::UnexpectedEof)? as usize;
                dp += 1;
                let expected = (w - bx).min(8) * (h - by).min(8);
                if count != expected {
                    return Err(Error::BadPalette);
                }
                let bytes = (count + 3) / 4;
                if data.len() < dp + bytes {
                    return Err(Error::UnexpectedEof);
                }
                let packed = &data[dp..dp + bytes];
                dp += bytes;
                let mut k = 0;
                for y in 0..8 {
                    if by + y >= h {
                        break;
                    }
                    for x in 0..8 {
                        if bx + x >= w {
                            break;
                        }
                        let ci = (packed[k / 4] >> ((k % 4) * 2)) & 3;
                        if ci as usize >= n {
                            return Err(Error::BadPalette);
                        }
                        recon[(by + y) * w + bx + x] = colors[ci as usize];
                        k += 1;
                    }
                }
                continue;
            }
            if mode > 3 {
                return Err(Error::BadMode);
            }
            let (mut ri, mut dx, mut dy) = (0usize, 0i32, 0i32);
            if mode == 3 {
                ri = *data.get(dp).ok_or(Error::UnexpectedEof)? as usize;
                dp += 1;
                if ri >= refs.len() {
                    return Err(Error::BadMode);
                }
                dx = get_svarint_i32(data, &mut dp)?;
                dy = get_svarint_i32(data, &mut dp)?;
                if dx.abs() > 64 || dy.abs() > 64 {
                    return Err(Error::BadMode);
                }
            }
            let nz = get_uvarint(data, &mut dp)? as usize;
            if nz > 64 {
                return Err(Error::BadCoefficient);
            }
            let mut qc = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = get_uvarint(data, &mut dp)? as usize;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(Error::BadCoefficient);
                }
                let v = get_svarint_i32(data, &mut dp)?;
                if v.abs() > 1_000_000 {
                    return Err(Error::BadCoefficient);
                }
                qc[i] = v;
                last = Some(i);
            }
            let mut deq = [0i32; 64];
            for i in 0..64 {
                deq[i] = qc[i].checked_mul(q).ok_or(Error::BadCoefficient)?;
                if deq[i].unsigned_abs() > 33_554_431 {
                    return Err(Error::BadCoefficient);
                }
            }
            let rr = inverse_transform(deq);
            for y in 0..8 {
                for x in 0..8 {
                    if bx + x >= w || by + y >= h {
                        continue;
                    }
                    let pred = if mode == 3 {
                        sample_half(
                            refs[ri],
                            w,
                            h,
                            ((bx + x) as i32) * 2 + dx,
                            ((by + y) as i32) * 2 + dy,
                        )
                    } else {
                        intra(&recon, w, h, bx, by, x, y, mode)
                    };
                    recon[(by + y) * w + bx + x] = clip(i32::from(pred) + rr[y * 8 + x]);
                }
            }
        }
    }
    if cp != control.len() || dp != data.len() {
        return Err(Error::TrailingData);
    }
    Ok(recon)
}

/// Decodes one ALV1 packet using literal specification-shaped reconstruction rules.
pub fn decode(
    input: &[u8],
    references: &[(u64, &Frame420)],
) -> Result<(u64, Frame420, Vec<u64>), Error> {
    if input.len() < 20 {
        return Err(Error::UnexpectedEof);
    }
    if input[..4] != CODEC_MAGIC {
        return Err(Error::BadHeader);
    }
    let frame_id = u64::from_le_bytes(input[4..12].try_into().unwrap());
    let w = u16::from_le_bytes(input[12..14].try_into().unwrap()) as u32;
    let h = u16::from_le_bytes(input[14..16].try_into().unwrap()) as u32;
    let q = u16::from_le_bytes(input[16..18].try_into().unwrap());
    if q == 0 {
        return Err(Error::BadQuantizer);
    }
    sizes(w, h)?;
    let rc = input[18] as usize;
    let flags = input[19];
    if rc > 4 || flags != 1 {
        return Err(Error::BadHeader);
    }
    let mut pos = 20usize;
    let mut dep = Vec::new();
    let mut refs = Vec::new();
    for _ in 0..rc {
        if input.len() < pos + 8 {
            return Err(Error::UnexpectedEof);
        }
        let id = u64::from_le_bytes(input[pos..pos + 8].try_into().unwrap());
        pos += 8;
        if dep.contains(&id) || id == frame_id {
            return Err(Error::BadHeader);
        }
        let r = references
            .iter()
            .find(|(rid, _)| *rid == id)
            .ok_or(Error::ReferenceMissing(id))?
            .1;
        if r.width != w || r.height != h {
            return Err(Error::ReferenceShape);
        }
        dep.push(id);
        refs.push(r)
    }
    let mut planes = Vec::new();
    for p in 0..3 {
        let layout = *input.get(pos).ok_or(Error::UnexpectedEof)?;
        pos += 1;
        let (pw, ph) = if p == 0 {
            (w as usize, h as usize)
        } else {
            (w as usize / 2, h as usize / 2)
        };
        let rplanes: Vec<&[u8]> = refs.iter().map(|r| plane(r, p)).collect();
        if layout == 0 {
            if input.len() < pos + 4 {
                return Err(Error::UnexpectedEof);
            }
            let n = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if input.len() < pos + n {
                return Err(Error::UnexpectedEof);
            }
            let tok = entropy_decompress(&input[pos..pos + n], 8 * 1024 * 1024)?;
            pos += n;
            planes.push(decode_plane_single(&tok, &rplanes, pw, ph, i32::from(q))?);
        } else if layout == 1 {
            if input.len() < pos + 4 {
                return Err(Error::UnexpectedEof);
            }
            let cn = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if input.len() < pos + cn + 4 {
                return Err(Error::UnexpectedEof);
            }
            let control = entropy_decompress(&input[pos..pos + cn], 8 * 1024 * 1024)?;
            pos += cn;
            let dn = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if input.len() < pos + dn {
                return Err(Error::UnexpectedEof);
            }
            let data = entropy_decompress(&input[pos..pos + dn], 8 * 1024 * 1024)?;
            pos += dn;
            planes.push(decode_plane(
                &control,
                &data,
                &rplanes,
                pw,
                ph,
                i32::from(q),
            )?);
        } else {
            return Err(Error::BadHeader);
        }
    }
    if pos != input.len() {
        return Err(Error::TrailingData);
    }
    Ok((
        frame_id,
        Frame420 {
            width: w,
            height: h,
            y: planes.remove(0),
            u: planes.remove(0),
            v: planes.remove(0),
        },
        dep,
    ))
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use avelune_video_v1::{EncodeOptions, Frame420 as ProdFrame};

    fn prod_frame(seed: u8, w: u32, h: u32) -> ProdFrame {
        let mut y = vec![0; (w * h) as usize];
        for yy in 0..h as usize {
            for xx in 0..w as usize {
                y[yy * w as usize + xx] = seed.wrapping_add((xx * 7 + yy * 13) as u8);
            }
        }
        ProdFrame {
            width: w,
            height: h,
            y,
            u: vec![seed; (w * h / 4) as usize],
            v: vec![seed.wrapping_add(41); (w * h / 4) as usize],
        }
    }
    fn rf(p: &ProdFrame) -> Frame420 {
        Frame420 {
            width: p.width,
            height: p.height,
            y: p.y.clone(),
            u: p.u.clone(),
            v: p.v.clone(),
        }
    }
    fn same(p: &ProdFrame, r: &Frame420) {
        assert_eq!(p.width, r.width);
        assert_eq!(p.height, r.height);
        assert_eq!(p.y, r.y);
        assert_eq!(p.u, r.u);
        assert_eq!(p.v, r.v);
    }

    #[test]
    fn differential_intra_inter_and_multi_reference() {
        for &q in &[1u16, 64, 96, 128, 192] {
            let a = prod_frame(3, 34, 26);
            let e0 = avelune_video_v1::encode(
                10,
                &a,
                &[],
                EncodeOptions {
                    qstep: q,
                    ..Default::default()
                },
            )
            .unwrap();
            let (_, r0, _) = decode(&e0.packet, &[]).unwrap();
            same(&e0.reconstructed, &r0);
            let b = prod_frame(7, 34, 26);
            let e1 = avelune_video_v1::encode(
                11,
                &b,
                &[(10, &e0.reconstructed)],
                EncodeOptions {
                    qstep: q,
                    max_refs: 1,
                    ..Default::default()
                },
            )
            .unwrap();
            let rr0 = rf(&e0.reconstructed);
            let (_, r1, _) = decode(&e1.packet, &[(10, &rr0)]).unwrap();
            same(&e1.reconstructed, &r1);
            let c = prod_frame(90, 34, 26);
            let e2 = avelune_video_v1::encode(
                12,
                &c,
                &[(11, &e1.reconstructed), (10, &e0.reconstructed)],
                EncodeOptions {
                    qstep: q,
                    max_refs: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            let rr1 = rf(&e1.reconstructed);
            let (_, r2, _) = decode(&e2.packet, &[(10, &rr0), (11, &rr1)]).unwrap();
            same(&e2.reconstructed, &r2);
        }
    }

    #[test]
    fn differential_palette() {
        let w = 32;
        let h = 24;
        let mut a = prod_frame(0, w, h);
        for y in 0..h as usize {
            for x in 0..w as usize {
                a.y[y * w as usize + x] = if (x / 4 + y / 4) % 2 == 0 { 17 } else { 231 };
            }
        }
        a.u.fill(128);
        a.v.fill(128);
        let e = avelune_video_v1::encode(
            1,
            &a,
            &[],
            EncodeOptions {
                qstep: 128,
                allow_palette: true,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, r, _) = decode(&e.packet, &[]).unwrap();
        same(&e.reconstructed, &r);
    }
}
