//! Reference encoder/decoder for the experimental `ALV1` video bitstream.
//!
//! This crate intentionally favors readable, specification-shaped code over optimized
//! throughput. Optimized backends belong outside this reference implementation.

// The reference implementation intentionally mirrors spec-shaped loops and argument lists.
use crate::bitstream::{BitstreamError, entropy_compress, put_svarint_i32, put_uvarint};

/// Four-byte packet magic for the current draft video generation.
pub const CODEC_MAGIC: [u8; 4] = *b"ALV1";
/// Maximum luma-pixel count accepted by the reference implementation.
pub const MAX_PIXELS: u64 = 8192 * 8192;
/// Transform/prediction block edge length in samples.
pub const BLOCK: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Owned 8-bit planar 4:2:0 frame used by the Draft Generation 1 reference API.
pub struct Frame420 {
    /// Luma width in pixels. Must be non-zero and even.
    pub width: u32,
    /// Luma height in pixels. Must be non-zero and even.
    pub height: u32,
    /// Full-resolution luma plane.
    pub y: Vec<u8>,
    /// Half-width, half-height Cb/U plane.
    pub u: Vec<u8>,
    /// Half-width, half-height Cr/V plane.
    pub v: Vec<u8>,
}
impl Frame420 {
    /// Validates dimensions and plane lengths.
    pub fn validate(&self) -> Result<(), VideoError> {
        let (y, c) = sizes(self.width, self.height)?;
        if self.y.len() != y || self.u.len() != c || self.v.len() != c {
            return Err(VideoError::PlaneLength);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy)]
/// Encoder search options. These are non-normative policy knobs.
pub struct EncodeOptions {
    /// Quantizer step. `1` selects mathematically lossless residual coding.
    pub qstep: u16,
    /// Integer-pixel motion-search radius before half-sample refinement.
    pub motion_radius: u8,
    /// Maximum reference frames considered by the reference encoder.
    pub max_refs: u8,
    /// Whether exact small-palette blocks may be considered.
    pub allow_palette: bool,
}
impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            qstep: 96,
            motion_radius: 4,
            max_refs: 1,
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
#[derive(Debug, Clone, PartialEq, Eq)]
/// Errors from ALV1 frame validation, encoding, or decoding.
pub enum VideoError {
    BadDimensions,
    PlaneLength,
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
    OutputTooLarge,
}
impl From<BitstreamError> for VideoError {
    fn from(e: BitstreamError) -> Self {
        Self::Bitstream(e)
    }
}
fn sizes(w: u32, h: u32) -> Result<(usize, usize), VideoError> {
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(VideoError::BadDimensions);
    }
    let y = (w as usize)
        .checked_mul(h as usize)
        .ok_or(VideoError::BadDimensions)?;
    Ok((y, y / 4))
}

#[derive(Clone, Debug)]
enum BlockRec {
    Residual {
        mode: u8,
        ref_idx: usize,
        dx2: i32,
        dy2: i32,
        qcoeff: [i32; 64],
    },
    Palette {
        colors: Vec<u8>,
        idx: Vec<u8>,
    },
}

fn hadamard8(v: &mut [i32; 8]) {
    let mut h = 1;
    while h < 8 {
        for i in (0..8).step_by(h * 2) {
            for j in i..i + h {
                let a = v[j];
                let b = v[j + h];
                v[j] = a + b;
                v[j + h] = a - b;
            }
        }
        h *= 2;
    }
}
fn wht2(mut a: [i32; 64]) -> [i32; 64] {
    for y in 0..8 {
        let mut r = [0i32; 8];
        r.copy_from_slice(&a[y * 8..y * 8 + 8]);
        hadamard8(&mut r);
        a[y * 8..y * 8 + 8].copy_from_slice(&r);
    }
    for x in 0..8 {
        let mut c = [0i32; 8];
        for y in 0..8 {
            c[y] = a[y * 8 + x]
        }
        hadamard8(&mut c);
        for y in 0..8 {
            a[y * 8 + x] = c[y]
        }
    }
    a
}
fn div_round(v: i32, d: i32) -> i32 {
    if v >= 0 {
        (v + d / 2) / d
    } else {
        -((-v + d / 2) / d)
    }
}
fn inv_wht2(a: [i32; 64]) -> [i32; 64] {
    let b = wht2(a);
    let mut o = [0i32; 64];
    for i in 0..64 {
        o[i] = div_round(b[i], 64)
    }
    o
}
fn clip8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn plane_dims(frame: &Frame420, p: usize) -> (usize, usize) {
    if p == 0 {
        (frame.width as usize, frame.height as usize)
    } else {
        (frame.width as usize / 2, frame.height as usize / 2)
    }
}
fn plane<'a>(frame: &'a Frame420, p: usize) -> &'a [u8] {
    match p {
        0 => &frame.y,
        1 => &frame.u,
        _ => &frame.v,
    }
}

