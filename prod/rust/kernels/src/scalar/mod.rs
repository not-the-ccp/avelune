//! Scalar semantic kernels.
/// Sum of absolute byte differences.
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
    let mut sum = 0_u64;
    for y in 0..height {
        let aa = &a[y * a_stride..y * a_stride + width];
        let bb = &b[y * b_stride..y * b_stride + width];
        sum += sad(aa, bb);
    }
    sum
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
