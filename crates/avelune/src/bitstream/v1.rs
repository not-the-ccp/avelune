//! Canonical implementation of the Draft Generation 1 integer and entropy primitives.
//!
//! The normative representation is `spec/common/001-entropy-v1.adoc`. The decoder uses a
//! optimized packed 4096-entry rANS lookup: one load yields symbol, frequency and
//! cumulative frequency, avoiding the dependent lookup chain used by the small reference crate.

/// Errors produced while parsing canonical varints or V1 entropy payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Appends one canonical unsigned base-128 varint.
pub fn put_uvarint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Reads one canonical unsigned base-128 varint and advances `pos` only over consumed bytes.
pub fn get_uvarint(input: &[u8], pos: &mut usize) -> Result<u64, BitstreamError> {
    let start = *pos;
    let mut value = 0u64;
    for byte_index in 0..10usize {
        let byte = *input.get(*pos).ok_or(BitstreamError::UnexpectedEof)?;
        *pos += 1;
        let payload = byte & 0x7f;
        if byte_index == 9 && payload > 1 {
            return Err(BitstreamError::VarintOverflow);
        }
        value |= u64::from(payload) << (byte_index * 7);
        if byte & 0x80 == 0 {
            if *pos - start > 1 && payload == 0 {
                return Err(BitstreamError::OverlongVarint);
            }
            return Ok(value);
        }
    }
    Err(BitstreamError::VarintTooLong)
}

/// Maps a signed 32-bit integer to its ZigZag representation.
#[inline]
pub const fn zigzag_i32(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}
/// Inverts [`zigzag_i32`].
#[inline]
pub const fn unzigzag_u32(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}
/// Appends a signed 32-bit ZigZag varint.
pub fn put_svarint_i32(value: i32, out: &mut Vec<u8>) {
    put_uvarint(u64::from(zigzag_i32(value)), out);
}
/// Reads a signed 32-bit ZigZag varint.
pub fn get_svarint_i32(input: &[u8], pos: &mut usize) -> Result<i32, BitstreamError> {
    let encoded = get_uvarint(input, pos)?;
    let encoded = u32::try_from(encoded).map_err(|_| BitstreamError::VarintOverflow)?;
    Ok(unzigzag_u32(encoded))
}

const RANS_BITS: u32 = 12;
const RANS_SCALE: u32 = 1 << RANS_BITS;
const RANS_MASK: u32 = RANS_SCALE - 1;
const RANS_L: u32 = 1 << 23;
const LOOKUP_LEN: usize = RANS_SCALE as usize;

#[derive(Clone)]
struct EncodeModel {
    freq: [u16; 256],
    cum: [u16; 256],
}

/// Reusable entropy decode storage.  Keeping this on a codec instance avoids allocating the
/// decoded token buffer and the 16-KiB packed rANS table for every entropy lane.
#[derive(Debug)]
pub struct EntropyScratch {
    output: Vec<u8>,
    // packed: symbol:8 | (frequency-1):12 | cumulative:12
    lookup: Box<[u32; LOOKUP_LEN]>,
}
impl Default for EntropyScratch {
    fn default() -> Self {
        Self::new()
    }
}
impl EntropyScratch {
    /// Creates empty reusable entropy storage.
    pub fn new() -> Self {
        Self {
            output: Vec::new(),
            lookup: Box::new([0; LOOKUP_LEN]),
        }
    }
    /// Current reusable output capacity in bytes.
    pub fn output_capacity(&self) -> usize {
        self.output.capacity()
    }
}

/// Validates a 256-symbol frequency table and builds the packed rANS encode model.
fn build_encode_model(freq: [u16; 256]) -> Result<EncodeModel, BitstreamError> {
    let mut cum = [0u16; 256];
    let mut total = 0u32;
    for (i, &f) in freq.iter().enumerate() {
        cum[i] = total as u16;
        total = total
            .checked_add(u32::from(f))
            .ok_or(BitstreamError::BadEntropyModel)?;
        if total > RANS_SCALE {
            return Err(BitstreamError::BadEntropyModel);
        }
    }
    if total != RANS_SCALE {
        return Err(BitstreamError::BadEntropyModel);
    }
    Ok(EncodeModel { freq, cum })
}

/// Builds the packed 4096-entry decode lookup from a validated symbol/cumulative model.
fn build_decode_lookup(
    freq: &[u16; 256],
    lookup: &mut [u32; LOOKUP_LEN],
) -> Result<(), BitstreamError> {
    let mut cumulative = 0u32;
    for (symbol, &f16) in freq.iter().enumerate() {
        let f = u32::from(f16);
        if f == 0 {
            continue;
        }
        let end = cumulative
            .checked_add(f)
            .ok_or(BitstreamError::BadEntropyModel)?;
        if end > RANS_SCALE {
            return Err(BitstreamError::BadEntropyModel);
        }
        let packed = ((symbol as u32) << 24) | ((f - 1) << 12) | cumulative;
        lookup[cumulative as usize..end as usize].fill(packed);
        cumulative = end;
    }
    if cumulative != RANS_SCALE {
        return Err(BitstreamError::BadEntropyModel);
    }
    Ok(())
}