fn intra_sample(
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
                        n += 1
                    }
                }
            }
            if bx > 0 {
                for yy in 0..BLOCK {
                    if by + yy < h {
                        sum += u32::from(recon[(by + yy) * w + bx - 1]);
                        n += 1
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
fn floor_div2(v: i32) -> i32 {
    v.div_euclid(2)
}
fn sample_half(src: &[u8], w: usize, h: usize, x2: i32, y2: i32) -> u8 {
    let x0 = floor_div2(x2);
    let y0 = floor_div2(y2);
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
fn integer_motion_origin(
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    dx2: i32,
    dy2: i32,
) -> Option<(usize, usize)> {
    if dx2 & 1 != 0 || dy2 & 1 != 0 {
        return None;
    }
    let sx = bx as i32 + dx2 / 2;
    let sy = by as i32 + dy2 / 2;
    let bw = (w - bx).min(BLOCK) as i32;
    let bh = (h - by).min(BLOCK) as i32;
    if sx < 0 || sy < 0 || sx + bw > w as i32 || sy + bh > h as i32 {
        None
    } else {
        Some((sx as usize, sy as usize))
    }
}

fn sad_intra(src: &[u8], recon: &[u8], w: usize, h: usize, bx: usize, by: usize, mode: u8) -> u32 {
    let mut s = 0;
    for y in 0..BLOCK {
        if by + y >= h {
            break;
        }
        for x in 0..BLOCK {
            if bx + x >= w {
                break;
            }
            let p = intra_sample(recon, w, h, bx, by, x, y, mode);
            s += (i32::from(src[(by + y) * w + bx + x]) - i32::from(p)).unsigned_abs();
        }
    }
    s
}
fn sad_inter(
    src: &[u8],
    r: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    dx2: i32,
    dy2: i32,
) -> u32 {
    let mut s = 0u32;
    if let Some((sx, sy)) = integer_motion_origin(w, h, bx, by, dx2, dy2) {
        let bw = (w - bx).min(BLOCK);
        let bh = (h - by).min(BLOCK);
        for y in 0..bh {
            let a = &src[(by + y) * w + bx..(by + y) * w + bx + bw];
            let b = &r[(sy + y) * w + sx..(sy + y) * w + sx + bw];
            for i in 0..bw {
                s += (i32::from(a[i]) - i32::from(b[i])).unsigned_abs();
            }
        }
        return s;
    }
    for y in 0..BLOCK {
        if by + y >= h {
            break;
        }
        for x in 0..BLOCK {
            if bx + x >= w {
                break;
            }
            let p = sample_half(
                r,
                w,
                h,
                ((bx + x) as i32) * 2 + dx2,
                ((by + y) as i32) * 2 + dy2,
            );
            s += (i32::from(src[(by + y) * w + bx + x]) - i32::from(p)).unsigned_abs();
        }
    }
    s
}

fn palette_for(src: &[u8], w: usize, h: usize, bx: usize, by: usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut colors = Vec::new();
    let mut idx = Vec::new();
    for y in 0..BLOCK {
        if by + y >= h {
            break;
        }
        for x in 0..BLOCK {
            if bx + x >= w {
                break;
            }
            let v = src[(by + y) * w + bx + x];
            let k = match colors.iter().position(|&c| c == v) {
                Some(k) => k,
                None => {
                    if colors.len() == 4 {
                        return None;
                    }
                    colors.push(v);
                    colors.len() - 1
                }
            };
            idx.push(k as u8);
        }
    }
    if idx.len() >= 16 {
        Some((colors, idx))
    } else {
        None
    }
}

fn encode_plane(
    src: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    opt: EncodeOptions,
) -> (Vec<BlockRec>, Vec<u8>, Vec<bool>) {
    let mut recon = vec![128u8; w * h];
    let mut blocks = Vec::new();
    let mut used = vec![false; refs.len()];
    let q = i32::from(opt.qstep.max(1));
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let palette = if opt.allow_palette {
                palette_for(src, w, h, bx, by)
            } else {
                None
            };
            let mut best_mode = 0u8;
            let mut best_ref = 0usize;
            let mut best_dx = 0i32;
            let mut best_dy = 0i32;
            let mut best = sad_intra(src, &recon, w, h, bx, by, 0);
            for m in [1u8, 2u8] {
                let d = sad_intra(src, &recon, w, h, bx, by, m);
                if d < best {
                    best = d;
                    best_mode = m
                }
            }
            let rpx = i32::from(opt.motion_radius);
            for (ri, r) in refs.iter().enumerate() {
                let zero = sad_inter(src, r, w, h, bx, by, 0, 0);
                let mut local = (zero, 0, 0);
                // Exact reuse is common in UI/animation.  Once SAD is zero no other
                // motion candidate can improve distortion, so avoid the full search.
                if zero != 0 {
                    for dy in -rpx..=rpx {
                        for dx in -rpx..=rpx {
                            let d = sad_inter(src, r, w, h, bx, by, dx * 2, dy * 2);
                            if d < local.0 {
                                local = (d, dx * 2, dy * 2)
                            }
                        }
                    }
                }
                let (base, dx0, dy0) = local;
                let mut sub = (base, dx0, dy0);
                if base != 0 {
                    for oy in -1..=1 {
                        for ox in -1..=1 {
                            let dx = dx0 + ox;
                            let dy = dy0 + oy;
                            let d = sad_inter(src, r, w, h, bx, by, dx, dy);
                            if d < sub.0 {
                                sub = (d, dx, dy)
                            }
                        }
                    }
                }
                if sub.0 < best {
                    best = sub.0;
                    best_mode = 3;
                    best_ref = ri;
                    best_dx = sub.1;
                    best_dy = sub.2
                }
            }
            if let Some((colors, idx)) = palette {
                // Palette is an exact screen-content tool, not a privileged mode.  A block
                // that temporal/spatial prediction already explains well should not resend
                // a literal palette every frame.
                let valid = ((w - bx).min(BLOCK)) * ((h - by).min(BLOCK));
                if best > (valid as u32) * 4 {
                    let mut k = 0;
                    for y in 0..BLOCK {
                        if by + y >= h {
                            break;
                        }
                        for x in 0..BLOCK {
                            if bx + x >= w {
                                break;
                            }
                            recon[(by + y) * w + bx + x] = colors[idx[k] as usize];
                            k += 1
                        }
                    }
                    blocks.push(BlockRec::Palette { colors, idx });
                    continue;
                }
            }
            let mut residual = [0i32; 64];
            for y in 0..BLOCK {
                for x in 0..BLOCK {
                    let i = y * 8 + x;
                    if bx + x >= w || by + y >= h {
                        residual[i] = 0;
                        continue;
                    }
                    let pred = if best_mode == 3 {
                        sample_half(
                            refs[best_ref],
                            w,
                            h,
                            ((bx + x) as i32) * 2 + best_dx,
                            ((by + y) as i32) * 2 + best_dy,
                        )
                    } else {
                        intra_sample(&recon, w, h, bx, by, x, y, best_mode)
                    };
                    residual[i] = i32::from(src[(by + y) * w + bx + x]) - i32::from(pred);
                }
            }
            let coeff = wht2(residual);
            let mut qc = [0i32; 64];
            let mut deq = [0i32; 64];
            for i in 0..64 {
                qc[i] = div_round(coeff[i], q);
                deq[i] = qc[i] * q
            }
            let rr = inv_wht2(deq);
            for y in 0..BLOCK {
                for x in 0..BLOCK {
                    if bx + x >= w || by + y >= h {
                        continue;
                    }
                    let pred = if best_mode == 3 {
                        sample_half(
                            refs[best_ref],
                            w,
                            h,
                            ((bx + x) as i32) * 2 + best_dx,
                            ((by + y) as i32) * 2 + best_dy,
                        )
                    } else {
                        intra_sample(&recon, w, h, bx, by, x, y, best_mode)
                    };
                    recon[(by + y) * w + bx + x] = clip8(i32::from(pred) + rr[y * 8 + x]);
                }
            }
            if best_mode == 3 {
                used[best_ref] = true
            }
            blocks.push(BlockRec::Residual {
                mode: best_mode,
                ref_idx: best_ref,
                dx2: best_dx,
                dy2: best_dy,
                qcoeff: qc,
            });
        }
    }
    (blocks, recon, used)
}

fn serialize_blocks_single(blocks: &[BlockRec], remap: &[usize]) -> Vec<u8> {
    let mut t = Vec::new();
    for b in blocks {
        match b {
            BlockRec::Palette { colors, idx } => {
                t.push(4);
                t.push(colors.len() as u8);
                t.extend(colors);
                t.push(idx.len() as u8);
                let mut p = 0u8;
                let mut shift = 0;
                for &v in idx {
                    p |= (v & 3) << shift;
                    shift += 2;
                    if shift == 8 {
                        t.push(p);
                        p = 0;
                        shift = 0
                    }
                }
                if shift != 0 {
                    t.push(p)
                }
            }
            BlockRec::Residual {
                mode,
                ref_idx,
                dx2,
                dy2,
                qcoeff,
            } => {
                t.push(*mode);
                if *mode == 3 {
                    t.push(remap[*ref_idx] as u8);
                    put_svarint_i32(*dx2, &mut t);
                    put_svarint_i32(*dy2, &mut t)
                }
                let nz = qcoeff.iter().filter(|&&v| v != 0).count();
                put_uvarint(nz as u64, &mut t);
                for (i, &v) in qcoeff.iter().enumerate() {
                    if v != 0 {
                        put_uvarint(i as u64, &mut t);
                        put_svarint_i32(v, &mut t)
                    }
                }
            }
        }
    }
    t
}

fn serialize_blocks_split(blocks: &[BlockRec], remap: &[usize]) -> (Vec<u8>, Vec<u8>) {
    let mut control = Vec::with_capacity(blocks.len());
    let mut data = Vec::new();
    for b in blocks {
        match b {
            BlockRec::Palette { colors, idx } => {
                control.push(4);
                data.push(colors.len() as u8);
                data.extend(colors);
                data.push(idx.len() as u8);
                let mut packed = 0u8;
                let mut shift = 0;
                for &v in idx {
                    packed |= (v & 3) << shift;
                    shift += 2;
                    if shift == 8 {
                        data.push(packed);
                        packed = 0;
                        shift = 0;
                    }
                }
                if shift != 0 {
                    data.push(packed);
                }
            }
            BlockRec::Residual {
                mode,
                ref_idx,
                dx2,
                dy2,
                qcoeff,
            } => {
                control.push(*mode);
                if *mode == 3 {
                    data.push(remap[*ref_idx] as u8);
                    put_svarint_i32(*dx2, &mut data);
                    put_svarint_i32(*dy2, &mut data);
                }
                let nz = qcoeff.iter().filter(|&&v| v != 0).count();
                put_uvarint(nz as u64, &mut data);
                for (i, &v) in qcoeff.iter().enumerate() {
                    if v != 0 {
                        put_uvarint(i as u64, &mut data);
                        put_svarint_i32(v, &mut data);
                    }
                }
            }
        }
    }
    (control, data)
}

/// Encodes one frame using the supplied reconstructed reference pictures.
pub fn encode(
    frame_id: u64,
    frame: &Frame420,
    references: &[(u64, &Frame420)],
    opt: EncodeOptions,
) -> Result<EncodedFrame, VideoError> {
    frame.validate()?;
    if opt.qstep == 0 {
        return Err(VideoError::BadQuantizer);
    }
    let maxrefs = usize::from(opt.max_refs.min(4));
    let refs = &references[..references.len().min(maxrefs)];
    for (_, r) in refs {
        r.validate()?;
        if r.width != frame.width || r.height != frame.height {
            return Err(VideoError::ReferenceShape);
        }
    }
    let mut rec_planes = Vec::new();
    let mut plane_blocks = Vec::new();
    let mut all_used = vec![false; refs.len()];
    for p in 0..3 {
        let (w, h) = plane_dims(frame, p);
        let rp: Vec<&[u8]> = refs.iter().map(|(_, r)| plane(r, p)).collect();
        let (b, recon, used) = encode_plane(plane(frame, p), &rp, w, h, opt);
        for i in 0..used.len() {
            all_used[i] |= used[i]
        }
        plane_blocks.push(b);
        rec_planes.push(recon);
    }
    let used_indices: Vec<usize> = all_used
        .iter()
        .enumerate()
        .filter_map(|(i, &u)| u.then_some(i))
        .collect();
    let mut remap = vec![0usize; refs.len()];
    for (new, &old) in used_indices.iter().enumerate() {
        remap[old] = new
    }
    let dependencies: Vec<u64> = used_indices.iter().map(|&i| refs[i].0).collect();
    let mut out = Vec::new();
    out.extend(CODEC_MAGIC);
    out.extend(frame_id.to_le_bytes());
    out.extend((frame.width as u16).to_le_bytes());
    out.extend((frame.height as u16).to_le_bytes());
    out.extend(opt.qstep.to_le_bytes());
    out.push(dependencies.len() as u8);
    out.push(1); // bit 0: separate control/data entropy lanes
    for &id in &dependencies {
        out.extend(id.to_le_bytes())
    }
    for b in &plane_blocks {
        let single = entropy_compress(&serialize_blocks_single(b, &remap));
        let (control, data) = serialize_blocks_split(b, &remap);
        let cc = entropy_compress(&control);
        let dc = entropy_compress(&data);
        if 4 + single.len() <= 8 + cc.len() + dc.len() {
            out.push(0);
            out.extend((single.len() as u32).to_le_bytes());
            out.extend(single);
        } else {
            out.push(1);
            out.extend((cc.len() as u32).to_le_bytes());
            out.extend(cc);
            out.extend((dc.len() as u32).to_le_bytes());
            out.extend(dc);
        }
    }
    let reconstructed = Frame420 {
        width: frame.width,
        height: frame.height,
        y: rec_planes.remove(0),
        u: rec_planes.remove(0),
        v: rec_planes.remove(0),
    };
    Ok(EncodedFrame {
        packet: out,
        reconstructed,
        dependencies,
    })
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for VideoError {}
