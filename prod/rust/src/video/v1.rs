//! Production-owned ALV1 Draft Generation 1 codec implementation.
//!
//! Normative parsing and reconstruction stay in safe Rust. Coarse independent plane work is
//! parallelized only above a size threshold; architecture-specific kernels remain isolated in
//! `avelune-prod-kernels`.

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
    /// Allocates a zero-filled tightly packed production frame.
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
/// Non-normative production encoder search policy.
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
fn inv_wht2(a: [i32; 64], kernels: crate::kernels::KernelSet) -> [i32; 64] {
    kernels.inverse_wht8x8(a)
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
        0 => frame.y(),
        1 => frame.u(),
        _ => frame.v(),
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
#[inline(always)]
fn sad_inter(
    src: &[u8],
    r: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    dx2: i32,
    dy2: i32,
    kernels: crate::kernels::KernelSet,
) -> u32 {
    let mut s = 0u32;
    if let Some((sx, sy)) = integer_motion_origin(w, h, bx, by, dx2, dy2) {
        let bw = (w - bx).min(BLOCK);
        let bh = (h - by).min(BLOCK);
        let a = &src[by * w + bx..];
        let b = &r[sy * w + sx..];
        return kernels
            .sad_block(a, w, b, w, bw, bh)
            .min(u64::from(u32::MAX)) as u32;
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

#[inline(always)]
fn consider_integer_motion(
    best: &mut (u32, i32, i32),
    src: &[u8],
    reference: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    radius: i32,
    dx: i32,
    dy: i32,
    kernels: crate::kernels::KernelSet,
) {
    if dx < -radius || dx > radius || dy < -radius || dy > radius {
        return;
    }
    let d = sad_inter(src, reference, w, h, bx, by, dx * 2, dy * 2, kernels);
    if d < best.0 {
        *best = (d, dx, dy);
    }
}

#[inline(always)]
fn search_inter_motion(
    src: &[u8],
    reference: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    radius: i32,
    preset: EncoderPreset,
    kernels: crate::kernels::KernelSet,
) -> (u32, i32, i32) {
    let zero = sad_inter(src, reference, w, h, bx, by, 0, 0, kernels);
    if zero == 0 || radius == 0 {
        return (zero, 0, 0);
    }

    let mut best = (zero, 0i32, 0i32); // SAD, integer dx, integer dy
    match preset {
        EncoderPreset::Quality | EncoderPreset::Balanced => {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let d = sad_inter(src, reference, w, h, bx, by, dx * 2, dy * 2, kernels);
                    if d < best.0 {
                        best = (d, dx, dy);
                    }
                }
            }
        }
        EncoderPreset::Fast => {
            // Fast mode intentionally accepts a local multiscale search in exchange for
            // throughput. Balanced/Quality preserve full configured-radius coverage.
            let mut step = if radius >= 2 { 2 } else { 1 };
            'scale: loop {
                loop {
                    let center = (best.1, best.2);
                    for (ox, oy) in [
                        (-step, 0),
                        (step, 0),
                        (0, -step),
                        (0, step),
                        (-step, -step),
                        (step, -step),
                        (-step, step),
                        (step, step),
                    ] {
                        consider_integer_motion(
                            &mut best,
                            src,
                            reference,
                            w,
                            h,
                            bx,
                            by,
                            radius,
                            center.0 + ox,
                            center.1 + oy,
                            kernels,
                        );
                        if best.0 == 0 {
                            break 'scale;
                        }
                    }
                    if (best.1, best.2) == center {
                        break;
                    }
                }
                if step == 1 {
                    break;
                }
                step = 1;
            }
        }
    }

    let (base, dx, dy) = best;
    let mut sub = (base, dx * 2, dy * 2);
    if base != 0 {
        for oy in -1..=1 {
            for ox in -1..=1 {
                let dx2 = dx * 2 + ox;
                let dy2 = dy * 2 + oy;
                if dx2.unsigned_abs() > 64 || dy2.unsigned_abs() > 64 {
                    continue;
                }
                let d = sad_inter(src, reference, w, h, bx, by, dx2, dy2, kernels);
                if d < sub.0 {
                    sub = (d, dx2, dy2);
                }
            }
        }
    }
    sub
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

fn uvarint_len(mut value: u64) -> usize {
    let mut len = 1usize;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

fn svarint_i32_len(value: i32) -> usize {
    let zigzag = ((value << 1) ^ (value >> 31)) as u32;
    uvarint_len(u64::from(zigzag))
}

fn palette_raw_rate(colors: &[u8], idx: &[u8]) -> usize {
    // one control byte plus data lane: count, colors, sample count, packed 2-bit indices
    1 + 1 + colors.len() + 1 + idx.len().div_ceil(4)
}

#[derive(Clone)]
struct ResidualEval {
    mode: u8,
    ref_idx: usize,
    dx2: i32,
    dy2: i32,
    qcoeff: [i32; 64],
    samples: [u8; 64],
    distortion: u64,
    raw_rate: usize,
}

fn prediction_sample(
    recon: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    x: usize,
    y: usize,
    mode: u8,
    ref_idx: usize,
    dx2: i32,
    dy2: i32,
) -> u8 {
    if mode == 3 {
        sample_half(
            refs[ref_idx],
            w,
            h,
            ((bx + x) as i32) * 2 + dx2,
            ((by + y) as i32) * 2 + dy2,
        )
    } else {
        intra_sample(recon, w, h, bx, by, x, y, mode)
    }
}

fn evaluate_residual_candidate(
    src: &[u8],
    recon: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    mode: u8,
    ref_idx: usize,
    dx2: i32,
    dy2: i32,
    q: i32,
    kernels: crate::kernels::KernelSet,
) -> ResidualEval {
    let mut residual = [0i32; 64];
    let mut prediction = [128u8; 64];
    for y in 0..BLOCK {
        for x in 0..BLOCK {
            let i = y * 8 + x;
            if bx + x >= w || by + y >= h {
                continue;
            }
            let pred = prediction_sample(recon, refs, w, h, bx, by, x, y, mode, ref_idx, dx2, dy2);
            prediction[i] = pred;
            residual[i] = i32::from(src[(by + y) * w + bx + x]) - i32::from(pred);
        }
    }
    let coeff = wht2(residual);
    let mut qcoeff = [0i32; 64];
    for i in 0..64 {
        qcoeff[i] = div_round(coeff[i], q);
    }
    let mut samples = [128u8; 64];
    let mut distortion = 0u64;
    if q == 1 {
        // The integer WHT pair is exactly reversible at q=1. Every residual predictor
        // reconstructs the source block identically, so avoid an unnecessary inverse transform
        // while rate-distortion selection compares syntax cost.
        for y in 0..BLOCK {
            for x in 0..BLOCK {
                if bx + x < w && by + y < h {
                    samples[y * 8 + x] = src[(by + y) * w + bx + x];
                }
            }
        }
    } else {
        let mut deq = [0i32; 64];
        for i in 0..64 {
            deq[i] = qcoeff[i] * q;
        }
        let reconstructed_residual = inv_wht2(deq, kernels);
        for y in 0..BLOCK {
            for x in 0..BLOCK {
                let i = y * 8 + x;
                if bx + x >= w || by + y >= h {
                    continue;
                }
                let reconstructed = clip8(i32::from(prediction[i]) + reconstructed_residual[i]);
                samples[i] = reconstructed;
                let delta = i32::from(src[(by + y) * w + bx + x]) - i32::from(reconstructed);
                distortion += u64::from(delta.unsigned_abs()).pow(2);
            }
        }
    }
    let nz = qcoeff.iter().filter(|&&v| v != 0).count();
    let mut raw_rate = 1 + uvarint_len(nz as u64); // control mode + nz
    if mode == 3 {
        raw_rate += 1 + svarint_i32_len(dx2) + svarint_i32_len(dy2);
    }
    for (i, &v) in qcoeff.iter().enumerate() {
        if v != 0 {
            raw_rate += uvarint_len(i as u64) + svarint_i32_len(v);
        }
    }
    ResidualEval {
        mode,
        ref_idx,
        dx2,
        dy2,
        qcoeff,
        samples,
        distortion,
        raw_rate,
    }
}

fn apply_residual_candidate(
    eval: &ResidualEval,
    recon: &mut [u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
) {
    for y in 0..BLOCK {
        for x in 0..BLOCK {
            if bx + x >= w || by + y >= h {
                continue;
            }
            recon[(by + y) * w + bx + x] = eval.samples[y * 8 + x];
        }
    }
}

fn rdo_lambda(q: i32) -> u64 {
    // Deliberately conservative. Raw syntax bytes are only a proxy for post-rANS rate, so rate
    // nudges decisions rather than overwhelming sample-domain distortion at lossy quantizers.
    ((i64::from(q) * i64::from(q)) / 512).max(1) as u64
}

fn encode_plane(
    src: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    opt: EncodeOptions,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> (Vec<BlockRec>, Vec<bool>) {
    debug_assert_eq!(recon.len(), w * h);
    recon.fill(128);
    let mut blocks = Vec::new();
    let mut used = vec![false; refs.len()];
    let q = i32::from(opt.qstep.max(1));
    let lambda = rdo_lambda(q);
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let palette = if opt.allow_palette {
                palette_for(src, w, h, bx, by)
            } else {
                None
            };

            let mut best_intra = (sad_intra(src, recon, w, h, bx, by, 0), 0u8);
            for mode in [1u8, 2u8] {
                let sad = sad_intra(src, recon, w, h, bx, by, mode);
                if sad < best_intra.0 {
                    best_intra = (sad, mode);
                }
            }

            let radius = i32::from(opt.motion_radius);
            let mut best_inter: Option<(u32, usize, i32, i32)> = None;
            for (ri, reference) in refs.iter().enumerate() {
                let (sad, dx2, dy2) =
                    search_inter_motion(src, reference, w, h, bx, by, radius, opt.preset, kernels);
                if best_inter.is_none_or(|current| sad < current.0) {
                    best_inter = Some((sad, ri, dx2, dy2));
                }
            }

            if matches!(opt.preset, EncoderPreset::Fast) {
                let (mode, ref_idx, dx2, dy2, best_sad) = match best_inter {
                    Some((sad, ri, dx2, dy2)) if sad < best_intra.0 => (3, ri, dx2, dy2, sad),
                    _ => (best_intra.1, 0, 0, 0, best_intra.0),
                };
                if let Some((colors, idx)) = palette {
                    let valid = ((w - bx).min(BLOCK)) * ((h - by).min(BLOCK));
                    if best_sad > (valid as u32) * 4 {
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
                                k += 1;
                            }
                        }
                        blocks.push(BlockRec::Palette { colors, idx });
                        continue;
                    }
                }
                let eval = evaluate_residual_candidate(
                    src, recon, refs, w, h, bx, by, mode, ref_idx, dx2, dy2, q, kernels,
                );
                apply_residual_candidate(&eval, recon, w, h, bx, by);
                if eval.mode == 3 {
                    used[eval.ref_idx] = true;
                }
                blocks.push(BlockRec::Residual {
                    mode: eval.mode,
                    ref_idx: eval.ref_idx,
                    dx2: eval.dx2,
                    dy2: eval.dy2,
                    qcoeff: eval.qcoeff,
                });
                continue;
            }

            let inter_candidate = best_inter;
            let primary_is_inter = inter_candidate.is_some_and(|(sad, _, _, _)| sad < best_intra.0);
            let (primary_mode, primary_ref, primary_dx2, primary_dy2, primary_sad) =
                if let Some((sad, ri, dx2, dy2)) = inter_candidate.filter(|_| primary_is_inter) {
                    (3, ri, dx2, dy2, sad)
                } else {
                    (best_intra.1, 0, 0, 0, best_intra.0)
                };
            let mut chosen = evaluate_residual_candidate(
                src,
                recon,
                refs,
                w,
                h,
                bx,
                by,
                primary_mode,
                primary_ref,
                primary_dx2,
                primary_dy2,
                q,
                kernels,
            );
            let primary_dependency = if chosen.mode == 3 && !used[chosen.ref_idx] {
                8
            } else {
                0
            };
            let mut chosen_score = chosen.distortion.saturating_add(
                lambda.saturating_mul((chosen.raw_rate + primary_dependency) as u64),
            );

            let alternate = if primary_is_inter {
                Some((best_intra.0, best_intra.1, 0usize, 0i32, 0i32))
            } else {
                inter_candidate.map(|(sad, ri, dx2, dy2)| (sad, 3u8, ri, dx2, dy2))
            };
            if let Some((alternate_sad, mode, ri, dx2, dy2)) = alternate {
                let sad_gap = primary_sad.abs_diff(alternate_sad);
                let compare_alternate = q == 1
                    || matches!(opt.preset, EncoderPreset::Quality)
                    || sad_gap <= (q as u32).saturating_mul(2);
                if compare_alternate {
                    let alt = evaluate_residual_candidate(
                        src, recon, refs, w, h, bx, by, mode, ri, dx2, dy2, q, kernels,
                    );
                    let dependency_cost = if alt.mode == 3 && !used[alt.ref_idx] {
                        8
                    } else {
                        0
                    };
                    let score = alt.distortion.saturating_add(
                        lambda.saturating_mul((alt.raw_rate + dependency_cost) as u64),
                    );
                    if score < chosen_score {
                        chosen = alt;
                        chosen_score = score;
                    }
                }
            }

            if let Some((colors, idx)) = palette {
                let palette_score = lambda.saturating_mul(palette_raw_rate(&colors, &idx) as u64);
                if palette_score < chosen_score {
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
                            k += 1;
                        }
                    }
                    blocks.push(BlockRec::Palette { colors, idx });
                    continue;
                }
            }

            apply_residual_candidate(&chosen, recon, w, h, bx, by);
            if chosen.mode == 3 {
                used[chosen.ref_idx] = true;
            }
            blocks.push(BlockRec::Residual {
                mode: chosen.mode,
                ref_idx: chosen.ref_idx,
                dx2: chosen.dx2,
                dy2: chosen.dy2,
                qcoeff: chosen.qcoeff,
            });
        }
    }
    (blocks, used)
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

