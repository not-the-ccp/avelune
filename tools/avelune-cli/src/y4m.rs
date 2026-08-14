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
            meta.chroma = if x.starts_with("420mpeg2") {
                ChromaLocation::Left
            } else if x.starts_with("420jpeg") || x.starts_with("420paldv") {
                ChromaLocation::Center
            } else {
                ChromaLocation::Unspecified
            };
        } else if let Some(x) = token.strip_prefix("XCOLORRANGE=") {
            meta.full_range = x.eq_ignore_ascii_case("FULL");
        }
    }
    if !chroma.starts_with("420") {
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
    let chroma = if meta.chroma == ChromaLocation::Left {
        "420mpeg2"
    } else {
        "420jpeg"
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

pub fn fps_flags(n: u32, d: u32) -> u32 {
    (n.min(65535) << 16) | d.min(65535)
}
pub fn fps_from_flags(flags: u32) -> (u32, u32) {
    ((flags >> 16).max(1), (flags & 65535).max(1))
}
