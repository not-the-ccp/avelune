use std::{collections::BTreeMap, convert::Infallible};

use avelune::{
    audio::v1::{self as audio, EncodeOptions as AudioOptions},
    container::v1::{
        self as container, ContainerStreamDecoder, DecodedOutput, Packet, PacketKind, StreamDesc,
        StreamKind, TIMEBASE,
    },
    video::v1::{EncodeOptions as VideoOptions, EncoderPreset, Frame420, VideoEncoder},
};

use crate::{
    error::{CliError, Result},
    y4m::{self, Y4m},
};

#[derive(Debug)]
pub struct AudioData {
    pub rate: u32,
    pub channels: u8,
    pub samples: Vec<i16>,
}
#[derive(Debug)]
pub struct DecodedMedia {
    pub video: Option<Y4m>,
    pub audio: Option<AudioData>,
}
#[derive(Debug, Default)]
pub struct VerifySummary {
    pub video_frames: BTreeMap<u16, usize>,
    pub audio_packets: BTreeMap<u16, usize>,
    pub epochs: usize,
}

pub fn video_options(qstep: u16, preset: EncoderPreset) -> VideoOptions {
    let (motion_radius, max_refs) = match preset {
        EncoderPreset::Fast => (2, 1),
        EncoderPreset::Balanced => (4, 1),
        EncoderPreset::Quality => (5, 4),
    };
    VideoOptions {
        qstep,
        motion_radius,
        max_refs,
        preset,
        allow_palette: true,
    }
}

fn scene_cut(a: &Frame420, b: &Frame420) -> bool {
    let step = (a.y().len() / 4096).max(1);
    let mut sum = 0u64;
    let mut count = 0u64;
    for i in (0..a.y().len()).step_by(step) {
        sum += u64::from((i32::from(a.y()[i]) - i32::from(b.y()[i])).unsigned_abs());
        count += 1;
    }
    sum / count.max(1) > 48
}