fn parallel_planes(width: u32, height: u32, policy: crate::config::ThreadPolicy) -> bool {
    u64::from(width) * u64::from(height) >= 256 * 256
        && crate::scheduler::worker_count(policy, 3) >= 2
}

fn encode_one_plane(
    p: usize,
    frame: &Frame420,
    refs: &[(u64, &Frame420)],
    opt: EncodeOptions,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> (Vec<BlockRec>, Vec<bool>) {
    let (w, h) = plane_dims(frame, p);
    let rp: Vec<&[u8]> = refs.iter().map(|(_, r)| plane(r, p)).collect();
    encode_plane(plane(frame, p), &rp, w, h, opt, kernels, recon)
}

/// Encodes one frame using the supplied reconstructed reference pictures.
pub fn encode(
    frame_id: u64,
    frame: &Frame420,
    references: &[(u64, &Frame420)],
    opt: EncodeOptions,
) -> Result<EncodedFrame, VideoError> {
    encode_with_threads(
        frame_id,
        frame,
        references,
        opt,
        crate::config::ThreadPolicy::Auto,
        crate::kernels::KernelSet::auto(),
        None,
        None,
    )
}

fn encode_with_threads(
    frame_id: u64,
    frame: &Frame420,
    references: &[(u64, &Frame420)],
    opt: EncodeOptions,
    thread_policy: crate::config::ThreadPolicy,
    kernels: crate::kernels::KernelSet,
    scheduler: Option<&crate::scheduler::Scheduler>,
    frame_pool: Option<&mut Vec<Frame420>>,
) -> Result<EncodedFrame, VideoError> {
    frame.validate()?;
    if opt.qstep == 0 {
        return Err(VideoError::BadQuantizer);
    }
    if opt.motion_radius > 32 {
        return Err(VideoError::BadHeader);
    }
    let maxrefs = usize::from(opt.max_refs.min(4));
    let refs = &references[..references.len().min(maxrefs)];
    for (i, (id, r)) in refs.iter().enumerate() {
        if *id == frame_id || refs[..i].iter().any(|(seen, _)| seen == id) {
            return Err(VideoError::BadHeader);
        }
        r.validate()?;
        if r.width != frame.width || r.height != frame.height {
            return Err(VideoError::ReferenceShape);
        }
    }

    let mut reconstructed = if let Some(pool) = frame_pool {
        if let Some(i) = pool.iter().position(|candidate| {
            candidate.width == frame.width && candidate.height == frame.height
        }) {
            pool.swap_remove(i)
        } else {
            Frame420::new(frame.width, frame.height)?
        }
    } else {
        Frame420::new(frame.width, frame.height)?
    };
    let mut view = reconstructed.view_mut();
    let y = view.y.contiguous_mut().ok_or(VideoError::PlaneLength)?;
    let u = view.u.contiguous_mut().ok_or(VideoError::PlaneLength)?;
    let v = view.v.contiguous_mut().ok_or(VideoError::PlaneLength)?;
    let (p0, p1, p2) = if parallel_planes(frame.width, frame.height, thread_policy) {
        if let Some(scheduler) = scheduler {
            scheduler.run_three(
                || encode_one_plane(0, frame, refs, opt, kernels, y),
                || encode_one_plane(1, frame, refs, opt, kernels, u),
                || encode_one_plane(2, frame, refs, opt, kernels, v),
            )
        } else {
            std::thread::scope(|scope| {
                let h0 = scope.spawn(|| encode_one_plane(0, frame, refs, opt, kernels, y));
                let h1 = scope.spawn(|| encode_one_plane(1, frame, refs, opt, kernels, u));
                let h2 = scope.spawn(|| encode_one_plane(2, frame, refs, opt, kernels, v));
                let a = h0.join().map_err(|_| VideoError::BadHeader)?;
                let b = h1.join().map_err(|_| VideoError::BadHeader)?;
                let c = h2.join().map_err(|_| VideoError::BadHeader)?;
                Ok::<_, VideoError>((a, b, c))
            })?
        }
    } else {
        (
            encode_one_plane(0, frame, refs, opt, kernels, y),
            encode_one_plane(1, frame, refs, opt, kernels, u),
            encode_one_plane(2, frame, refs, opt, kernels, v),
        )
    };
    let plane_results = [p0, p1, p2];
    let mut all_used = vec![false; refs.len()];
    for (_, used) in &plane_results {
        for (dst, src) in all_used.iter_mut().zip(used) {
            *dst |= *src;
        }
    }
    let used_indices: Vec<usize> = all_used
        .iter()
        .enumerate()
        .filter_map(|(i, &u)| u.then_some(i))
        .collect();
    let mut remap = vec![0usize; refs.len()];
    for (new, &old) in used_indices.iter().enumerate() {
        remap[old] = new;
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
        out.extend(id.to_le_bytes());
    }
    for (blocks, _) in &plane_results {
        let single = entropy_compress(&serialize_blocks_single(blocks, &remap));
        let (control, data) = serialize_blocks_split(blocks, &remap);
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
    Ok(EncodedFrame {
        packet: out,
        reconstructed,
        dependencies,
    })
}

fn decode_plane_single_into(
    tokens: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> Result<(), VideoError> {
    if recon.len() != w.checked_mul(h).ok_or(VideoError::BadDimensions)? {
        return Err(VideoError::PlaneLength);
    }
    recon.fill(128);
    let mut pos = 0usize;
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let mode = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)?;
            pos += 1;
            if mode == 4 {
                let n = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)? as usize;
                pos += 1;
                if n == 0 || n > 4 || tokens.len() < pos + n + 1 {
                    return Err(VideoError::BadPalette);
                }
                let colors = &tokens[pos..pos + n];
                pos += n;
                let count = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)? as usize;
                pos += 1;
                let expected = (w - bx).min(8) * (h - by).min(8);
                if count != expected {
                    return Err(VideoError::BadPalette);
                }
                let bytes = (count + 3) / 4;
                if tokens.len() < pos + bytes {
                    return Err(VideoError::UnexpectedEof);
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
                            return Err(VideoError::BadPalette);
                        }
                        recon[(by + y) * w + bx + x] = colors[ci as usize];
                        k += 1
                    }
                }
                continue;
            }
            if mode > 3 {
                return Err(VideoError::BadMode);
            }
            let (mut ri, mut dx, mut dy) = (0usize, 0i32, 0i32);
            if mode == 3 {
                ri = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)? as usize;
                pos += 1;
                if ri >= refs.len() {
                    return Err(VideoError::BadMode);
                }
                dx = get_svarint_i32(tokens, &mut pos)?;
                dy = get_svarint_i32(tokens, &mut pos)?;
                if dx.unsigned_abs() > 64 || dy.unsigned_abs() > 64 {
                    return Err(VideoError::BadMode);
                }
            }
            let nz = usize::try_from(get_uvarint(tokens, &mut pos)?)
                .map_err(|_| VideoError::BadCoefficient)?;
            if nz > 64 {
                return Err(VideoError::BadCoefficient);
            }
            let mut qc = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = usize::try_from(get_uvarint(tokens, &mut pos)?)
                    .map_err(|_| VideoError::BadCoefficient)?;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(VideoError::BadCoefficient);
                }
                let v = get_svarint_i32(tokens, &mut pos)?;
                if v.unsigned_abs() > 1_000_000 {
                    return Err(VideoError::BadCoefficient);
                }
                qc[i] = v;
                last = Some(i)
            }
            if nz == 0 {
                if mode == 3 {
                    if let Some((sx, sy)) = integer_motion_origin(w, h, bx, by, dx, dy) {
                        let bw = (w - bx).min(8);
                        let bh = (h - by).min(8);
                        for y in 0..bh {
                            let src = &refs[ri][(sy + y) * w + sx..(sy + y) * w + sx + bw];
                            let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + bw];
                            dst.copy_from_slice(src)
                        }
                    } else {
                        for y in 0..8 {
                            for x in 0..8 {
                                if bx + x >= w || by + y >= h {
                                    continue;
                                }
                                recon[(by + y) * w + bx + x] = sample_half(
                                    refs[ri],
                                    w,
                                    h,
                                    ((bx + x) as i32) * 2 + dx,
                                    ((by + y) as i32) * 2 + dy,
                                )
                            }
                        }
                    }
                } else {
                    for y in 0..8 {
                        for x in 0..8 {
                            if bx + x >= w || by + y >= h {
                                continue;
                            }
                            recon[(by + y) * w + bx + x] =
                                intra_sample(recon, w, h, bx, by, x, y, mode)
                        }
                    }
                }
                continue;
            }
            let mut deq = [0i32; 64];
            for i in 0..64 {
                deq[i] = qc[i].checked_mul(q).ok_or(VideoError::BadCoefficient)?;
                if deq[i].unsigned_abs() > 33_554_431 {
                    return Err(VideoError::BadCoefficient);
                }
            }
            let rr = inv_wht2(deq, kernels);
            let fast = if mode == 3 {
                integer_motion_origin(w, h, bx, by, dx, dy)
            } else {
                None
            };
            for y in 0..8 {
                for x in 0..8 {
                    if bx + x >= w || by + y >= h {
                        continue;
                    }
                    let pred = if mode == 3 {
                        if let Some((sx, sy)) = fast {
                            refs[ri][(sy + y) * w + sx + x]
                        } else {
                            sample_half(
                                refs[ri],
                                w,
                                h,
                                ((bx + x) as i32) * 2 + dx,
                                ((by + y) as i32) * 2 + dy,
                            )
                        }
                    } else {
                        intra_sample(recon, w, h, bx, by, x, y, mode)
                    };
                    recon[(by + y) * w + bx + x] = clip8(i32::from(pred) + rr[y * 8 + x]);
                }
            }
        }
    }
    if pos != tokens.len() {
        return Err(VideoError::TrailingData);
    }
    Ok(())
}

