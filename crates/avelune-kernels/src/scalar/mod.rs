//! Scalar semantic kernels.
/// Sum of absolute byte differences over the common prefix of both slices.
///
/// If the inputs differ in length, samples beyond the shorter slice are ignored.
#[inline]
pub fn sad(a: &[u8], b: &[u8]) -> u64 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| u64::from(x.abs_diff(y)))
        .sum()
}

/// SAD over a small strided rectangular block.
pub fn sad_block(
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    fn footprint_valid(len: usize, stride: usize, width: usize, height: usize) -> bool {
        if height == 0 {
            return true;
        }
        if stride < width {
            return false;
        }
        let Some(last_row) = (height - 1).checked_mul(stride) else {
            return false;
        };
        let Some(need) = last_row.checked_add(width) else {
            return false;
        };
        need <= len
    }

    if !footprint_valid(a.len(), a_stride, width, height)
        || !footprint_valid(b.len(), b_stride, width, height)
    {
        return 0;
    }

    let mut sum = 0_u64;
    for y in 0..height {
        let a_start = y * a_stride;
        let b_start = y * b_stride;
        sum += sad(&a[a_start..a_start + width], &b[b_start..b_start + width]);
    }
    sum
}

/// Validates the exact footprint required by one full 8x8 half-sample prediction.
pub(super) fn halfpel_footprint_valid(src_len: usize, stride: usize, fx: u8, fy: u8) -> bool {
    if fx > 1 || fy > 1 || stride < 8 + usize::from(fx) {
        return false;
    }
    let Some(rows) = 8usize.checked_add(usize::from(fy)) else {
        return false;
    };
    let Some(need) = (rows - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(8 + usize::from(fx)))
    else {
        return false;
    };
    need <= src_len
}

/// Predicts one full 8x8 half-sample block from an interior reference footprint.
/// `fx`/`fy` are fractional phases in {0,1}.
pub fn halfpel_predict_8x8(src: &[u8], stride: usize, fx: u8, fy: u8) -> Option<[u8; 64]> {
    if !halfpel_footprint_valid(src.len(), stride, fx, fy) {
        return None;
    }
    let mut out = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            let a = u16::from(src[y * stride + x]);
            let b = u16::from(src[y * stride + x + usize::from(fx)]);
            let c = u16::from(src[(y + usize::from(fy)) * stride + x]);
            let d = u16::from(src[(y + usize::from(fy)) * stride + x + usize::from(fx)]);
            out[y * 8 + x] = match (fx, fy) {
                (0, 0) => a as u8,
                (1, 0) => ((a + b + 1) >> 1) as u8,
                (0, 1) => ((a + c + 1) >> 1) as u8,
                (1, 1) => ((a + b + c + d + 2) >> 2) as u8,
                _ => unreachable!(),
            };
        }
    }
    Some(out)
}

/// SAD between a strided 8x8 source block and one predicted half-sample reference block.
pub fn halfpel_sad_8x8(
    a: &[u8],
    a_stride: usize,
    reference: &[u8],
    ref_stride: usize,
    fx: u8,
    fy: u8,
) -> Option<u64> {
    let a_need = 7usize.checked_mul(a_stride)?.checked_add(8)?;
    if a_stride < 8 || a_need > a.len() {
        return None;
    }
    let p = halfpel_predict_8x8(reference, ref_stride, fx, fy)?;
    let mut sum = 0u64;
    for y in 0..8 {
        sum += sad(&a[y * a_stride..y * a_stride + 8], &p[y * 8..y * 8 + 8]);
    }
    Some(sum)
}

/// Portable CRC-32C (Castagnoli).
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

/// Inverse 8x8 Walsh-Hadamard transform with Avelune V1 rounding semantics.
pub fn inverse_wht8x8(mut a: [i32; 64]) -> [i32; 64] {
    for y in 0..8 {
        let mut row = [0_i32; 8];
        row.copy_from_slice(&a[y * 8..y * 8 + 8]);
        hadamard8(&mut row);
        a[y * 8..y * 8 + 8].copy_from_slice(&row);
    }
    for x in 0..8 {
        let mut col = [0_i32; 8];
        for y in 0..8 {
            col[y] = a[y * 8 + x];
        }
        hadamard8(&mut col);
        for y in 0..8 {
            a[y * 8 + x] = div_round_64(col[y]);
        }
    }
    a
}

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

fn div_round_64(v: i32) -> i32 {
    if v >= 0 {
        (v + 32) / 64
    } else {
        -((-v + 32) / 64)
    }
}