pub fn encode_av(
    y4m: &Y4m,
    audio_input: Option<(&[i16], u8, u32)>,
    video_q: u16,
    audio_q: u16,
    epoch_frames: usize,
    preset: EncoderPreset,
) -> Result<Vec<u8>> {
    if epoch_frames == 0 {
        return Err(CliError::message("epoch length must be > 0"));
    }
    if y4m.frames.is_empty() {
        return Err(CliError::message("input contains no video frames"));
    }
    let video_options = video_options(video_q, preset);
    let frame_us = (1_000_000u64 * u64::from(y4m.fps_d)) / u64::from(y4m.fps_n);
    let mut starts = vec![0usize];
    for i in 1..y4m.frames.len() {
        if (i % epoch_frames == 0 || scene_cut(&y4m.frames[i - 1], &y4m.frames[i]))
            && *starts.last().expect("starts is non-empty") != i
        {
            starts.push(i);
        }
    }
    starts.push(y4m.frames.len());
    let mut epochs = Vec::new();
    for epoch_index in 0..starts.len() - 1 {
        let start = starts[epoch_index];
        let end = starts[epoch_index + 1];
        if start == end {
            continue;
        }
        let pts = start as u64 * frame_us;
        let end_pts = end as u64 * frame_us;
        let mut packets = Vec::new();
        packets.push(Packet {
            kind: PacketKind::EpochStart,
            flags: 0,
            stream_id: 0,
            pts,
            duration: (end_pts - pts) as u32,
            payload: (epoch_index as u32).to_le_bytes().to_vec(),
        });
        let mut encoder = VideoEncoder::new(video_options);
        for i in start..end {
            let encoded = encoder.encode_shared(i as u64, &y4m.frames[i])?;
            packets.push(Packet {
                kind: PacketKind::VideoFrame,
                flags: 0,
                stream_id: 1,
                pts: i as u64 * frame_us,
                duration: frame_us as u32,
                payload: encoded.packet,
            });
        }
        if let Some((samples, channels, rate)) = audio_input {
            let ch = channels as usize;
            if ch == 0 || samples.len() % ch != 0 {
                return Err(CliError::message(
                    "audio samples are not aligned to the channel count",
                ));
            }
            let total_frames = samples.len() / ch;
            let start_af = ((pts * u64::from(rate)) / 1_000_000) as usize;
            let end_af =
                ((end_pts * u64::from(rate)) / 1_000_000).min(total_frames as u64) as usize;
            let mut af = start_af;
            while af < end_af {
                let n = (end_af - af).min(960);
                let coded = audio::encode(
                    &samples[af * ch..(af + n) * ch],
                    AudioOptions {
                        sample_rate: rate,
                        channels,
                        qstep: audio_q,
                        mid_side: true,
                    },
                )?;
                let audio_pts = af as u64 * 1_000_000 / u64::from(rate);
                packets.push(Packet {
                    kind: PacketKind::AudioFrame,
                    flags: 0,
                    stream_id: 2,
                    pts: audio_pts,
                    duration: (n as u64 * 1_000_000 / u64::from(rate)) as u32,
                    payload: coded,
                });
                af += n;
            }
        }
        // EpochStart is a structural boundary. Audio timestamp rounding can place the first audio
        // packet a few microseconds before the video-derived epoch PTS, so sort the boundary first.
        packets.sort_by_key(|p| {
            (
                u8::from(p.kind != PacketKind::EpochStart),
                p.pts,
                match p.kind {
                    PacketKind::VideoFrame => 0u8,
                    PacketKind::AudioFrame => 1,
                    PacketKind::Metadata => 2,
                    PacketKind::EpochStart => 0,
                },
            )
        });
        let mut bytes = Vec::new();
        for packet in packets {
            container::encode_packet_checked(&packet, &mut bytes)?;
        }
        epochs.push((epoch_index as u32, pts, (end_pts - pts) as u32, bytes));
    }
    let mut streams = vec![StreamDesc {
        id: 1,
        kind: StreamKind::Video,
        codec: 1,
        timescale: TIMEBASE,
        param0: y4m.w,
        param1: y4m.h,
        flags: y4m::fps_flags(y4m.fps_n, y4m.fps_d),
        meta0: y4m.meta0,
    }];
    if let Some((_, channels, rate)) = audio_input {
        streams.push(StreamDesc {
            id: 2,
            kind: StreamKind::Audio,
            codec: 1,
            timescale: TIMEBASE,
            param0: rate,
            param1: u32::from(channels),
            flags: 0,
            meta0: 0,
        });
    }
    Ok(container::build_file_checked(streams, epochs)?)
}

pub fn encode_audio_only(
    samples: &[i16],
    channels: u8,
    rate: u32,
    qstep: u16,
    epoch_seconds: usize,
) -> Result<Vec<u8>> {
    if channels == 0 || !samples.len().is_multiple_of(channels as usize) {
        return Err(CliError::message(
            "audio samples are not aligned to channels",
        ));
    }
    if epoch_seconds == 0 {
        return Err(CliError::message("epoch length must be > 0"));
    }
    let total = samples.len() / channels as usize;
    let epoch_sample_frames = epoch_seconds
        .checked_mul(rate as usize)
        .ok_or_else(|| CliError::message("audio epoch size overflow"))?;
    let mut epochs = Vec::new();
    let mut start = 0usize;
    let mut epoch_id = 0u32;
    while start < total {
        let end = (start + epoch_sample_frames).min(total);
        let pts = start as u64 * 1_000_000 / u64::from(rate);
        let end_pts = end as u64 * 1_000_000 / u64::from(rate);
        let mut bytes = Vec::new();
        container::encode_packet_checked(
            &Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts,
                duration: (end_pts - pts) as u32,
                payload: epoch_id.to_le_bytes().to_vec(),
            },
            &mut bytes,
        )?;
        let mut frame = start;
        while frame < end {
            let n = (end - frame).min(960);
            let begin = frame * channels as usize;
            let finish = (frame + n) * channels as usize;
            let payload = audio::encode(
                &samples[begin..finish],
                AudioOptions {
                    sample_rate: rate,
                    channels,
                    qstep,
                    mid_side: true,
                },
            )?;
            container::encode_packet_checked(
                &Packet {
                    kind: PacketKind::AudioFrame,
                    flags: 0,
                    stream_id: 2,
                    pts: frame as u64 * 1_000_000 / u64::from(rate),
                    duration: (n as u64 * 1_000_000 / u64::from(rate)) as u32,
                    payload,
                },
                &mut bytes,
            )?;
            frame += n;
        }
        epochs.push((epoch_id, pts, (end_pts - pts) as u32, bytes));
        start = end;
        epoch_id += 1;
    }
    Ok(container::build_file_checked(
        vec![StreamDesc {
            id: 2,
            kind: StreamKind::Audio,
            codec: 1,
            timescale: TIMEBASE,
            param0: rate,
            param1: u32::from(channels),
            flags: 0,
            meta0: 0,
        }],
        epochs,
    )?)
}