#[cfg(test)]
fn decode_plane_single(
    tokens: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
    kernels: crate::kernels::KernelSet,
) -> Result<Vec<u8>, VideoError> {
    let mut recon = vec![0u8; w.checked_mul(h).ok_or(VideoError::BadDimensions)?];
    decode_plane_single_into(tokens, refs, w, h, q, kernels, &mut recon)?;
    Ok(recon)
}

fn decode_plane_into(
    control: &[u8],
    data: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> Result<(), VideoError> {
    if recon.len() != w.checked_mul(h).ok_or(VideoError::BadDimensions)? {
        return Err(VideoError::PlaneLength);
    }
    recon.fill(128);
    let mut cp = 0usize;
    let mut dp = 0usize;
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let mode = *control.get(cp).ok_or(VideoError::UnexpectedEof)?;
            cp += 1;
            if mode == 4 {
                let n = *data.get(dp).ok_or(VideoError::UnexpectedEof)? as usize;
                dp += 1;
                if n == 0 || n > 4 || data.len() < dp + n + 1 {
                    return Err(VideoError::BadPalette);
                }
                let colors = &data[dp..dp + n];
                dp += n;
                let count = *data.get(dp).ok_or(VideoError::UnexpectedEof)? as usize;
                dp += 1;
                let expected = (w - bx).min(8) * (h - by).min(8);
                if count != expected {
                    return Err(VideoError::BadPalette);
                }
                let bytes = (count + 3) / 4;
                if data.len() < dp + bytes {
                    return Err(VideoError::UnexpectedEof);
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
                            return Err(VideoError::BadPalette);
                        }
                        recon[(by + y) * w + bx + x] = colors[ci as usize];
                        k += 1;
                    }
                }
                continue;
            }
            if mode > 3 {
                return Err(VideoError::BadMode);
            }
            let (mut ri, mut dx, mut dy) = (0usize, 0i32, 0i32);
            if mode == 3 {
                ri = *data.get(dp).ok_or(VideoError::UnexpectedEof)? as usize;
                dp += 1;
                if ri >= refs.len() {
                    return Err(VideoError::BadMode);
                }
                dx = get_svarint_i32(data, &mut dp)?;
                dy = get_svarint_i32(data, &mut dp)?;
                if dx.unsigned_abs() > 64 || dy.unsigned_abs() > 64 {
                    return Err(VideoError::BadMode);
                }
            }
            let nz = usize::try_from(get_uvarint(data, &mut dp)?)
                .map_err(|_| VideoError::BadCoefficient)?;
            if nz > 64 {
                return Err(VideoError::BadCoefficient);
            }
            let mut qc = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = usize::try_from(get_uvarint(data, &mut dp)?)
                    .map_err(|_| VideoError::BadCoefficient)?;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(VideoError::BadCoefficient);
                }
                let v = get_svarint_i32(data, &mut dp)?;
                if v.unsigned_abs() > 1_000_000 {
                    return Err(VideoError::BadCoefficient);
                }
                qc[i] = v;
                last = Some(i);
            }
            if nz == 0 {
                if mode == 3 {
                    if let Some((sx, sy)) = integer_motion_origin(w, h, bx, by, dx, dy) {
                        let bw = (w - bx).min(8);
                        let bh = (h - by).min(8);
                        for y in 0..bh {
                            let src = &refs[ri][(sy + y) * w + sx..(sy + y) * w + sx + bw];
                            let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + bw];
                            dst.copy_from_slice(src)
                        }
                    } else {
                        for y in 0..8 {
                            for x in 0..8 {
                                if bx + x >= w || by + y >= h {
                                    continue;
                                }
                                recon[(by + y) * w + bx + x] = sample_half(
                                    refs[ri],
                                    w,
                                    h,
                                    ((bx + x) as i32) * 2 + dx,
                                    ((by + y) as i32) * 2 + dy,
                                )
                            }
                        }
                    }
                } else {
                    for y in 0..8 {
                        for x in 0..8 {
                            if bx + x >= w || by + y >= h {
                                continue;
                            }
                            recon[(by + y) * w + bx + x] =
                                intra_sample(recon, w, h, bx, by, x, y, mode)
                        }
                    }
                }
                continue;
            }
            let mut deq = [0i32; 64];
            for i in 0..64 {
                deq[i] = qc[i].checked_mul(q).ok_or(VideoError::BadCoefficient)?;
                if deq[i].unsigned_abs() > 33_554_431 {
                    return Err(VideoError::BadCoefficient);
                }
            }
            let rr = inv_wht2(deq, kernels);
            let fast = if mode == 3 {
                integer_motion_origin(w, h, bx, by, dx, dy)
            } else {
                None
            };
            for y in 0..8 {
                for x in 0..8 {
                    if bx + x >= w || by + y >= h {
                        continue;
                    }
                    let pred = if mode == 3 {
                        if let Some((sx, sy)) = fast {
                            refs[ri][(sy + y) * w + sx + x]
                        } else {
                            sample_half(
                                refs[ri],
                                w,
                                h,
                                ((bx + x) as i32) * 2 + dx,
                                ((by + y) as i32) * 2 + dy,
                            )
                        }
                    } else {
                        intra_sample(recon, w, h, bx, by, x, y, mode)
                    };
                    recon[(by + y) * w + bx + x] = clip8(i32::from(pred) + rr[y * 8 + x]);
                }
            }
        }
    }
    if cp != control.len() || dp != data.len() {
        return Err(VideoError::TrailingData);
    }
    Ok(())
}

