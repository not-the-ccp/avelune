use avelune::container::v1::{
    ChromaLocation, ColorMatrix, ColorPrimaries, ColorTransfer, VideoMeta,
};
use std::{fs, process::Command};

use crate::{
    codec::{AudioData, DecodedMedia},
    error::{CliError, Result},
    io_util::{TempPath, run_checked},
    y4m,
};

pub fn has_audio(input: &str) -> bool {
    Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
            input,
        ])
        .output()
        .ok()
        .is_some_and(|o| o.status.success() && !o.stdout.is_empty())
}

pub fn probe_video_meta(input: &str) -> u32 {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=color_range,color_space,color_transfer,color_primaries,chroma_location",
            "-of",
            "default=nw=1",
            input,
        ])
        .output();
    let Ok(out) = out else { return 0 };
    if !out.status.success() {
        return 0;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut meta = VideoMeta::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "color_range" => meta.full_range = value == "pc" || value == "jpeg",
            "color_space" => {
                meta.matrix = match value {
                    "bt709" => ColorMatrix::Bt709,
                    "bt2020nc" => ColorMatrix::Bt2020Ncl,
                    "smpte170m" | "bt470bg" => ColorMatrix::Bt601,
                    _ => ColorMatrix::Unspecified,
                }
            }
            "color_transfer" => {
                meta.transfer = match value {
                    "bt709" => ColorTransfer::Bt709,
                    "iec61966-2-1" => ColorTransfer::Srgb,
                    "smpte2084" => ColorTransfer::Pq,
                    "arib-std-b67" => ColorTransfer::Hlg,
                    _ => ColorTransfer::Unspecified,
                }
            }
            "color_primaries" => {
                meta.primaries = match value {
                    "bt709" => ColorPrimaries::Bt709,
                    "bt2020" => ColorPrimaries::Bt2020,
                    "smpte170m" | "bt470bg" => ColorPrimaries::Bt601,
                    _ => ColorPrimaries::Unspecified,
                }
            }
            "chroma_location" => {
                meta.chroma = match value {
                    "left" => ChromaLocation::Left,
                    "center" => ChromaLocation::Center,
                    _ => ChromaLocation::Unspecified,
                }
            }
            _ => {}
        }
    }
    meta.pack()
}

/// Extracts the first video stream as 8-bit 4:2:0 Y4M with `ffmpeg`.
///
/// `seconds` limits the decoded duration and `size` requests a `WIDTHxHEIGHT`
/// scale. The intermediate Y4M file is removed when this function returns,
/// including on errors.
pub fn extract_video(input: &str, seconds: Option<&str>, size: Option<&str>) -> Result<y4m::Y4m> {
    let temp = TempPath::new("encode-video.y4m")?;
    let mut command = Command::new("ffmpeg");
    command.args(["-y", "-loglevel", "error"]);
    if let Some(seconds) = seconds {
        command.args(["-t", seconds]);
    }
    command.args(["-i", input]);
    if let Some(size) = size {
        command.args([
            "-vf",
            &format!("scale={}:flags=lanczos", size.replace('x', ":")),
        ]);
    }
    command.args(["-pix_fmt", "yuv420p", "-f", "yuv4mpegpipe"]);
    command.arg(temp.path());
    run_checked(&mut command, "ffmpeg video decode")?;
    let bytes = fs::read(temp.path())
        .map_err(|e| CliError::io(format!("read {}", temp.path().display()), e))?;
    let mut parsed = y4m::parse(&bytes)?;
    let probed = probe_video_meta(input);
    if probed != 0 {
        let mut source = VideoMeta::unpack(probed).unwrap_or_default();
        let y4m_meta = VideoMeta::unpack(parsed.meta0).unwrap_or_default();
        if source.chroma == ChromaLocation::Unspecified {
            source.chroma = y4m_meta.chroma;
        }
        parsed.meta0 = source.pack();
    }
    Ok(parsed)
}

/// Extracts the first audio stream as stereo 48 kHz signed 16-bit PCM.
///
/// Returns `Ok(None)` when the input has no audio stream. The intermediate PCM
/// file is removed when this function returns, including on errors.
pub fn extract_audio(input: &str, seconds: Option<&str>) -> Result<Option<AudioData>> {
    if !has_audio(input) {
        return Ok(None);
    }
    let temp = TempPath::new("encode-audio.s16")?;
    let mut command = Command::new("ffmpeg");
    command.args(["-y", "-loglevel", "error"]);
    if let Some(seconds) = seconds {
        command.args(["-t", seconds]);
    }
    command.args([
        "-i", input, "-vn", "-ar", "48000", "-ac", "2", "-f", "s16le",
    ]);
    command.arg(temp.path());
    run_checked(&mut command, "ffmpeg audio decode")?;
    let bytes = fs::read(temp.path())
        .map_err(|e| CliError::io(format!("read {}", temp.path().display()), e))?;
    if bytes.len() % 2 != 0 {
        return Err(CliError::message("ffmpeg returned odd-length s16le audio"));
    }
    let samples = bytes
        .chunks_exact(2)
        .map(|x| i16::from_le_bytes([x[0], x[1]]))
        .collect();
    Ok(Some(AudioData {
        rate: 48_000,
        channels: 2,
        samples,
    }))
}

/// Encodes decoded media to `output` with `ffmpeg`.
///
/// Video is supplied to `ffmpeg` through a temporary Y4M file and audio through
/// a temporary signed 16-bit PCM file. Both files and their private temporary
/// directories are removed when this function returns, including on errors.
pub fn write_media(media: &DecodedMedia, output: &str) -> Result<()> {
    if media.video.is_none() && media.audio.is_none() {
        return Err(CliError::message(
            "container has no decodable media streams",
        ));
    }
    let video_temp = TempPath::new("decode-video.y4m")?;
    let audio_temp = TempPath::new("decode-audio.s16")?;
    if let Some(video) = &media.video {
        fs::write(video_temp.path(), y4m::emit(video))
            .map_err(|e| CliError::io(format!("write {}", video_temp.path().display()), e))?;
    }
    if let Some(audio) = &media.audio {
        let mut raw = Vec::with_capacity(audio.samples.len() * 2);
        for &sample in &audio.samples {
            raw.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(audio_temp.path(), raw)
            .map_err(|e| CliError::io(format!("write {}", audio_temp.path().display()), e))?;
    }
    let mut command = Command::new("ffmpeg");
    command.args(["-y", "-loglevel", "error"]);
    if media.video.is_some() {
        command.arg("-i").arg(video_temp.path());
    }
    if let Some(audio) = &media.audio {
        command
            .args([
                "-f",
                "s16le",
                "-ar",
                &audio.rate.to_string(),
                "-ac",
                &audio.channels.to_string(),
                "-i",
            ])
            .arg(audio_temp.path());
    }
    if media.video.is_some() {
        command.args(["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
    }
    if media.audio.is_some() {
        command.args(["-c:a", "aac"]);
    }
    command.arg(output);
    run_checked(&mut command, "ffmpeg media encode")
}