fn unique_stream(front: &container::Front, kind: StreamKind) -> Result<Option<&StreamDesc>> {
    let mut matches = front.streams.iter().filter(|s| s.kind == kind);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(CliError::message(format!(
            "container has multiple {kind:?} streams; this CLI output path requires an explicit single stream"
        )));
    }
    Ok(first)
}

pub fn decode(bytes: &[u8]) -> Result<DecodedMedia> {
    let (_, front, _) = container::parse_file_prefix(bytes)?;
    let video_desc = unique_stream(&front, StreamKind::Video)?.cloned();
    let audio_desc = unique_stream(&front, StreamKind::Audio)?.cloned();
    let mut decoder = ContainerStreamDecoder::new();
    let mut video_frames = Vec::new();
    let mut audio_samples = Vec::new();
    for chunk in bytes.chunks(64 * 1024) {
        decoder.push_each(chunk, |output| {
            match output {
                DecodedOutput::Video {
                    stream_id, frame, ..
                } if video_desc.as_ref().is_some_and(|s| s.id == stream_id) => {
                    video_frames.push(
                        Frame420::from_planes(
                            frame.width,
                            frame.height,
                            frame.y().to_vec(),
                            frame.u().to_vec(),
                            frame.v().to_vec(),
                        )
                        .expect("decoded frame already validated"),
                    );
                }
                DecodedOutput::Audio { stream_id, pcm, .. }
                    if audio_desc.as_ref().is_some_and(|s| s.id == stream_id) =>
                {
                    audio_samples.extend(pcm)
                }
                _ => {}
            }
            Ok::<_, Infallible>(())
        })?;
    }
    decoder.finish_input()?;
    let video = video_desc.map(|s| {
        let (fps_n, fps_d) = y4m::fps_from_flags(s.flags);
        Y4m {
            w: s.param0,
            h: s.param1,
            fps_n,
            fps_d,
            meta0: s.meta0,
            frames: video_frames,
        }
    });
    let audio = audio_desc.map(|s| AudioData {
        rate: s.param0,
        channels: s.param1 as u8,
        samples: audio_samples,
    });
    Ok(DecodedMedia { video, audio })
}

pub fn verify(bytes: &[u8]) -> Result<VerifySummary> {
    container::parse_file_prefix(bytes)?;
    let mut decoder = ContainerStreamDecoder::new();
    let mut summary = VerifySummary::default();
    for chunk in bytes.chunks(64 * 1024) {
        decoder.push_each(chunk, |output| {
            match output {
                DecodedOutput::EpochStart { .. } => summary.epochs += 1,
                DecodedOutput::Video { stream_id, .. } => {
                    *summary.video_frames.entry(stream_id).or_default() += 1
                }
                DecodedOutput::Audio { stream_id, .. } => {
                    *summary.audio_packets.entry(stream_id).or_default() += 1
                }
                DecodedOutput::Metadata { .. } => {}
            }
            Ok::<_, Infallible>(())
        })?;
    }
    decoder.finish_input()?;
    Ok(summary)
}