#[derive(Debug, Default)]
struct PlaneEntropyScratch {
    control_or_single: EntropyScratch,
    data: EntropyScratch,
}

#[derive(Clone, Copy)]
enum PlaneEnvelope<'a> {
    Single(&'a [u8]),
    Split(&'a [u8], &'a [u8]),
}

fn parse_plane_envelopes<'a>(
    input: &'a [u8],
    pos: &mut usize,
) -> Result<[PlaneEnvelope<'a>; 3], VideoError> {
    let mut planes = Vec::with_capacity(3);
    for _ in 0..3 {
        let layout = *input.get(*pos).ok_or(VideoError::UnexpectedEof)?;
        *pos += 1;
        if input.len() < *pos + 4 {
            return Err(VideoError::UnexpectedEof);
        }
        let first = u32::from_le_bytes(input[*pos..*pos + 4].try_into().unwrap()) as usize;
        *pos += 4;
        let first_end = pos.checked_add(first).ok_or(VideoError::OutputTooLarge)?;
        if first_end > input.len() {
            return Err(VideoError::UnexpectedEof);
        }
        let first_bytes = &input[*pos..first_end];
        *pos = first_end;
        match layout {
            0 => planes.push(PlaneEnvelope::Single(first_bytes)),
            1 => {
                if input.len() < *pos + 4 {
                    return Err(VideoError::UnexpectedEof);
                }
                let second = u32::from_le_bytes(input[*pos..*pos + 4].try_into().unwrap()) as usize;
                *pos += 4;
                let second_end = pos.checked_add(second).ok_or(VideoError::OutputTooLarge)?;
                if second_end > input.len() {
                    return Err(VideoError::UnexpectedEof);
                }
                let second_bytes = &input[*pos..second_end];
                *pos = second_end;
                planes.push(PlaneEnvelope::Split(first_bytes, second_bytes));
            }
            _ => return Err(VideoError::BadHeader),
        }
    }
    planes.try_into().map_err(|_| VideoError::BadHeader)
}

