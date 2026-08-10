//! Canonical integer and static-rANS primitives shared by Avelune Draft Generation 1.
//!
//! This crate is intentionally small. The normative representation is described in
//! `spec/common/001-entropy-v1.adoc`; this implementation is not the specification.

#![forbid(unsafe_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Errors produced while parsing canonical varints or rANS payloads.
pub enum BitstreamError {
    UnexpectedEof,
    VarintTooLong,
    VarintOverflow,
    OverlongVarint,
    BadEntropyHeader,
    BadEntropyModel,
    BadEntropyState,
    TrailingData,
}

/// Appends a canonical unsigned LEB128-style varint.
pub fn put_uvarint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Reads one canonical unsigned varint and advances `pos`.
pub fn get_uvarint(input: &[u8], pos: &mut usize) -> Result<u64, BitstreamError> {
    let start = *pos;
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *input.get(*pos).ok_or(BitstreamError::UnexpectedEof)?;
        *pos += 1;
        if shift == 63 && (byte & 0x7e) != 0 {
            return Err(BitstreamError::VarintOverflow);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            let encoded_len = *pos - start;
            if encoded_len > 1 && byte == 0 {
                return Err(BitstreamError::OverlongVarint);
            }
            return Ok(value);
        }
    }
    Err(BitstreamError::VarintTooLong)
}

#[inline]
/// Maps a signed 32-bit integer to an unsigned ZigZag value.
pub const fn zigzag_i32(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}
#[inline]
/// Inverts [`zigzag_i32`].
pub const fn unzigzag_u32(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}
/// Appends a canonical signed 32-bit ZigZag varint.
pub fn put_svarint_i32(value: i32, out: &mut Vec<u8>) {
    put_uvarint(zigzag_i32(value) as u64, out);
}
/// Reads one canonical signed 32-bit ZigZag varint.
pub fn get_svarint_i32(input: &[u8], pos: &mut usize) -> Result<i32, BitstreamError> {
    let v = get_uvarint(input, pos)?;
    if v > u32::MAX as u64 {
        return Err(BitstreamError::VarintOverflow);
    }
    Ok(unzigzag_u32(v as u32))
}

const RANS_BITS: u32 = 12;
const RANS_SCALE: u32 = 1 << RANS_BITS;
const RANS_MASK: u32 = RANS_SCALE - 1;
const RANS_L: u32 = 1 << 23;

#[derive(Clone)]
struct Model {
    freq: [u16; 256],
    cum: [u16; 256],
    lookup: [u8; RANS_SCALE as usize],
}

fn normalize(data: &[u8]) -> Model {
    let mut counts = [0u32; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    if data.is_empty() {
        let mut f = [0u16; 256];
        f[0] = RANS_SCALE as u16;
        return build_model(f).expect("valid singleton model");
    }
    let n = data.len() as u64;
    let mut freq = [0u16; 256];
    let mut rem = [0u64; 256];
    let mut sum = 0u32;
    for i in 0..256 {
        if counts[i] == 0 {
            continue;
        }
        let scaled = counts[i] as u64 * RANS_SCALE as u64;
        let mut q = (scaled / n) as u32;
        if q == 0 {
            q = 1;
        }
        freq[i] = q as u16;
        rem[i] = scaled % n;
        sum += q;
    }
    while sum < RANS_SCALE {
        let mut best = 0usize;
        for i in 1..256 {
            if counts[i] > 0 && rem[i] > rem[best] {
                best = i;
            }
        }
        freq[best] += 1;
        rem[best] = 0;
        sum += 1;
    }
    while sum > RANS_SCALE {
        let mut best = None;
        let mut score = 0u32;
        for i in 0..256 {
            if freq[i] > 1 && counts[i] >= score {
                best = Some(i);
                score = counts[i];
            }
        }
        if let Some(i) = best {
            freq[i] -= 1;
            sum -= 1;
        } else {
            break;
        }
    }
    build_model(freq).expect("normalized model")
}

fn build_model(freq: [u16; 256]) -> Result<Model, BitstreamError> {
    let mut cum = [0u16; 256];
    let mut lookup = [0u8; RANS_SCALE as usize];
    let mut c = 0u32;
    for i in 0..256 {
        cum[i] = c as u16;
        let f = freq[i] as u32;
        if c.checked_add(f).ok_or(BitstreamError::BadEntropyModel)? > RANS_SCALE {
            return Err(BitstreamError::BadEntropyModel);
        }
        for x in c..c + f {
            lookup[x as usize] = i as u8;
        }
        c += f;
    }
    if c != RANS_SCALE {
        return Err(BitstreamError::BadEntropyModel);
    }
    Ok(Model { freq, cum, lookup })
}

fn rans_encode(data: &[u8], model: &Model) -> Vec<u8> {
    let mut state = RANS_L;
    let mut renorm = Vec::new();
    for &s in data.iter().rev() {
        let f = model.freq[s as usize] as u32;
        let c = model.cum[s as usize] as u32;
        let x_max = ((RANS_L >> RANS_BITS) << 8).saturating_mul(f);
        while state >= x_max {
            renorm.push((state & 0xff) as u8);
            state >>= 8;
        }
        state = ((state / f) << RANS_BITS) + (state % f) + c;
    }
    let mut out = Vec::with_capacity(4 + renorm.len());
    out.extend(state.to_le_bytes());
    out.extend(renorm.into_iter().rev());
    out
}

fn rans_decode(input: &[u8], n: usize, model: &Model) -> Result<Vec<u8>, BitstreamError> {
    if input.len() < 4 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let mut state = u32::from_le_bytes(input[0..4].try_into().unwrap());
    if state < RANS_L {
        return Err(BitstreamError::BadEntropyState);
    }
    let mut pos = 4usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let x = (state & RANS_MASK) as usize;
        let s = model.lookup[x];
        let f = model.freq[s as usize] as u32;
        let c = model.cum[s as usize] as u32;
        state = f * (state >> RANS_BITS) + (x as u32 - c);
        while state < RANS_L {
            let b = *input.get(pos).ok_or(BitstreamError::UnexpectedEof)?;
            pos += 1;
            state = (state << 8) | u32::from(b);
        }
        out.push(s);
    }
    if pos != input.len() {
        return Err(BitstreamError::TrailingData);
    }
    Ok(out)
}

