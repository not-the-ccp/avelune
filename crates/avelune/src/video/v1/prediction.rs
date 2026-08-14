use super::*;

pub(super) fn hadamard8(v: &mut [i32; 8]) {
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
pub(super) fn wht2(mut a: [i32; 64]) -> [i32; 64] {
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
pub(super) fn div_round(v: i32, d: i32) -> i32 {
    if v >= 0 {
        (v + d / 2) / d
    } else {
        -((-v + d / 2) / d)
    }
}
pub(super) fn inv_wht2(a: [i32; 64], kernels: crate::kernels::KernelSet) -> [i32; 64] {
    kernels.inverse_wht8x8(a)
}
pub(super) fn clip8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

pub(super) fn plane_dims(frame: &Frame420, p: usize) -> (usize, usize) {
    if p == 0 {
        (frame.width as usize, frame.height as usize)
    } else {
        (frame.width as usize / 2, frame.height as usize / 2)
    }
}
pub(super) fn plane<'a>(frame: &'a Frame420, p: usize) -> &'a [u8] {
    match p {
        0 => frame.y(),
        1 => frame.u(),
        _ => frame.v(),
    }
}

pub(super) fn intra_sample(
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
pub(super) fn intra_prediction_block(
    recon: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    mode: u8,
) -> [u8; 64] {
    let mut out = [128u8; 64];
    let bw = (w - bx).min(BLOCK);
    let bh = (h - by).min(BLOCK);
    match mode {
        1 => {
            if bx > 0 {
                for y in 0..bh {
                    let value = recon[(by + y) * w + bx - 1];
                    out[y * 8..y * 8 + bw].fill(value);
                }
            }
        }
        2 => {
            if by > 0 {
                for y in 0..bh {
                    for x in 0..bw {
                        out[y * 8 + x] = recon[(by - 1) * w + bx + x];
                    }
                }
            }
        }
        _ => {
            let mut sum = 0u32;
            let mut n = 0u32;
            if by > 0 {
                for x in 0..bw {
                    sum += u32::from(recon[(by - 1) * w + bx + x]);
                    n += 1;
                }
            }
            if bx > 0 {
                for y in 0..bh {
                    sum += u32::from(recon[(by + y) * w + bx - 1]);
                    n += 1;
                }
            }
            let value = if n == 0 {
                128
            } else {
                ((sum + n / 2) / n) as u8
            };
            for y in 0..bh {
                out[y * 8..y * 8 + bw].fill(value);
            }
        }
    }
    out
}

pub(super) fn floor_div2(v: i32) -> i32 {
    v.div_euclid(2)
}
pub(super) fn sample_half(src: &[u8], w: usize, h: usize, x2: i32, y2: i32) -> u8 {
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
pub(super) fn integer_motion_origin(
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

pub(super) fn halfpel_interior_origin(
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    dx2: i32,
    dy2: i32,
) -> Option<(usize, usize, u8, u8)> {
    if bx.checked_add(BLOCK)? > w || by.checked_add(BLOCK)? > h {
        return None;
    }
    let x2 = i32::try_from(bx).ok()?.checked_mul(2)?.checked_add(dx2)?;
    let y2 = i32::try_from(by).ok()?.checked_mul(2)?.checked_add(dy2)?;
    let x0 = floor_div2(x2);
    let y0 = floor_div2(y2);
    let fx = x2.rem_euclid(2) as u8;
    let fy = y2.rem_euclid(2) as u8;
    if x0 < 0 || y0 < 0 {
        return None;
    }
    let sx = usize::try_from(x0).ok()?;
    let sy = usize::try_from(y0).ok()?;
    if sx.checked_add(BLOCK + usize::from(fx))? > w || sy.checked_add(BLOCK + usize::from(fy))? > h
    {
        return None;
    }
    Some((sx, sy, fx, fy))
}

pub(super) fn halfpel_prediction_block(
    reference: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    dx2: i32,
    dy2: i32,
    kernels: crate::kernels::KernelSet,
) -> Option<[u8; 64]> {
    let (sx, sy, fx, fy) = halfpel_interior_origin(w, h, bx, by, dx2, dy2)?;
    kernels.halfpel_predict_8x8(&reference[sy * w + sx..], w, fx, fy)
}

pub(super) fn sad_intra(
    src: &[u8],
    recon: &[u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    mode: u8,
) -> u32 {
    let prediction = intra_prediction_block(recon, w, h, bx, by, mode);
    let bw = (w - bx).min(BLOCK);
    let bh = (h - by).min(BLOCK);
    let mut sad = 0u32;
    for y in 0..bh {
        for x in 0..bw {
            sad += (i32::from(src[(by + y) * w + bx + x]) - i32::from(prediction[y * 8 + x]))
                .unsigned_abs();
        }
    }
    sad
}

#[inline(always)]
pub(super) fn sad_inter(
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
    if let Some((sx, sy, fx, fy)) = halfpel_interior_origin(w, h, bx, by, dx2, dy2)
        && let Some(sad) =
            kernels.halfpel_sad_8x8(&src[by * w + bx..], w, &r[sy * w + sx..], w, fx, fy)
    {
        return sad.min(u64::from(u32::MAX)) as u32;
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
pub(super) fn consider_integer_motion(
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
pub(super) fn search_inter_motion(
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

pub(super) fn parallel_planes(
    width: u32,
    height: u32,
    policy: crate::config::ThreadPolicy,
) -> bool {
    u64::from(width) * u64::from(height) >= 256 * 256
        && crate::scheduler::worker_count(policy, 3) >= 2
}