fn decode_one_plane_into(
    p: usize,
    envelope: PlaneEnvelope<'_>,
    refs: &[&Frame420],
    w: u32,
    h: u32,
    q: u16,
    scratch: &mut PlaneEntropyScratch,
    kernels: crate::kernels::KernelSet,
    max_entropy_bytes: usize,
    recon: &mut [u8],
) -> Result<(), VideoError> {
    let (pw, ph) = if p == 0 {
        (w as usize, h as usize)
    } else {
        (w as usize / 2, h as usize / 2)
    };
    let rplanes: Vec<&[u8]> = refs.iter().map(|r| plane(r, p)).collect();
    match envelope {
        PlaneEnvelope::Single(bytes) => {
            let tokens = entropy_decompress_with_scratch(
                bytes,
                max_entropy_bytes,
                &mut scratch.control_or_single,
            )?;
            decode_plane_single_into(tokens, &rplanes, pw, ph, i32::from(q), kernels, recon)
        }
        PlaneEnvelope::Split(control_bytes, data_bytes) => {
            let control = entropy_decompress_with_scratch(
                control_bytes,
                max_entropy_bytes,
                &mut scratch.control_or_single,
            )?;
            let data =
                entropy_decompress_with_scratch(data_bytes, max_entropy_bytes, &mut scratch.data)?;
            decode_plane_into(
                control,
                data,
                &rplanes,
                pw,
                ph,
                i32::from(q),
                kernels,
                recon,
            )
        }
    }
}