/// Self-contained byte entropy block.
///
/// Mode 0 is raw. Mode 1 is the original fixed-width static-model rANS
/// representation. Mode 2 has identical rANS semantics but writes each nonzero
/// frequency as a canonical uvarint. V1 encoders use mode 2 when entropy coding
/// wins; decoders retain mode 1 so pre-freeze development vectors remain useful.
/// Compresses a byte sequence using the Draft Generation 1 static rANS representation.
pub fn entropy_compress(data: &[u8]) -> Vec<u8> {
    let model = normalize(data);
    let coded = rans_encode(data, &model);
    let used = model.freq.iter().filter(|&&f| f != 0).count();
    let mut r = Vec::new();
    r.push(2);
    r.extend((data.len() as u32).to_le_bytes());
    r.extend((used as u16).to_le_bytes());
    for i in 0..256 {
        if model.freq[i] != 0 {
            r.push(i as u8);
            put_uvarint(model.freq[i] as u64, &mut r);
        }
    }
    r.extend((coded.len() as u32).to_le_bytes());
    r.extend(coded);
    if r.len() >= data.len() + 5 {
        let mut raw = Vec::with_capacity(data.len() + 5);
        raw.push(0);
        raw.extend((data.len() as u32).to_le_bytes());
        raw.extend(data);
        raw
    } else {
        r
    }
}

/// Decompresses one static-rANS payload with an explicit output-size ceiling.
pub fn entropy_decompress(input: &[u8], max_output: usize) -> Result<Vec<u8>, BitstreamError> {
    if input.len() < 5 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let mode = input[0];
    let n = u32::from_le_bytes(input[1..5].try_into().unwrap()) as usize;
    if n > max_output {
        return Err(BitstreamError::BadEntropyHeader);
    }
    if mode == 0 {
        if input.len() != 5 + n {
            return Err(if input.len() < 5 + n {
                BitstreamError::UnexpectedEof
            } else {
                BitstreamError::TrailingData
            });
        }
        return Ok(input[5..].to_vec());
    }
    if mode != 1 && mode != 2 {
        return Err(BitstreamError::BadEntropyHeader);
    }
    if input.len() < 7 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let used = u16::from_le_bytes(input[5..7].try_into().unwrap()) as usize;
    if used == 0 || used > 256 {
        return Err(BitstreamError::BadEntropyModel);
    }
    let mut freq = [0u16; 256];
    let mut p = 7usize;
    if mode == 1 {
        let model_end = 7usize
            .checked_add(used * 3)
            .ok_or(BitstreamError::BadEntropyHeader)?;
        if input.len() < model_end + 4 {
            return Err(BitstreamError::UnexpectedEof);
        }
        for _ in 0..used {
            let sym = input[p] as usize;
            let f = u16::from_le_bytes([input[p + 1], input[p + 2]]);
            p += 3;
            if f == 0 || freq[sym] != 0 {
                return Err(BitstreamError::BadEntropyModel);
            }
            freq[sym] = f;
        }
    } else {
        for _ in 0..used {
            let sym = *input.get(p).ok_or(BitstreamError::UnexpectedEof)? as usize;
            p += 1;
            let fv = get_uvarint(input, &mut p)?;
            if fv == 0 || fv > RANS_SCALE as u64 || freq[sym] != 0 {
                return Err(BitstreamError::BadEntropyModel);
            }
            freq[sym] = fv as u16;
        }
    }
    let model = build_model(freq)?;
    if input.len() < p + 4 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let clen = u32::from_le_bytes(input[p..p + 4].try_into().unwrap()) as usize;
    p += 4;
    if input.len() != p + clen {
        return Err(if input.len() < p + clen {
            BitstreamError::UnexpectedEof
        } else {
            BitstreamError::TrailingData
        });
    }
    rans_decode(&input[p..], n, &model)
}

impl std::fmt::Display for BitstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BitstreamError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_roundtrip() {
        for v in [i32::MIN, -100000, -1, 0, 1, 127, 128, 100000, i32::MAX] {
            let mut b = Vec::new();
            put_svarint_i32(v, &mut b);
            let mut p = 0;
            assert_eq!(get_svarint_i32(&b, &mut p), Ok(v));
            assert_eq!(p, b.len());
        }
    }
    #[test]
    fn rejects_overlong_zero() {
        let mut p = 0;
        assert_eq!(
            get_uvarint(&[0x80, 0], &mut p),
            Err(BitstreamError::OverlongVarint)
        );
    }
    #[test]
    fn entropy_roundtrip_patterns() {
        let mut x = Vec::new();
        for i in 0..20000 {
            x.push(((i * i + i * 17) >> 3) as u8)
        }
        for data in [&b""[..], &b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaa"[..], &x[..]] {
            let c = entropy_compress(data);
            let d = entropy_decompress(&c, 1 << 20).unwrap();
            assert_eq!(d, data);
        }
    }
    #[test]
    fn entropy_compresses_low_entropy() {
        let d = vec![7u8; 10000];
        let c = entropy_compress(&d);
        assert!(c.len() < 200);
        assert_eq!(entropy_decompress(&c, 20000).unwrap(), d);
    }
}