/// Constructs the frequency model for one byte sequence (non-normative encoder model selection).
fn normalize(data: &[u8]) -> EncodeModel {
    let mut counts = [0u32; 256];
    for &byte in data {
        counts[usize::from(byte)] += 1;
    }
    if data.is_empty() {
        let mut freq = [0u16; 256];
        freq[0] = RANS_SCALE as u16;
        return build_encode_model(freq).expect("singleton model sums to scale");
    }

    let n = data.len() as u64;
    let mut freq = [0u16; 256];
    let mut remainders = [0u64; 256];
    let mut sum = 0u32;
    for symbol in 0..256 {
        let count = counts[symbol];
        if count == 0 {
            continue;
        }
        let scaled = u64::from(count) * u64::from(RANS_SCALE);
        let q = (scaled / n).max(1) as u32;
        freq[symbol] = q as u16;
        remainders[symbol] = scaled % n;
        sum += q;
    }
    while sum < RANS_SCALE {
        let symbol = (0..256)
            .filter(|&i| counts[i] != 0)
            .max_by_key(|&i| remainders[i])
            .expect("nonempty source has a symbol");
        freq[symbol] += 1;
        remainders[symbol] = 0;
        sum += 1;
    }
    while sum > RANS_SCALE {
        let Some(symbol) = (0..256).filter(|&i| freq[i] > 1).max_by_key(|&i| counts[i]) else {
            break;
        };
        freq[symbol] -= 1;
        sum -= 1;
    }
    build_encode_model(freq).expect("normalization must sum to 4096")
}

/// tANS/rANS encodes one byte slice with the supplied model.
fn rans_encode(data: &[u8], model: &EncodeModel) -> Vec<u8> {
    let mut state = RANS_L;
    let mut reverse_renorm = Vec::with_capacity(data.len() / 2);
    for &symbol in data.iter().rev() {
        let i = usize::from(symbol);
        let f = u32::from(model.freq[i]);
        let c = u32::from(model.cum[i]);
        let max_before_emit = ((RANS_L >> RANS_BITS) << 8) * f;
        while state >= max_before_emit {
            reverse_renorm.push(state as u8);
            state >>= 8;
        }
        state = ((state / f) << RANS_BITS) + state % f + c;
    }
    let mut coded = Vec::with_capacity(4 + reverse_renorm.len());
    coded.extend_from_slice(&state.to_le_bytes());
    coded.extend(reverse_renorm.into_iter().rev());
    coded
}

/// Decodes one rANS block into `out`, enforcing exact output length.
fn rans_decode_into(
    coded: &[u8],
    raw_len: usize,
    lookup: &[u32; LOOKUP_LEN],
    output: &mut Vec<u8>,
) -> Result<(), BitstreamError> {
    if coded.len() < 4 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let mut state = u32::from_le_bytes(coded[..4].try_into().expect("four-byte prefix"));
    if state < RANS_L {
        return Err(BitstreamError::BadEntropyState);
    }
    let mut coded_pos = 4usize;
    output.clear();
    output.reserve(raw_len.saturating_sub(output.capacity()));
    for _ in 0..raw_len {
        let x = state & RANS_MASK;
        let packed = lookup[x as usize];
        let symbol = (packed >> 24) as u8;
        let frequency = ((packed >> 12) & RANS_MASK) + 1;
        let cumulative = packed & RANS_MASK;
        state = frequency * (state >> RANS_BITS) + (x - cumulative);
        while state < RANS_L {
            let byte = *coded.get(coded_pos).ok_or(BitstreamError::UnexpectedEof)?;
            coded_pos += 1;
            state = (state << 8) | u32::from(byte);
        }
        output.push(symbol);
    }
    if coded_pos != coded.len() {
        return Err(BitstreamError::TrailingData);
    }
    Ok(())
}

/// Compresses a byte sequence using V1 raw mode or static byte rANS mode 2.
pub fn entropy_compress(data: &[u8]) -> Vec<u8> {
    let model = normalize(data);
    let coded = rans_encode(data, &model);
    let used = model.freq.iter().filter(|&&f| f != 0).count();

    let mut entropy = Vec::with_capacity(coded.len() + used * 3 + 11);
    entropy.push(2);
    entropy.extend_from_slice(&(data.len() as u32).to_le_bytes());
    entropy.extend_from_slice(&(used as u16).to_le_bytes());
    for (symbol, &frequency) in model.freq.iter().enumerate() {
        if frequency != 0 {
            entropy.push(symbol as u8);
            put_uvarint(u64::from(frequency), &mut entropy);
        }
    }
    entropy.extend_from_slice(&(coded.len() as u32).to_le_bytes());
    entropy.extend_from_slice(&coded);

    if entropy.len() >= data.len() + 5 {
        let mut raw = Vec::with_capacity(data.len() + 5);
        raw.push(0);
        raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
        raw.extend_from_slice(data);
        raw
    } else {
        entropy
    }
}