/// Decodes one ALV1 frame packet using the supplied immutable reference pictures.
pub fn decode(
    input: &[u8],
    references: &[(u64, &Frame420)],
) -> Result<(u64, Frame420, Vec<u64>), VideoError> {
    let mut scratch: [PlaneEntropyScratch; 3] =
        std::array::from_fn(|_| PlaneEntropyScratch::default());
    decode_with_threads(
        input,
        references,
        crate::config::ThreadPolicy::Auto,
        &mut scratch,
        crate::kernels::KernelSet::auto(),
        None,
        None,
        crate::limits::Limits::default().max_frame_pixels,
        crate::limits::Limits::default().max_entropy_bytes,
    )
}

fn decode_with_threads(
    input: &[u8],
    references: &[(u64, &Frame420)],
    thread_policy: crate::config::ThreadPolicy,
    scratch: &mut [PlaneEntropyScratch; 3],
    kernels: crate::kernels::KernelSet,
    scheduler: Option<&crate::scheduler::Scheduler>,
    frame_pool: Option<&mut Vec<Frame420>>,
    max_frame_pixels: u64,
    max_entropy_bytes: usize,
) -> Result<(u64, Frame420, Vec<u64>), VideoError> {
    if input.len() < 20 {
        return Err(VideoError::UnexpectedEof);
    }
    if input[..4] != CODEC_MAGIC {
        return Err(VideoError::BadHeader);
    }
    let frame_id = u64::from_le_bytes(input[4..12].try_into().unwrap());
    let w = u16::from_le_bytes(input[12..14].try_into().unwrap()) as u32;
    let h = u16::from_le_bytes(input[14..16].try_into().unwrap()) as u32;
    let q = u16::from_le_bytes(input[16..18].try_into().unwrap());
    if q == 0 {
        return Err(VideoError::BadQuantizer);
    }
    sizes(w, h)?;
    if u64::from(w) * u64::from(h) > max_frame_pixels {
        return Err(VideoError::OutputTooLarge);
    }
    let rc = input[18] as usize;
    let flags = input[19];
    if rc > 4 || flags != 1 {
        return Err(VideoError::BadHeader);
    }
    let mut pos = 20usize;
    let mut dep = Vec::with_capacity(rc);
    let mut refs = Vec::with_capacity(rc);
    for _ in 0..rc {
        if input.len() < pos + 8 {
            return Err(VideoError::UnexpectedEof);
        }
        let id = u64::from_le_bytes(input[pos..pos + 8].try_into().unwrap());
        pos += 8;
        if dep.contains(&id) || id == frame_id {
            return Err(VideoError::BadHeader);
        }
        let r = references
            .iter()
            .find(|(rid, _)| *rid == id)
            .ok_or(VideoError::ReferenceMissing(id))?
            .1;
        if r.width != w || r.height != h {
            return Err(VideoError::ReferenceShape);
        }
        dep.push(id);
        refs.push(r);
    }

    // Validate and discover all plane envelopes before entropy allocation/reconstruction. This
    // makes attacker-controlled lengths cheap to reject and exposes three independent jobs.
    let envelopes = parse_plane_envelopes(input, &mut pos)?;
    if pos != input.len() {
        return Err(VideoError::TrailingData);
    }

    let [scratch0, scratch1, scratch2] = scratch;
    let mut frame = if let Some(pool) = frame_pool {
        if let Some(index) = pool.iter().position(|f| f.width == w && f.height == h) {
            pool.swap_remove(index)
        } else {
            Frame420::new(w, h)?
        }
    } else {
        Frame420::new(w, h)?
    };
    {
        let crate::buffer::Frame420ViewMut {
            mut y,
            mut u,
            mut v,
        } = frame.view_mut();
        let y = y.contiguous_mut().ok_or(VideoError::PlaneLength)?;
        let u = u.contiguous_mut().ok_or(VideoError::PlaneLength)?;
        let v = v.contiguous_mut().ok_or(VideoError::PlaneLength)?;
        if parallel_planes(w, h, thread_policy) {
            if let Some(scheduler) = scheduler {
                let (yr, ur, vr) = scheduler.run_three(
                    || {
                        decode_one_plane_into(
                            0,
                            envelopes[0],
                            &refs,
                            w,
                            h,
                            q,
                            scratch0,
                            kernels,
                            max_entropy_bytes,
                            y,
                        )
                    },
                    || {
                        decode_one_plane_into(
                            1,
                            envelopes[1],
                            &refs,
                            w,
                            h,
                            q,
                            scratch1,
                            kernels,
                            max_entropy_bytes,
                            u,
                        )
                    },
                    || {
                        decode_one_plane_into(
                            2,
                            envelopes[2],
                            &refs,
                            w,
                            h,
                            q,
                            scratch2,
                            kernels,
                            max_entropy_bytes,
                            v,
                        )
                    },
                );
                yr?;
                ur?;
                vr?;
            } else {
                std::thread::scope(|scope| {
                    let h0 = scope.spawn(|| {
                        decode_one_plane_into(
                            0,
                            envelopes[0],
                            &refs,
                            w,
                            h,
                            q,
                            scratch0,
                            kernels,
                            max_entropy_bytes,
                            y,
                        )
                    });
                    let h1 = scope.spawn(|| {
                        decode_one_plane_into(
                            1,
                            envelopes[1],
                            &refs,
                            w,
                            h,
                            q,
                            scratch1,
                            kernels,
                            max_entropy_bytes,
                            u,
                        )
                    });
                    let h2 = scope.spawn(|| {
                        decode_one_plane_into(
                            2,
                            envelopes[2],
                            &refs,
                            w,
                            h,
                            q,
                            scratch2,
                            kernels,
                            max_entropy_bytes,
                            v,
                        )
                    });
                    h0.join().map_err(|_| VideoError::BadHeader)??;
                    h1.join().map_err(|_| VideoError::BadHeader)??;
                    h2.join().map_err(|_| VideoError::BadHeader)??;
                    Ok::<(), VideoError>(())
                })?;
            }
        } else {
            decode_one_plane_into(
                0,
                envelopes[0],
                &refs,
                w,
                h,
                q,
                scratch0,
                kernels,
                max_entropy_bytes,
                y,
            )?;
            decode_one_plane_into(
                1,
                envelopes[1],
                &refs,
                w,
                h,
                q,
                scratch1,
                kernels,
                max_entropy_bytes,
                u,
            )?;
            decode_one_plane_into(
                2,
                envelopes[2],
                &refs,
                w,
                h,
                q,
                scratch2,
                kernels,
                max_entropy_bytes,
                v,
            )?;
        }
    }
    Ok((frame_id, frame, dep))
}

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

/// Stateful production decoder retaining at most four immutable reconstructed references.
#[derive(Debug)]
pub struct VideoDecoder {
    references: std::collections::VecDeque<(u64, std::sync::Arc<Frame420>)>,
    threads: crate::config::ThreadPolicy,
    kernels: crate::kernels::KernelSet,
    scheduler: crate::scheduler::Scheduler,
    max_frame_pixels: u64,
    max_entropy_bytes: usize,
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
            entropy: std::array::from_fn(|_| PlaneEntropyScratch::default()),
            frame_pool: Vec::with_capacity(4),
        }
    }
    /// Creates a decoder with an explicit production configuration.
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
            entropy: std::array::from_fn(|_| PlaneEntropyScratch::default()),
            frame_pool: Vec::with_capacity(4),
        })
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

/// Stateful production encoder retaining reconstructed reference pictures across calls.
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
