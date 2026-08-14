use avelune::{
    container::v1::{ChromaLocation, VideoMeta},
    video::v1::Frame420,
};

use crate::error::{CliError, Result};

#[derive(Debug)]
pub struct Y4m {
    pub w: u32,
    pub h: u32,
    pub fps_n: u32,
    pub fps_d: u32,
    pub meta0: u32,
    pub frames: Vec<Frame420>,
}

pub fn parse(bytes: &[u8]) -> Result<Y4m> {
    let nl = bytes
        .iter()
        .position(|&x| x == b'\n')
        .ok_or_else(|| CliError::message("missing Y4M header"))?;
    let header =
        std::str::from_utf8(&bytes[..nl]).map_err(|_| CliError::message("bad Y4M UTF-8"))?;
    if !header.starts_with("YUV4MPEG2 ") {
        return Err(CliError::message("not YUV4MPEG2"));
    }
    let (mut w, mut h, mut fps_n, mut fps_d) = (None, None, 30u32, 1u32);
    let mut chroma = "420";
    let mut meta = VideoMeta::default();
    for token in header.split_whitespace().skip(1) {
        if let Some(x) = token.strip_prefix('W') {
            w = Some(x.parse().map_err(|_| CliError::message("bad Y4M width"))?);
        } else if let Some(x) = token.strip_prefix('H') {
            h = Some(x.parse().map_err(|_| CliError::message("bad Y4M height"))?);
        } else if let Some(x) = token.strip_prefix('F') {
            let mut parts = x.split(':');
            fps_n = parts
                .next()
                .ok_or_else(|| CliError::message("bad Y4M fps"))?
                .parse()
                .map_err(|_| CliError::message("bad Y4M fps"))?;
            fps_d = parts
                .next()
                .unwrap_or("1")
                .parse()
                .map_err(|_| CliError::message("bad Y4M fps"))?;
        } else if let Some(x) = token.strip_prefix('C') {
            chroma = x;
            meta.chroma = match x {
                "420mpeg2" => ChromaLocation::Left,
                "420jpeg" => ChromaLocation::Center,
                _ => ChromaLocation::Unspecified,
            };
        } else if let Some(x) = token.strip_prefix("XCOLORRANGE=") {
            meta.full_range = x.eq_ignore_ascii_case("FULL");
        }
    }
    if !matches!(chroma, "420" | "420jpeg" | "420mpeg2") {
        return Err(CliError::message(format!(
            "Draft Gen 1 baseline requires 8-bit 4:2:0 Y4M, got C{chroma}"
        )));
    }
    let (w, h) = (
        w.ok_or_else(|| CliError::message("Y4M has no width"))?,
        h.ok_or_else(|| CliError::message("Y4M has no height"))?,
    );
    if w % 2 != 0 || h % 2 != 0 {
        return Err(CliError::message("4:2:0 dimensions must be even"));
    }
    if fps_n == 0 || fps_d == 0 {
        return Err(CliError::message("Y4M frame rate must be non-zero"));
    }
    if fps_n > u16::MAX.into() || fps_d > u16::MAX.into() {
        return Err(CliError::message(
            "Y4M frame-rate numerator/denominator must fit the Draft Gen 1 16-bit fields",
        ));
    }
    let y = (w as usize)
        .checked_mul(h as usize)
        .ok_or_else(|| CliError::message("Y4M dimensions overflow"))?;
    let c = y / 4;
    let frame_bytes = y
        .checked_add(2 * c)
        .ok_or_else(|| CliError::message("Y4M frame size overflow"))?;
    let mut pos = nl + 1;
    let mut frames = Vec::new();
    while pos < bytes.len() {
        if !bytes[pos..].starts_with(b"FRAME") {
            return Err(CliError::message(format!("expected FRAME at byte {pos}")));
        }
        let n = bytes[pos..]
            .iter()
            .position(|&x| x == b'\n')
            .ok_or_else(|| CliError::message("truncated FRAME header"))?;
        pos += n + 1;
        if bytes.len().saturating_sub(pos) < frame_bytes {
            return Err(CliError::message("truncated Y4M frame"));
        }
        frames.push(Frame420::from_planes(
            w,
            h,
            bytes[pos..pos + y].to_vec(),
            bytes[pos + y..pos + y + c].to_vec(),
            bytes[pos + y + c..pos + frame_bytes].to_vec(),
        )?);
        pos += frame_bytes;
    }
    Ok(Y4m {
        w,
        h,
        fps_n,
        fps_d,
        meta0: meta.pack(),
        frames,
    })
}

pub fn emit(y4m: &Y4m) -> Vec<u8> {
    let meta = VideoMeta::unpack(y4m.meta0).unwrap_or_default();
    let chroma = match meta.chroma {
        ChromaLocation::Left => "420mpeg2",
        ChromaLocation::Center | ChromaLocation::Unspecified => "420jpeg",
    };
    let range = if meta.full_range { "FULL" } else { "LIMITED" };
    let mut out = format!(
        "YUV4MPEG2 W{} H{} F{}:{} Ip A1:1 C{} XCOLORRANGE={}\n",
        y4m.w, y4m.h, y4m.fps_n, y4m.fps_d, chroma, range
    )
    .into_bytes();
    for frame in &y4m.frames {
        out.extend_from_slice(b"FRAME\n");
        out.extend_from_slice(frame.y());
        out.extend_from_slice(frame.u());
        out.extend_from_slice(frame.v());
    }
    out
}

/// Packs a validated frame rate into a Draft Generation 1 stream flags field.
///
/// Both components must be in `1..=65_535`; [`parse`] enforces this contract for
/// Y4M input.
pub fn fps_flags(n: u32, d: u32) -> u32 {
    debug_assert!(n <= u16::MAX.into() && d <= u16::MAX.into());
    (n << 16) | d
}

/// Unpacks a frame rate from a Draft Generation 1 stream flags field.
///
/// Returns an error if either the numerator or denominator is zero.
pub fn fps_from_flags(flags: u32) -> Result<(u32, u32)> {
    let (n, d) = (flags >> 16, flags & u32::from(u16::MAX));
    if n == 0 || d == 0 {
        return Err(CliError::message(
            "video stream declares a zero frame-rate numerator or denominator",
        ));
    }
    Ok((n, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unrepresentable_y4m_profiles_and_frame_rates() {
        for chroma in ["420p10", "420p12", "420p16", "420paldv"] {
            let input = format!("YUV4MPEG2 W2 H2 F30:1 Ip A1:1 C{chroma}\n");
            assert!(parse(input.as_bytes()).is_err(), "accepted C{chroma}");
        }
        assert!(parse(b"YUV4MPEG2 W2 H2 F65536:1 Ip A1:1 C420\n").is_err());
        assert!(parse(b"YUV4MPEG2 W2 H2 F1:65536 Ip A1:1 C420\n").is_err());
    }

    #[test]
    fn accepts_representable_8_bit_420_locations() {
        for chroma in ["420", "420jpeg", "420mpeg2"] {
            let input = format!("YUV4MPEG2 W2 H2 F30:1 Ip A1:1 C{chroma}\n");
            assert!(parse(input.as_bytes()).is_ok(), "rejected C{chroma}");
        }
    }

    #[test]
    fn frame_rate_flags_reject_zero_components_and_round_trip() {
        assert!(fps_from_flags(0).is_err());
        assert!(fps_from_flags(fps_flags(30, 0)).is_err());
        assert!(fps_from_flags(fps_flags(0, 1)).is_err());
        assert_eq!(fps_from_flags(fps_flags(30, 1)).unwrap(), (30, 1));
    }
}