/// Reads a mode-2 canonical-uvarint symbol/frequency model and rejects invalid models.
fn parse_entropy_model(
    input: &[u8],
    used: usize,
    pos: &mut usize,
    freq: &mut [u16; 256],
) -> Result<(), BitstreamError> {
    freq.fill(0);
    for _ in 0..used {
        let symbol = usize::from(*input.get(*pos).ok_or(BitstreamError::UnexpectedEof)?);
        *pos += 1;
        let frequency = get_uvarint(input, pos)?;
        if frequency == 0 || frequency > u64::from(RANS_SCALE) || freq[symbol] != 0 {
            return Err(BitstreamError::BadEntropyModel);
        }
        freq[symbol] = frequency as u16;
    }
    Ok(())
}

/// Decompresses one entropy block into reusable storage and returns the decoded token slice.
pub fn entropy_decompress_with_scratch<'a>(
    input: &[u8],
    max_output: usize,
    scratch: &'a mut EntropyScratch,
) -> Result<&'a [u8], BitstreamError> {
    if input.len() < 5 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let mode = input[0];
    let raw_len = u32::from_le_bytes(input[1..5].try_into().expect("length checked")) as usize;
    if raw_len > max_output {
        return Err(BitstreamError::BadEntropyHeader);
    }
    if mode == 0 {
        let expected = 5usize
            .checked_add(raw_len)
            .ok_or(BitstreamError::BadEntropyHeader)?;
        if input.len() != expected {
            return Err(if input.len() < expected {
                BitstreamError::UnexpectedEof
            } else {
                BitstreamError::TrailingData
            });
        }
        scratch.output.clear();
        scratch.output.extend_from_slice(&input[5..]);
        return Ok(&scratch.output);
    }
    if mode != 2 {
        return Err(BitstreamError::BadEntropyHeader);
    }
    if input.len() < 7 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let used = u16::from_le_bytes(input[5..7].try_into().expect("length checked")) as usize;
    if !(1..=256).contains(&used) {
        return Err(BitstreamError::BadEntropyModel);
    }
    let mut pos = 7usize;
    let mut freq = [0u16; 256];
    parse_entropy_model(input, used, &mut pos, &mut freq)?;
    build_decode_lookup(&freq, &mut scratch.lookup)?;
    if input.len() < pos + 4 {
        return Err(BitstreamError::UnexpectedEof);
    }
    let coded_len =
        u32::from_le_bytes(input[pos..pos + 4].try_into().expect("length checked")) as usize;
    pos += 4;
    let end = pos
        .checked_add(coded_len)
        .ok_or(BitstreamError::BadEntropyHeader)?;
    if input.len() != end {
        return Err(if input.len() < end {
            BitstreamError::UnexpectedEof
        } else {
            BitstreamError::TrailingData
        });
    }
    rans_decode_into(
        &input[pos..end],
        raw_len,
        &scratch.lookup,
        &mut scratch.output,
    )?;
    Ok(&scratch.output)
}

/// Decompresses one entropy block with an explicit output-size ceiling.
pub fn entropy_decompress(input: &[u8], max_output: usize) -> Result<Vec<u8>, BitstreamError> {
    let mut scratch = EntropyScratch::new();
    entropy_decompress_with_scratch(input, max_output, &mut scratch)?;
    Ok(scratch.output)
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
        for value in [i32::MIN, -100000, -1, 0, 1, 127, 128, 100000, i32::MAX] {
            let mut bytes = Vec::new();
            put_svarint_i32(value, &mut bytes);
            let mut pos = 0;
            assert_eq!(get_svarint_i32(&bytes, &mut pos), Ok(value));
            assert_eq!(pos, bytes.len());
        }
    }

    #[test]
    fn canonical_varint_edges() {
        for value in [0, 1, 127, 128, u32::MAX as u64, u64::MAX] {
            let mut bytes = Vec::new();
            put_uvarint(value, &mut bytes);
            let mut pos = 0;
            assert_eq!(get_uvarint(&bytes, &mut pos), Ok(value));
            assert_eq!(pos, bytes.len());
        }
        for bad in [
            &[0x80, 0][..],
            &[0x81, 0][..],
            &[0xff; 11][..],
            &[0x82, 0x80, 0][..],
        ] {
            let mut pos = 0;
            assert!(get_uvarint(bad, &mut pos).is_err());
        }
    }

    #[test]
    fn entropy_roundtrip_and_scratch_reuse() {
        let patterns = [
            Vec::new(),
            vec![7u8; 10000],
            (0..20000).map(|i| ((i * i + i * 17) >> 3) as u8).collect(),
        ];
        let mut scratch = EntropyScratch::new();
        for data in patterns {
            let coded = entropy_compress(&data);
            let decoded = entropy_decompress_with_scratch(&coded, 1 << 20, &mut scratch).unwrap();
            assert_eq!(decoded, data);
        }
        assert!(scratch.output_capacity() >= 10000);
    }

    #[test]
    fn rejects_obsolete_entropy_mode_one() {
        assert_eq!(
            entropy_decompress(&[1, 0, 0, 0, 0], 1),
            Err(BitstreamError::BadEntropyHeader)
        );
    }
}
