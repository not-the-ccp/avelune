#![forbid(unsafe_code)]
use avelune_audio_v1::EncodeOptions as AOptions;
use avelune_container_v1::*;
use avelune_video_v1::{EncodeOptions as VOptions, Frame420};
use std::{
    collections::VecDeque,
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode},
    time::Instant,
};

use clap::{Args as ClapArgs, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PresetArg {
    Fast,
    Balanced,
    Quality,
}
impl PresetArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Quality => "quality",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum BackendArg {
    Auto,
    Prod,
    Reference,
}
impl BackendArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Prod => "prod",
            Self::Reference => "reference",
        }
    }
    const fn use_prod(self) -> bool {
        matches!(self, Self::Auto | Self::Prod)
    }
}

fn encode_packet_for_backend(
    packet: Packet,
    out: &mut Vec<u8>,
    backend: BackendArg,
) -> Result<(), String> {
    if backend.use_prod() {
        let kind = match packet.kind {
            PacketKind::EpochStart => avelune_prod::container::v1::PacketKind::EpochStart,
            PacketKind::VideoFrame => avelune_prod::container::v1::PacketKind::VideoFrame,
            PacketKind::AudioFrame => avelune_prod::container::v1::PacketKind::AudioFrame,
            PacketKind::Metadata => avelune_prod::container::v1::PacketKind::Metadata,
        };
        avelune_prod::container::v1::encode_packet_checked(
            &avelune_prod::container::v1::Packet {
                kind,
                flags: packet.flags,
                stream_id: packet.stream_id,
                pts: packet.pts,
                duration: packet.duration,
                payload: packet.payload,
            },
            out,
        )
        .map_err(|e| format!("production packet encode: {e:?}"))?;
    } else {
        encode_packet(&packet, out);
    }
    Ok(())
}

fn build_file_for_backend(
    streams: Vec<StreamDesc>,
    epochs: Vec<(u32, u64, u32, Vec<u8>)>,
    backend: BackendArg,
) -> Result<Vec<u8>, String> {
    if backend.use_prod() {
        let streams = streams
            .into_iter()
            .map(|s| avelune_prod::container::v1::StreamDesc {
                id: s.id,
                kind: match s.kind {
                    StreamKind::Video => avelune_prod::container::v1::StreamKind::Video,
                    StreamKind::Audio => avelune_prod::container::v1::StreamKind::Audio,
                },
                codec: s.codec,
                timescale: s.timescale,
                param0: s.param0,
                param1: s.param1,
                flags: s.flags,
                meta0: s.meta0,
            })
            .collect();
        avelune_prod::container::v1::build_file_checked(streams, epochs)
            .map_err(|e| format!("production container build: {e}"))
    } else {
        avelune_container_v1::build_file_checked(streams, epochs)
            .map_err(|e| format!("reference container build: {e:?}"))
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "avelune",
    version,
    about = "Tools for the experimental Avelune audiovisual codec family",
    long_about = "Encode, decode, inspect, validate, and experiment with Draft Generation 1 Avelune media.\n\nProduction and reference backends remain separately selectable for conformance and diagnostics.",
    arg_required_else_help = true
)]
struct Cli {
    /// Codec backend for commands that encode/decode media. `auto` currently selects production.
    #[arg(long, value_enum, default_value_t = BackendArg::Auto, global = true)]
    backend: BackendArg,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Encode an ordinary media file through FFmpeg into an Avelune container.
    Encode(MediaEncodeArgs),
    /// Decode an Avelune container through FFmpeg into an ordinary media file.
    Decode(FilePairArgs),
    /// Convenience playback path. Not a low-latency native player.
    Play(FileInputArgs),
    /// Show container, stream, and epoch metadata.
    Inspect(FileInputArgs),
    /// Parse and deeply decode the stream to verify integrity.
    Verify(FileInputArgs),
    /// Print one concise line per coded video frame.
    Frames(FileInputArgs),
    /// Measure selected-backend video decode throughput.
    Benchmark(FileInputArgs),
    /// Rebuild the front index from valid packets.
    Reindex(FilePairArgs),
    /// Best-effort packet resynchronization and front-index rebuild.
    Repair(FilePairArgs),
    /// Generate deterministic conformance vectors and expected outputs.
    Conformance(ConformanceArgs),
    /// Deterministic malformed-container and raw-codec mutation smoke test.
    FuzzSmoke(FuzzArgs),
    /// Generate shell completion scripts to stdout.
    Completions(CompletionsArgs),
    /// Raw workflows that avoid the ordinary-media FFmpeg bridge.
    Raw {
        #[command(subcommand)]
        command: RawCommand,
    },
}

#[derive(Debug, Subcommand)]
enum RawCommand {
    /// Encode 8-bit 4:2:0 Y4M into an Avelune container.
    EncodeY4m(RawVideoEncodeArgs),
    /// Decode video from an Avelune container to Y4M.
    DecodeY4m(FilePairArgs),
    /// Encode an ordinary audio file through FFmpeg into audio-only Avelune.
    EncodeAudio(RawAudioEncodeArgs),
    /// Decode Avelune audio to interleaved signed 16-bit little-endian PCM.
    DecodeAudio(FilePairArgs),
}

#[derive(Debug, ClapArgs)]
struct FileInputArgs {
    /// Input path, or '-' where the command supports stdin.
    input: String,
}

#[derive(Debug, ClapArgs)]
struct FilePairArgs {
    /// Input path, or '-' where supported.
    input: String,
    /// Output path, or '-' where supported.
    output: String,
}

#[derive(Debug, ClapArgs)]
struct MediaEncodeArgs {
    /// Input media readable by FFmpeg.
    input: String,
    /// Output Avelune container.
    output: String,
    /// Encode only the first N seconds.
    #[arg(long)]
    seconds: Option<String>,
    /// Resize video before encoding, for example 1280x720.
    #[arg(long)]
    size: Option<String>,
    /// Video quantizer step. This is experimental and is not a CRF/bitrate target.
    #[arg(long, default_value_t = 96)]
    video_q: u16,
    /// Audio quantizer step. Defaults to mathematically lossless q=1; lossy ALA1 is experimental and not perceptually tuned.
    #[arg(long, default_value_t = 1)]
    audio_q: u16,
    /// Maximum epoch length in video frames. Scene cuts may start an epoch earlier.
    #[arg(long)]
    epoch: Option<usize>,
    /// Encoder search preset; production and reference backends may use different policies.
    #[arg(long, value_enum, default_value_t = PresetArg::Balanced)]
    preset: PresetArg,
}

#[derive(Debug, ClapArgs)]
struct RawVideoEncodeArgs {
    input: String,
    output: String,
    /// Video quantizer step; 1 is mathematically lossless.
    #[arg(short = 'q', long, default_value_t = 96)]
    q: u16,
    /// Maximum epoch length in video frames.
    #[arg(long)]
    epoch: Option<usize>,
    #[arg(long, value_enum, default_value_t = PresetArg::Balanced)]
    preset: PresetArg,
}

#[derive(Debug, ClapArgs)]
struct RawAudioEncodeArgs {
    input: String,
    output: String,
    /// Audio quantizer step; defaults to mathematically lossless q=1. Lossy ALA1 is experimental and not perceptually tuned.
    #[arg(short = 'q', long, default_value_t = 1)]
    q: u16,
    /// Encode only the first N seconds.
    #[arg(long)]
    seconds: Option<String>,
    /// Audio-only epoch length in seconds.
    #[arg(long, default_value_t = 2)]
    epoch_seconds: usize,
}

#[derive(Debug, ClapArgs)]
struct ConformanceArgs {
    /// Directory to create or replace with generated vectors.
    directory: String,
}

#[derive(Debug, ClapArgs)]
struct FuzzArgs {
    /// Valid seed Avelune container.
    input: String,
    /// Number of container mutations and raw-codec mutations each.
    #[arg(default_value_t = 1000)]
    iterations: usize,
}

#[derive(Debug, ClapArgs)]
struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    shell: Shell,
}

fn argv(command: &str, positional: &[&str], options: &[(&str, Option<String>)]) -> Vec<String> {
    let mut out = vec!["avelune".into(), command.into()];
    out.extend(positional.iter().map(|s| (*s).to_owned()));
    for (name, value) in options {
        if let Some(value) = value {
            out.push((*name).into());
            out.push(value.clone());
        }
    }
    out
}

fn color_error(message: &str) {
    let color = io::stderr().is_terminal() && env::var_os("NO_COLOR").is_none();
    if color {
        eprintln!("\x1b[1;31merror:\x1b[0m {message}");
    } else {
        eprintln!("error: {message}");
    }
}

fn read_all(p: &str) -> Result<Vec<u8>, String> {
    if p == "-" {
        let mut v = Vec::new();
        io::stdin().read_to_end(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    } else {
        fs::read(p).map_err(|e| e.to_string())
    }
}
fn write_all(p: &str, b: &[u8]) -> Result<(), String> {
    if p == "-" {
        io::stdout().write_all(b).map_err(|e| e.to_string())
    } else {
        fs::write(p, b).map_err(|e| e.to_string())
    }
}
fn arg_value(a: &[String], name: &str) -> Option<String> {
    a.windows(2).find(|x| x[0] == name).map(|x| x[1].clone())
}
fn arg_u16(a: &[String], name: &str, default: u16) -> Result<u16, String> {
    arg_value(a, name).map_or(Ok(default), |s| {
        s.parse().map_err(|_| format!("bad {name}"))
    })
}
fn arg_usize(a: &[String], name: &str, default: usize) -> Result<usize, String> {
    arg_value(a, name).map_or(Ok(default), |s| {
        s.parse().map_err(|_| format!("bad {name}"))
    })
}
fn arg_backend(a: &[String]) -> Result<BackendArg, String> {
    match arg_value(a, "--backend").as_deref().unwrap_or("auto") {
        "auto" => Ok(BackendArg::Auto),
        "prod" => Ok(BackendArg::Prod),
        "reference" => Ok(BackendArg::Reference),
        _ => Err("bad --backend".into()),
    }
}
fn add_backend(a: &mut Vec<String>, backend: BackendArg) {
    a.push("--backend".into());
    a.push(backend.as_str().into());
}

#[derive(Clone)]
struct Y4m {
    w: u32,
    h: u32,
    fps_n: u32,
    fps_d: u32,
    meta0: u32,
    frames: Vec<Frame420>,
}
fn to_prod_frame(f: &Frame420) -> Result<avelune_prod::video::v1::Frame420, String> {
    avelune_prod::video::v1::Frame420::from_planes(
        f.width,
        f.height,
        f.y.clone(),
        f.u.clone(),
        f.v.clone(),
    )
    .map_err(|e| format!("production frame conversion: {e:?}"))
}
fn parse_y4m(b: &[u8]) -> Result<Y4m, String> {
    let nl = b
        .iter()
        .position(|&x| x == b'\n')
        .ok_or("missing Y4M header")?;
    let h = std::str::from_utf8(&b[..nl]).map_err(|_| "bad Y4M UTF-8")?;
    if !h.starts_with("YUV4MPEG2 ") {
        return Err("not YUV4MPEG2".into());
    }
    let (mut w, mut ht, mut fn_, mut fd) = (None, None, 30u32, 1u32);
    let mut chroma = "420";
    let mut meta = VideoMeta::default();
    for t in h.split_whitespace().skip(1) {
        if let Some(x) = t.strip_prefix('W') {
            w = Some(x.parse().map_err(|_| "bad width")?)
        } else if let Some(x) = t.strip_prefix('H') {
            ht = Some(x.parse().map_err(|_| "bad height")?)
        } else if let Some(x) = t.strip_prefix('F') {
            let mut q = x.split(':');
            fn_ = q.next().ok_or("bad fps")?.parse().map_err(|_| "bad fps")?;
            fd = q.next().unwrap_or("1").parse().map_err(|_| "bad fps")?;
        } else if let Some(x) = t.strip_prefix('C') {
            chroma = x;
            meta.chroma = if x.starts_with("420mpeg2") {
                ChromaLocation::Left
            } else if x.starts_with("420jpeg") || x.starts_with("420paldv") {
                ChromaLocation::Center
            } else {
                ChromaLocation::Unspecified
            };
        } else if let Some(x) = t.strip_prefix("XCOLORRANGE=") {
            meta.full_range = x.eq_ignore_ascii_case("FULL");
        }
    }
    if !chroma.starts_with("420") {
        return Err(format!(
            "v1 baseline requires 8-bit 4:2:0 Y4M, got C{chroma}"
        ));
    }
    let (w, ht) = (w.ok_or("no width")?, ht.ok_or("no height")?);
    if w % 2 != 0 || ht % 2 != 0 {
        return Err("4:2:0 dimensions must be even".into());
    }
    let y = w as usize * ht as usize;
    let c = y / 4;
    let mut p = nl + 1;
    let mut frames = Vec::new();
    while p < b.len() {
        if !b[p..].starts_with(b"FRAME") {
            return Err(format!("expected FRAME at {p}"));
        }
        let n = b[p..]
            .iter()
            .position(|&x| x == b'\n')
            .ok_or("truncated FRAME header")?;
        p += n + 1;
        if b.len() < p + y + 2 * c {
            return Err("truncated frame".into());
        }
        frames.push(Frame420 {
            width: w,
            height: ht,
            y: b[p..p + y].to_vec(),
            u: b[p + y..p + y + c].to_vec(),
            v: b[p + y + c..p + y + 2 * c].to_vec(),
        });
        p += y + 2 * c;
    }
    Ok(Y4m {
        w,
        h: ht,
        fps_n: fn_,
        fps_d: fd,
        meta0: meta.pack(),
        frames,
    })
}
fn emit_y4m(y: &Y4m) -> Vec<u8> {
    let meta = VideoMeta::unpack(y.meta0).unwrap_or_default();
    let chroma = match meta.chroma {
        ChromaLocation::Left => "420mpeg2",
        _ => "420jpeg",
    };
    let range = if meta.full_range { "FULL" } else { "LIMITED" };
    let mut o = format!(
        "YUV4MPEG2 W{} H{} F{}:{} Ip A1:1 C{} XCOLORRANGE={}\n",
        y.w, y.h, y.fps_n, y.fps_d, chroma, range
    )
    .into_bytes();
    for f in &y.frames {
        o.extend(b"FRAME\n");
        o.extend(&f.y);
        o.extend(&f.u);
        o.extend(&f.v)
    }
    o
}
fn fps_flags(n: u32, d: u32) -> u32 {
    ((n.min(65535)) << 16) | d.min(65535)
}
fn fps_from_flags(v: u32) -> (u32, u32) {
    ((v >> 16).max(1), (v & 65535).max(1))
}
fn preset(name: &str, q: u16) -> VOptions {
    match name {
        "fast" => VOptions {
            qstep: q,
            motion_radius: 2,
            max_refs: 1,
            allow_palette: true,
        },
        "quality" => VOptions {
            qstep: q,
            motion_radius: 5,
            max_refs: 4,
            allow_palette: true,
        },
        _ => VOptions {
            qstep: q,
            motion_radius: 4,
            max_refs: 1,
            allow_palette: true,
        },
    }
}
fn prod_voptions(v: VOptions) -> avelune_prod::video::v1::EncodeOptions {
    avelune_prod::video::v1::EncodeOptions {
        qstep: v.qstep,
        motion_radius: v.motion_radius,
        max_refs: v.max_refs,
        preset: if v.max_refs >= 4 || v.motion_radius >= 5 {
            avelune_prod::video::v1::EncoderPreset::Quality
        } else if v.motion_radius <= 2 {
            avelune_prod::video::v1::EncoderPreset::Fast
        } else {
            avelune_prod::video::v1::EncoderPreset::Balanced
        },
        allow_palette: v.allow_palette,
    }
}
fn prod_aoptions(v: AOptions) -> avelune_prod::audio::v1::EncodeOptions {
    avelune_prod::audio::v1::EncodeOptions {
        sample_rate: v.sample_rate,
        channels: v.channels,
        qstep: v.qstep,
        mid_side: v.mid_side,
    }
}

fn scene_cut(a: &Frame420, b: &Frame420) -> bool {
    let step = (a.y.len() / 4096).max(1);
    let mut s = 0u64;
    let mut n = 0u64;
    for i in (0..a.y.len()).step_by(step) {
        s += u64::from((i32::from(a.y[i]) - i32::from(b.y[i])).unsigned_abs());
        n += 1
    }
    s / n.max(1) > 48
}

fn encode_av(
    y: &Y4m,
    audio: Option<(&[i16], u8, u32)>,
    outp: &str,
    quantizers: (u16, u16),
    epoch_frames: usize,
    preset_name: &str,
    backend: BackendArg,
) -> Result<(), String> {
    if epoch_frames == 0 {
        return Err("epoch length must be >0".into());
    }
    let (qv, qa) = quantizers;
    let vopt = preset(preset_name, qv);
    let frame_us = (1_000_000u64 * u64::from(y.fps_d)) / u64::from(y.fps_n.max(1));
    let mut epoch_starts = vec![0usize];
    for i in 1..y.frames.len() {
        if (i % epoch_frames == 0 || scene_cut(&y.frames[i - 1], &y.frames[i]))
            && *epoch_starts.last().unwrap() != i
        {
            epoch_starts.push(i)
        }
    }
    epoch_starts.push(y.frames.len());
    let mut epochs = Vec::new();
    let start_time = Instant::now();
    for ei in 0..epoch_starts.len() - 1 {
        let s = epoch_starts[ei];
        let e = epoch_starts[ei + 1];
        if s == e {
            continue;
        }
        let pts = s as u64 * frame_us;
        let end_pts = e as u64 * frame_us;
        let mut packets = Vec::<Packet>::new();
        packets.push(Packet {
            kind: PacketKind::EpochStart,
            flags: 0,
            stream_id: 0,
            pts,
            duration: (end_pts - pts) as u32,
            payload: (ei as u32).to_le_bytes().to_vec(),
        });
        if backend.use_prod() {
            let mut encoder = avelune_prod::video::v1::VideoEncoder::new(prod_voptions(vopt));
            for i in s..e {
                let source = to_prod_frame(&y.frames[i])?;
                let enc = encoder
                    .encode_shared(i as u64, &source)
                    .map_err(|x| format!("production video encode frame {i}: {x:?}"))?;
                packets.push(Packet {
                    kind: PacketKind::VideoFrame,
                    flags: 0,
                    stream_id: 1,
                    pts: i as u64 * frame_us,
                    duration: frame_us as u32,
                    payload: enc.packet,
                });
            }
        } else {
            let mut hist: VecDeque<(u64, Frame420)> = VecDeque::new();
            for i in s..e {
                let refs: Vec<(u64, &Frame420)> = hist
                    .iter()
                    .rev()
                    .take(vopt.max_refs as usize)
                    .map(|(id, f)| (*id, f))
                    .collect();
                let enc = avelune_video_v1::encode(i as u64, &y.frames[i], &refs, vopt)
                    .map_err(|x| format!("reference video encode frame {i}: {x:?}"))?;
                packets.push(Packet {
                    kind: PacketKind::VideoFrame,
                    flags: 0,
                    stream_id: 1,
                    pts: i as u64 * frame_us,
                    duration: frame_us as u32,
                    payload: enc.packet,
                });
                hist.push_back((i as u64, enc.reconstructed));
                while hist.len() > 4 {
                    hist.pop_front();
                }
            }
        }
        if let Some((samples, ch, rate)) = audio {
            let chn = ch as usize;
            let total_frames = samples.len() / chn;
            let start_af = ((pts * u64::from(rate)) / 1_000_000) as usize;
            let end_af =
                ((end_pts * u64::from(rate)) / 1_000_000).min(total_frames as u64) as usize;
            let mut af = start_af;
            while af < end_af {
                let n = (end_af - af).min(960);
                let slice = &samples[af * chn..(af + n) * chn];
                let aopts = AOptions {
                    sample_rate: rate,
                    channels: ch,
                    qstep: qa,
                    mid_side: true,
                };
                let coded = if backend.use_prod() {
                    avelune_prod::audio::v1::encode(slice, prod_aoptions(aopts))
                        .map_err(|x| format!("production audio encode: {x:?}"))?
                } else {
                    avelune_audio_v1::encode(slice, aopts)
                        .map_err(|x| format!("reference audio encode: {x:?}"))?
                };
                let ap = (af as u64 * 1_000_000) / u64::from(rate);
                let dur = (n as u64 * 1_000_000 / u64::from(rate)) as u32;
                packets.push(Packet {
                    kind: PacketKind::AudioFrame,
                    flags: 0,
                    stream_id: 2,
                    pts: ap,
                    duration: dur,
                    payload: coded,
                });
                af += n;
            }
        }
        // EpochStart is a structural boundary, not merely another timestamped packet.
        // Audio sample timestamps can quantize a few microseconds below a video-derived
        // epoch PTS, so timestamp-only sorting can otherwise move audio before EpochStart
        // and make the front-indexed byte range non-conforming.
        packets.sort_by_key(|p| {
            (
                if p.kind == PacketKind::EpochStart {
                    0u8
                } else {
                    1u8
                },
                p.pts,
                match p.kind {
                    PacketKind::VideoFrame => 0,
                    PacketKind::AudioFrame => 1,
                    PacketKind::Metadata => 2,
                    PacketKind::EpochStart => 0,
                },
            )
        });
        let mut bytes = Vec::new();
        for p in packets {
            encode_packet_for_backend(p, &mut bytes, backend)?;
        }
        epochs.push((ei as u32, pts, (end_pts - pts) as u32, bytes));
    }
    let mut streams = vec![StreamDesc {
        id: 1,
        kind: StreamKind::Video,
        codec: 1,
        timescale: TIMEBASE,
        param0: y.w,
        param1: y.h,
        flags: fps_flags(y.fps_n, y.fps_d),
        meta0: y.meta0,
    }];
    if let Some((_, ch, rate)) = audio {
        streams.push(StreamDesc {
            id: 2,
            kind: StreamKind::Audio,
            codec: 1,
            timescale: TIMEBASE,
            param0: rate,
            param1: u32::from(ch),
            flags: 0,
            meta0: 0,
        })
    }
    let file = build_file_for_backend(streams, epochs, backend)?;
    write_all(outp, &file)?;
    eprintln!(
        "encoded {} frames -> {} bytes in {:.3}s ({} epochs, qv={}, qa={}, preset={}, backend={})",
        y.frames.len(),
        file.len(),
        start_time.elapsed().as_secs_f64(),
        epoch_starts.len() - 1,
        qv,
        qa,
        preset_name,
        if backend.use_prod() {
            "prod"
        } else {
            "reference"
        }
    );
    Ok(())
}

fn encode_y4m_cmd(a: &[String]) -> Result<(), String> {
    let inp = a.get(2).ok_or("missing input")?;
    let out = a.get(3).ok_or("missing output")?;
    let y = parse_y4m(&read_all(inp)?)?;
    let q = arg_u16(a, "--q", 96)?;
    let ep = arg_usize(
        a,
        "--epoch",
        ((y.fps_n / y.fps_d.max(1)) * 2).max(1) as usize,
    )?;
    let pre = arg_value(a, "--preset").unwrap_or_else(|| "balanced".into());
    encode_av(&y, None, out, (q, 1), ep, &pre, arg_backend(a)?)
}
fn temp(name: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "avelune-{}-{}-{name}",
        std::process::id(),
        std::thread::current().name().unwrap_or("main")
    ))
}
fn run_checked(mut c: ProcessCommand) -> Result<(), String> {
    let s = c.status().map_err(|e| e.to_string())?;
    if s.success() {
        Ok(())
    } else {
        Err(format!("command failed with {s}"))
    }
}
fn has_audio(inp: &str) -> bool {
    ProcessCommand::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
            inp,
        ])
        .output()
        .ok()
        .is_some_and(|o| o.status.success() && !o.stdout.is_empty())
}
fn probe_video_meta(inp: &str) -> u32 {
    let out = ProcessCommand::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=color_range,color_space,color_transfer,color_primaries,chroma_location",
            "-of",
            "default=nw=1",
            inp,
        ])
        .output();
    let Ok(out) = out else { return 0 };
    if !out.status.success() {
        return 0;
    }
    let txt = String::from_utf8_lossy(&out.stdout);
    let mut m = VideoMeta::default();
    for line in txt.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "color_range" => m.full_range = v == "pc" || v == "jpeg",
            "color_space" => {
                m.matrix = match v {
                    "bt709" => ColorMatrix::Bt709,
                    "bt2020nc" => ColorMatrix::Bt2020Ncl,
                    "smpte170m" | "bt470bg" => ColorMatrix::Bt601,
                    _ => ColorMatrix::Unspecified,
                }
            }
            "color_transfer" => {
                m.transfer = match v {
                    "bt709" => ColorTransfer::Bt709,
                    "iec61966-2-1" => ColorTransfer::Srgb,
                    "smpte2084" => ColorTransfer::Pq,
                    "arib-std-b67" => ColorTransfer::Hlg,
                    _ => ColorTransfer::Unspecified,
                }
            }
            "color_primaries" => {
                m.primaries = match v {
                    "bt709" => ColorPrimaries::Bt709,
                    "bt2020" => ColorPrimaries::Bt2020,
                    "smpte170m" | "bt470bg" => ColorPrimaries::Bt601,
                    _ => ColorPrimaries::Unspecified,
                }
            }
            "chroma_location" => {
                m.chroma = match v {
                    "left" => ChromaLocation::Left,
                    "center" => ChromaLocation::Center,
                    _ => ChromaLocation::Unspecified,
                }
            }
            _ => {}
        }
    }
    m.pack()
}
fn encode_media_cmd(a: &[String]) -> Result<(), String> {
    let inp = a.get(2).ok_or("missing input")?;
    let out = a.get(3).ok_or("missing output")?;
    let ypath = temp("video.y4m");
    let apath = temp("audio.s16");
    let mut vf = ProcessCommand::new("ffmpeg");
    vf.args(["-y", "-loglevel", "error"]);
    if let Some(sec) = arg_value(a, "--seconds") {
        vf.args(["-t", &sec]);
    }
    vf.args(["-i", inp]);
    if let Some(sz) = arg_value(a, "--size") {
        vf.args([
            "-vf",
            &format!("scale={}:flags=lanczos", sz.replace('x', ":")),
        ]);
    }
    vf.args([
        "-pix_fmt",
        "yuv420p",
        "-f",
        "yuv4mpegpipe",
        ypath.to_str().unwrap(),
    ]);
    run_checked(vf)?;
    let mut y = parse_y4m(&fs::read(&ypath).map_err(|e| e.to_string())?)?;
    let probed = probe_video_meta(inp);
    if probed != 0 {
        let mut pm = VideoMeta::unpack(probed).unwrap_or_default();
        let ym = VideoMeta::unpack(y.meta0).unwrap_or_default();
        if pm.chroma == ChromaLocation::Unspecified {
            pm.chroma = ym.chroma
        }
        y.meta0 = pm.pack();
    }
    let audio_present = has_audio(inp);
    let mut audio_buf = Vec::<i16>::new();
    if audio_present {
        let mut af = ProcessCommand::new("ffmpeg");
        af.args(["-y", "-loglevel", "error"]);
        if let Some(sec) = arg_value(a, "--seconds") {
            af.args(["-t", &sec]);
        }
        af.args([
            "-i",
            inp,
            "-vn",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-f",
            "s16le",
            apath.to_str().unwrap(),
        ]);
        run_checked(af)?;
        let b = fs::read(&apath).map_err(|e| e.to_string())?;
        audio_buf = b
            .chunks_exact(2)
            .map(|x| i16::from_le_bytes([x[0], x[1]]))
            .collect();
    }
    let qv = arg_u16(a, "--video-q", 96)?;
    let qa = arg_u16(a, "--audio-q", 1)?;
    let ep = arg_usize(
        a,
        "--epoch",
        ((y.fps_n / y.fps_d.max(1)) * 2).max(1) as usize,
    )?;
    let pre = arg_value(a, "--preset").unwrap_or_else(|| "balanced".into());
    let r = encode_av(
        &y,
        audio_present.then_some((audio_buf.as_slice(), 2, 48_000)),
        out,
        (qv, qa),
        ep,
        &pre,
        arg_backend(a)?,
    );
    let _ = fs::remove_file(ypath);
    let _ = fs::remove_file(apath);
    r
}

fn encode_audio_cmd(a: &[String]) -> Result<(), String> {
    let inp = a.get(2).ok_or("missing input")?;
    let out = a.get(3).ok_or("missing output")?;
    let apath = temp("audio-only.s16");
    let mut af = ProcessCommand::new("ffmpeg");
    af.args(["-y", "-loglevel", "error"]);
    if let Some(sec) = arg_value(a, "--seconds") {
        af.args(["-t", &sec]);
    }
    af.args([
        "-i",
        inp,
        "-vn",
        "-ar",
        "48000",
        "-ac",
        "2",
        "-f",
        "s16le",
        apath.to_str().unwrap(),
    ]);
    run_checked(af)?;
    let raw = fs::read(&apath).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&apath);
    let samples: Vec<i16> = raw
        .chunks_exact(2)
        .map(|x| i16::from_le_bytes([x[0], x[1]]))
        .collect();
    let q = arg_u16(a, "--q", 1)?;
    let epoch_s = arg_usize(a, "--epoch-seconds", 2)?.max(1);
    let backend = arg_backend(a)?;
    let rate = 48_000u32;
    let ch = 2u8;
    let total = samples.len() / 2;
    let epoch_frames = epoch_s * rate as usize;
    let mut epochs = Vec::new();
    let mut start = 0usize;
    let mut eid = 0u32;
    while start < total {
        let end = (start + epoch_frames).min(total);
        let pts = start as u64 * 1_000_000 / u64::from(rate);
        let end_pts = end as u64 * 1_000_000 / u64::from(rate);
        let mut bytes = Vec::new();
        encode_packet_for_backend(
            Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts,
                duration: (end_pts - pts) as u32,
                payload: eid.to_le_bytes().to_vec(),
            },
            &mut bytes,
            backend,
        )?;
        let mut f = start;
        while f < end {
            let n = (end - f).min(960);
            let aopts = AOptions {
                sample_rate: rate,
                channels: ch,
                qstep: q,
                mid_side: true,
            };
            let coded = if backend.use_prod() {
                avelune_prod::audio::v1::encode(&samples[f * 2..(f + n) * 2], prod_aoptions(aopts))
                    .map_err(|e| format!("production audio encode: {e:?}"))?
            } else {
                avelune_audio_v1::encode(&samples[f * 2..(f + n) * 2], aopts)
                    .map_err(|e| format!("reference audio encode: {e:?}"))?
            };
            let p = f as u64 * 1_000_000 / u64::from(rate);
            let dur = (n as u64 * 1_000_000 / u64::from(rate)) as u32;
            encode_packet_for_backend(
                Packet {
                    kind: PacketKind::AudioFrame,
                    flags: 0,
                    stream_id: 2,
                    pts: p,
                    duration: dur,
                    payload: coded,
                },
                &mut bytes,
                backend,
            )?;
            f += n;
        }
        epochs.push((eid, pts, (end_pts - pts) as u32, bytes));
        start = end;
        eid += 1;
    }
    let streams = vec![StreamDesc {
        id: 2,
        kind: StreamKind::Audio,
        codec: 1,
        timescale: TIMEBASE,
        param0: rate,
        param1: u32::from(ch),
        flags: 0,
        meta0: 0,
    }];
    let file = build_file_for_backend(streams, epochs, backend)?;
    write_all(out, &file)?;
    eprintln!(
        "encoded audio sample_frames={} q={} -> {} bytes",
        total,
        q,
        file.len()
    );
    Ok(())
}

fn all_packets(b: &[u8]) -> Result<(Front, Vec<Packet>, usize), String> {
    let (_, front, n) = parse_file_prefix(b).map_err(|e| format!("container prefix: {e:?}"))?;
    let mut p = n;
    let mut v = Vec::new();
    while p < b.len() {
        let (pkt, k) = decode_packet(&b[p..], DEFAULT_MAX_PACKET)
            .map_err(|e| format!("packet at {p}: {e:?}"))?;
        v.push(pkt);
        p += k
    }
    Ok((front, v, n))
}
fn decode_video_frames(b: &[u8], backend: BackendArg) -> Result<(Y4m, f64), String> {
    if backend.use_prod() {
        let (_, front, prefix) = avelune_prod::container::v1::parse_file_prefix(b)
            .map_err(|e| format!("production container prefix: {e:?}"))?;
        let sd = front
            .streams
            .iter()
            .find(|s| s.kind == avelune_prod::container::v1::StreamKind::Video)
            .ok_or("no video")?;
        let (n, d) = fps_from_flags(sd.flags);
        let mut decoder = avelune_prod::video::v1::VideoDecoder::new();
        let parser = avelune_prod::container::v1::SliceParser::default();
        let mut frames = Vec::new();
        let mut pos = prefix;
        let t = Instant::now();
        while pos < b.len() {
            let (pkt, used) = parser
                .packet(&b[pos..])
                .map_err(|e| format!("production packet at {pos}: {e:?}"))?;
            match pkt.kind {
                avelune_prod::container::v1::PacketKind::EpochStart => decoder.reset_epoch(),
                avelune_prod::container::v1::PacketKind::VideoFrame => {
                    let (_, f, _) = decoder
                        .decode_shared(pkt.payload)
                        .map_err(|e| format!("production video decode: {e:?}"))?;
                    frames.push(Frame420 {
                        width: f.width,
                        height: f.height,
                        y: f.y().to_vec(),
                        u: f.u().to_vec(),
                        v: f.v().to_vec(),
                    });
                }
                _ => {}
            }
            pos += used;
        }
        let secs = t.elapsed().as_secs_f64();
        return Ok((
            Y4m {
                w: sd.param0,
                h: sd.param1,
                fps_n: n,
                fps_d: d,
                meta0: sd.meta0,
                frames,
            },
            secs,
        ));
    }

    let (front, pkts, _) = all_packets(b)?;
    let sd = front
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Video)
        .ok_or("no video")?;
    let (n, d) = fps_from_flags(sd.flags);
    let mut hist: VecDeque<(u64, Frame420)> = VecDeque::new();
    let mut frames = Vec::new();
    let t = Instant::now();
    for p in pkts {
        match p.kind {
            PacketKind::EpochStart => hist.clear(),
            PacketKind::VideoFrame => {
                let refs: Vec<(u64, &Frame420)> = hist.iter().map(|(id, f)| (*id, f)).collect();
                let (id, f, _) = avelune_video_v1::decode(&p.payload, &refs)
                    .map_err(|e| format!("reference video decode: {e:?}"))?;
                hist.push_back((id, f.clone()));
                while hist.len() > 4 {
                    hist.pop_front();
                }
                frames.push(f);
            }
            _ => {}
        }
    }
    let secs = t.elapsed().as_secs_f64();
    Ok((
        Y4m {
            w: sd.param0,
            h: sd.param1,
            fps_n: n,
            fps_d: d,
            meta0: sd.meta0,
            frames,
        },
        secs,
    ))
}
fn decode_audio_samples(
    b: &[u8],
    backend: BackendArg,
) -> Result<Option<(u32, u8, Vec<i16>)>, String> {
    if backend.use_prod() {
        let (_, front, prefix) = avelune_prod::container::v1::parse_file_prefix(b)
            .map_err(|e| format!("production container prefix: {e:?}"))?;
        if !front
            .streams
            .iter()
            .any(|s| s.kind == avelune_prod::container::v1::StreamKind::Audio)
        {
            return Ok(None);
        }
        let parser = avelune_prod::container::v1::SliceParser::default();
        let mut decoder = avelune_prod::audio::v1::AudioDecoder::new();
        let mut out = Vec::new();
        let mut rate = 0;
        let mut ch = 0;
        let mut pos = prefix;
        while pos < b.len() {
            let (pkt, used) = parser
                .packet(&b[pos..])
                .map_err(|e| format!("production packet at {pos}: {e:?}"))?;
            if pkt.kind == avelune_prod::container::v1::PacketKind::AudioFrame {
                let (r, c, samples) = decoder
                    .decode(pkt.payload)
                    .map_err(|e| format!("production audio decode: {e:?}"))?;
                if rate == 0 {
                    rate = r;
                    ch = c;
                } else if rate != r || ch != c {
                    return Err("audio format changed mid-stream".into());
                }
                out.extend(samples);
            }
            pos += used;
        }
        return Ok(Some((rate, ch, out)));
    }

    let (front, pkts, _) = all_packets(b)?;
    if !front.streams.iter().any(|s| s.kind == StreamKind::Audio) {
        return Ok(None);
    }
    let mut out = Vec::new();
    let mut rate = 0;
    let mut ch = 0;
    for p in pkts {
        if p.kind == PacketKind::AudioFrame {
            let (r, c, samples) = avelune_audio_v1::decode(&p.payload)
                .map_err(|e| format!("reference audio decode: {e:?}"))?;
            if rate == 0 {
                rate = r;
                ch = c;
            } else if rate != r || ch != c {
                return Err("audio format changed mid-stream".into());
            }
            out.extend(samples);
        }
    }
    Ok(Some((rate, ch, out)))
}
fn decode_y4m_cmd(a: &[String]) -> Result<(), String> {
    let b = read_all(a.get(2).ok_or("missing input")?)?;
    let (y, _) = decode_video_frames(&b, arg_backend(a)?)?;
    write_all(a.get(3).ok_or("missing output")?, &emit_y4m(&y))
}
fn decode_audio_cmd(a: &[String]) -> Result<(), String> {
    let b = read_all(a.get(2).ok_or("missing input")?)?;
    let (_, _, s) = decode_audio_samples(&b, arg_backend(a)?)?.ok_or("no audio")?;
    let mut o = Vec::with_capacity(s.len() * 2);
    for x in s {
        o.extend(x.to_le_bytes())
    }
    write_all(a.get(3).ok_or("missing output")?, &o)
}
fn decode_media_to(inp: &str, out: &str, backend: BackendArg) -> Result<(), String> {
    let b = read_all(inp)?;
    let (y, _) = decode_video_frames(&b, backend)?;
    let yp = temp("decode.y4m");
    fs::write(&yp, emit_y4m(&y)).map_err(|e| e.to_string())?;
    let aud = decode_audio_samples(&b, backend)?;
    let ap = temp("decode.s16");
    if let Some((_, _, ref s)) = aud {
        let mut raw = Vec::with_capacity(s.len() * 2);
        for &x in s {
            raw.extend(x.to_le_bytes())
        }
        fs::write(&ap, raw).map_err(|e| e.to_string())?
    }
    let mut c = ProcessCommand::new("ffmpeg");
    c.args(["-y", "-loglevel", "error", "-i", yp.to_str().unwrap()]);
    if let Some((rate, ch, _)) = aud {
        c.args([
            "-f",
            "s16le",
            "-ar",
            &rate.to_string(),
            "-ac",
            &ch.to_string(),
            "-i",
            ap.to_str().unwrap(),
            "-c:a",
            "aac",
        ]);
    }
    c.args(["-c:v", "libx264", "-pix_fmt", "yuv420p", out]);
    let r = run_checked(c);
    let _ = fs::remove_file(yp);
    let _ = fs::remove_file(ap);
    r
}
fn decode_media_cmd(a: &[String]) -> Result<(), String> {
    decode_media_to(
        a.get(2).ok_or("missing input")?,
        a.get(3).ok_or("missing output")?,
        arg_backend(a)?,
    )
}
fn play_cmd(a: &[String]) -> Result<(), String> {
    let inp = a.get(2).ok_or("missing input")?;
    let tmp = temp("play.mkv");
    decode_media_to(inp, tmp.to_str().unwrap(), arg_backend(a)?)?;
    let r = run_checked({
        let mut c = ProcessCommand::new("ffplay");
        c.args(["-autoexit", "-loglevel", "warning", tmp.to_str().unwrap()]);
        c
    });
    let _ = fs::remove_file(tmp);
    r
}

fn inspect_cmd(a: &[String], deep: bool) -> Result<(), String> {
    let b = read_all(a.get(2).ok_or("missing input")?)?;
    let (h, f, n) = parse_file_prefix(&b).map_err(|e| format!("prefix: {e:?}"))?;
    println!(
        "Avelune container draft-generation=1 header-minor=0 bytes={} prefix={} streams={} epochs={}",
        b.len(),
        n,
        h.stream_count,
        f.epochs.len()
    );
    for s in &f.streams {
        println!(
            "stream {} {:?} codec={} timebase={} p0={} p1={} flags={:#x} meta={:#x}",
            s.id, s.kind, s.codec, s.timescale, s.param0, s.param1, s.flags, s.meta0
        )
    }
    for e in &f.epochs {
        println!(
            "epoch {} pts={} duration={} offset={} len={}",
            e.id, e.pts, e.duration, e.offset, e.len
        );
        if e.offset + e.len > b.len() as u64 {
            return Err("index points outside file".into());
        }
    }
    if deep {
        if f.streams.iter().any(|s| s.kind == StreamKind::Video) {
            let (y, secs) = decode_video_frames(&b, arg_backend(a)?)?;
            println!(
                "verified video frames={} decode={:.3}s",
                y.frames.len(),
                secs
            );
        }
        if let Some((r, c, s)) = decode_audio_samples(&b, arg_backend(a)?)? {
            println!(
                "verified audio rate={} channels={} sample_frames={}",
                r,
                c,
                s.len() / c as usize
            )
        }
    }
    Ok(())
}
fn frames_cmd(a: &[String]) -> Result<(), String> {
    let b = read_all(a.get(2).ok_or("missing input")?)?;
    let (_, pkts, _) = all_packets(&b)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for p in pkts {
        if p.kind == PacketKind::VideoFrame
            && p.payload.len() >= 20
            && p.payload[..4] == avelune_video_v1::CODEC_MAGIC
        {
            let id = u64::from_le_bytes(p.payload[4..12].try_into().unwrap());
            let q = u16::from_le_bytes(p.payload[16..18].try_into().unwrap());
            let rc = p.payload[18];
            if let Err(e) = writeln!(
                out,
                "frame {} pts={} q={} refs={} packet={}",
                id,
                p.pts,
                q,
                rc,
                p.payload.len()
            ) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(e.to_string());
            }
        }
    }
    Ok(())
}
fn benchmark_cmd(a: &[String]) -> Result<(), String> {
    let b = read_all(a.get(2).ok_or("missing input")?)?;
    let t = Instant::now();
    let (y, _) = decode_video_frames(&b, arg_backend(a)?)?;
    let dt = t.elapsed().as_secs_f64();
    let fps = y.frames.len() as f64 / dt.max(1e-9);
    println!(
        "decode: {} frames {}x{} in {:.4}s = {:.2} fps; file={} bytes",
        y.frames.len(),
        y.w,
        y.h,
        dt,
        fps,
        b.len()
    );
    Ok(())
}

fn reindex_bytes(b: &[u8], repair: bool) -> Result<Vec<u8>, String> {
    let (_, front, prefix) = parse_file_prefix(b).map_err(|e| format!("prefix: {e:?}"))?;
    let mut groups = Vec::<(u32, u64, u32, Vec<u8>)>::new();
    let mut p = prefix;
    let mut cur = None::<(u32, u64, u32, Vec<u8>)>;
    while p < b.len() {
        match decode_packet(&b[p..], DEFAULT_MAX_PACKET) {
            Ok((pkt, n)) => {
                if pkt.kind == PacketKind::EpochStart {
                    if let Some(x) = cur.take() {
                        groups.push(x)
                    }
                    let id = if pkt.payload.len() >= 4 {
                        u32::from_le_bytes(pkt.payload[..4].try_into().unwrap())
                    } else {
                        groups.len() as u32
                    };
                    cur = Some((id, pkt.pts, pkt.duration, Vec::new()));
                }
                if cur.is_none() {
                    cur = Some((groups.len() as u32, pkt.pts, 0, Vec::new()))
                }
                cur.as_mut().unwrap().3.extend(&b[p..p + n]);
                p += n
            }
            Err(e) if repair => {
                p += 1;
                while p + 4 <= b.len() && b[p..p + 4] != PACKET_MAGIC {
                    p += 1
                }
                eprintln!("repair: skipped bytes after {e:?}")
            }
            Err(e) => return Err(format!("packet at {p}: {e:?}")),
        }
    }
    if let Some(x) = cur {
        groups.push(x)
    }
    if groups.is_empty() {
        return Err("no recoverable packets".into());
    }
    Ok(build_file(front.streams, groups))
}
fn reindex_cmd(a: &[String], repair: bool) -> Result<(), String> {
    let b = read_all(a.get(2).ok_or("missing input")?)?;
    let out = reindex_bytes(&b, repair)?;
    write_all(a.get(3).ok_or("missing output")?, &out)
}

fn verify_video_with_reference_decoder(b: &[u8]) -> Result<(), String> {
    let (_, pkts, _) = all_packets(b)?;
    let mut prod_hist: VecDeque<(u64, Frame420)> = VecDeque::new();
    let mut ref_hist: VecDeque<(u64, avelune_video_ref_v1::Frame420)> = VecDeque::new();
    for pkt in pkts {
        match pkt.kind {
            PacketKind::EpochStart => {
                prod_hist.clear();
                ref_hist.clear();
            }
            PacketKind::VideoFrame => {
                let pr: Vec<(u64, &Frame420)> = prod_hist.iter().map(|(id, f)| (*id, f)).collect();
                let rr: Vec<(u64, &avelune_video_ref_v1::Frame420)> =
                    ref_hist.iter().map(|(id, f)| (*id, f)).collect();
                let (pid, pf, pdeps) = avelune_video_v1::decode(&pkt.payload, &pr)
                    .map_err(|e| format!("production decode in conformance: {e:?}"))?;
                let (rid, rf, rdeps) = avelune_video_ref_v1::decode(&pkt.payload, &rr)
                    .map_err(|e| format!("reference decode in conformance: {e:?}"))?;
                if pid != rid
                    || pdeps != rdeps
                    || pf.width != rf.width
                    || pf.height != rf.height
                    || pf.y != rf.y
                    || pf.u != rf.u
                    || pf.v != rf.v
                {
                    return Err(format!("reference decoder mismatch at frame {pid}"));
                }
                prod_hist.push_back((pid, pf.clone()));
                ref_hist.push_back((rid, rf));
                while prod_hist.len() > 4 {
                    prod_hist.pop_front();
                }
                while ref_hist.len() > 4 {
                    ref_hist.pop_front();
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn conformance_cmd(a: &[String]) -> Result<(), String> {
    let dir = Path::new(a.get(2).ok_or("missing dir")?);
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let w = 32u32;
    let h = 24u32;
    let mut f = Frame420 {
        width: w,
        height: h,
        y: vec![0; w as usize * h as usize],
        u: vec![0; w as usize * h as usize / 4],
        v: vec![0; w as usize * h as usize / 4],
    };
    for y in 0..h as usize {
        for x in 0..w as usize {
            f.y[y * w as usize + x] = ((x * 9 + y * 13) & 255) as u8
        }
    }
    f.u.fill(96);
    f.v.fill(160);
    let mut shifted = f.clone();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let sx = x.saturating_sub(1);
            shifted.y[y * w as usize + x] = f.y[y * w as usize + sx];
        }
    }
    let mut palette = f.clone();
    for (i, px) in palette.y.iter_mut().enumerate() {
        *px = [16, 64, 160, 235][(i / 8 + i % 8) & 3];
    }
    palette.u.fill(128);
    palette.v.fill(128);
    let y = Y4m {
        w,
        h,
        fps_n: 30,
        fps_d: 1,
        meta0: VideoMeta {
            matrix: ColorMatrix::Bt709,
            transfer: ColorTransfer::Bt709,
            primaries: ColorPrimaries::Bt709,
            full_range: false,
            chroma: ChromaLocation::Left,
        }
        .pack(),
        frames: vec![f.clone(), shifted, f.clone(), palette],
    };
    let avl = dir.join("video-lossless.avl");
    encode_av(
        &y,
        None,
        avl.to_str().unwrap(),
        (1, 1),
        60,
        "quality",
        BackendArg::Prod,
    )?;
    let avl_bytes = fs::read(&avl).map_err(|e| e.to_string())?;
    verify_video_with_reference_decoder(&avl_bytes)?;
    let (decoded, _) = decode_video_frames(&avl_bytes, BackendArg::Prod)?;
    if decoded.frames != y.frames {
        return Err("lossless video conformance mismatch".into());
    }
    fs::write(dir.join("video-lossless-expected.y4m"), emit_y4m(&y)).map_err(|e| e.to_string())?;

    let lossy = dir.join("video-lossy.avl");
    encode_av(
        &y,
        None,
        lossy.to_str().unwrap(),
        (128, 1),
        60,
        "quality",
        BackendArg::Prod,
    )?;
    let lossy_bytes = fs::read(&lossy).map_err(|e| e.to_string())?;
    verify_video_with_reference_decoder(&lossy_bytes)?;
    let (lossy_dec, _) = decode_video_frames(&lossy_bytes, BackendArg::Prod)?;
    fs::write(dir.join("video-lossy-expected.y4m"), emit_y4m(&lossy_dec))
        .map_err(|e| e.to_string())?;

    let s: Vec<i16> = (0..960 * 2)
        .map(|i| (((i * 127) % 60000) - 30000) as i16)
        .collect();
    let ac = avelune_audio_v1::encode(
        &s,
        AOptions {
            qstep: 1,
            ..Default::default()
        },
    )
    .map_err(|e| format!("{e:?}"))?;
    let (_, _, ad) = avelune_audio_v1::decode(&ac).map_err(|e| format!("{e:?}"))?;
    if ad != s {
        return Err("lossless audio conformance mismatch".into());
    }
    fs::write(dir.join("audio-lossless.ala"), ac).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    for x in &s {
        raw.extend(x.to_le_bytes())
    }
    fs::write(dir.join("audio-lossless-expected.s16le"), raw).map_err(|e| e.to_string())?;

    let acl = avelune_audio_v1::encode(
        &s,
        AOptions {
            qstep: 256,
            ..Default::default()
        },
    )
    .map_err(|e| format!("{e:?}"))?;
    let (_, _, adl) = avelune_audio_v1::decode(&acl).map_err(|e| format!("{e:?}"))?;
    fs::write(dir.join("audio-lossy.ala"), acl).map_err(|e| e.to_string())?;
    let mut rawl = Vec::new();
    for x in adl {
        rawl.extend(x.to_le_bytes())
    }
    fs::write(dir.join("audio-lossy-expected.s16le"), rawl).map_err(|e| e.to_string())?;

    let mut corrupt = avl_bytes;
    if let Some(x) = corrupt.last_mut() {
        *x ^= 0x80
    }
    fs::write(dir.join("reject-corrupt.avl"), corrupt).map_err(|e| e.to_string())?;

    let manifest = "Avelune V1 conformance vectors\nvideo-lossless.avl -> video-lossless-expected.y4m (bit-exact source)\nvideo-lossy.avl -> video-lossy-expected.y4m\naudio-lossless.ala -> audio-lossless-expected.s16le\naudio-lossy.ala -> audio-lossy-expected.s16le\nreject-corrupt.avl MUST be rejected\n";
    fs::write(dir.join("README.txt"), manifest).map_err(|e| e.to_string())?;
    println!(
        "wrote and cross-decoder-verified conformance vectors to {}",
        dir.display()
    );
    Ok(())
}

enum RawCodecCase {
    Video {
        payload: Vec<u8>,
        refs: Vec<(u64, Frame420)>,
    },
    Audio {
        payload: Vec<u8>,
    },
}

fn codec_cases(b: &[u8]) -> Result<Vec<RawCodecCase>, String> {
    let (_, pkts, _) = all_packets(b)?;
    let mut hist: VecDeque<(u64, Frame420)> = VecDeque::new();
    let mut cases = Vec::new();
    for pkt in pkts {
        match pkt.kind {
            PacketKind::EpochStart => hist.clear(),
            PacketKind::VideoFrame => {
                let snapshot: Vec<(u64, Frame420)> = hist.iter().cloned().collect();
                let refs: Vec<(u64, &Frame420)> = hist.iter().map(|(id, f)| (*id, f)).collect();
                let (id, f, _) = avelune_video_v1::decode(&pkt.payload, &refs)
                    .map_err(|e| format!("seed video decode: {e:?}"))?;
                cases.push(RawCodecCase::Video {
                    payload: pkt.payload,
                    refs: snapshot,
                });
                hist.push_back((id, f));
                while hist.len() > 4 {
                    hist.pop_front();
                }
            }
            PacketKind::AudioFrame => cases.push(RawCodecCase::Audio {
                payload: pkt.payload,
            }),
            _ => {}
        }
    }
    Ok(cases)
}

fn fuzz_cmd(a: &[String]) -> Result<(), String> {
    let b = read_all(a.get(2).ok_or("missing input")?)?;
    let it = arg_usize(
        a,
        "--iterations",
        a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1000),
    )?;
    let backend = arg_backend(a)?;
    let mut x = 0x9e3779b97f4a7c15u64;
    let mut structurally_valid = 0usize;
    for _ in 0..it {
        x ^= x << 7;
        x ^= x >> 9;
        x ^= x << 8;
        let mut m = b.clone();
        if !m.is_empty() {
            let p = (x as usize) % m.len();
            m[p] ^= 1 << ((x >> 32) & 7);
            let r = std::panic::catch_unwind(|| {
                if all_packets(&m).is_ok() {
                    let _ = decode_video_frames(&m, backend);
                    let _ = decode_audio_samples(&m, backend);
                    true
                } else {
                    false
                }
            });
            if r.is_err() {
                return Err(format!("panic on container mutation at byte {p}"));
            }
            if r.unwrap() {
                structurally_valid += 1;
            }
        }
    }

    // Bypass container CRC to exercise raw codec parsers with the exact valid
    // reference snapshot that precedes each seed packet.
    let cases = codec_cases(&b)?;
    let mut codec_valid = 0usize;
    if !cases.is_empty() {
        for _ in 0..it {
            x ^= x << 7;
            x ^= x >> 9;
            x ^= x << 8;
            let case = &cases[(x as usize) % cases.len()];
            let r = std::panic::catch_unwind(|| match case {
                RawCodecCase::Video { payload, refs } => {
                    let mut m = payload.clone();
                    if !m.is_empty() {
                        let p = ((x >> 11) as usize) % m.len();
                        m[p] ^= 1 << ((x >> 43) & 7);
                    }
                    let rr: Vec<(u64, &Frame420)> = refs.iter().map(|(id, f)| (*id, f)).collect();
                    avelune_video_v1::decode(&m, &rr).is_ok()
                }
                RawCodecCase::Audio { payload } => {
                    let mut m = payload.clone();
                    if !m.is_empty() {
                        let p = ((x >> 11) as usize) % m.len();
                        m[p] ^= 1 << ((x >> 43) & 7);
                    }
                    avelune_audio_v1::decode(&m).is_ok()
                }
            });
            if r.is_err() {
                return Err("panic on raw codec mutation".into());
            }
            if r.unwrap() {
                codec_valid += 1;
            }
        }
    }
    println!(
        "fuzz-smoke container_iterations={} structurally_valid={} raw_codec_iterations={} codec_valid={}",
        it, structurally_valid, it, codec_valid
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let backend = cli.backend;
    match cli.command {
        Command::Encode(a) => {
            let mut v = argv(
                "encode-media",
                &[&a.input, &a.output],
                &[
                    ("--seconds", a.seconds),
                    ("--size", a.size),
                    ("--video-q", Some(a.video_q.to_string())),
                    ("--audio-q", Some(a.audio_q.to_string())),
                    ("--epoch", a.epoch.map(|v| v.to_string())),
                    ("--preset", Some(a.preset.as_str().to_owned())),
                ],
            );
            add_backend(&mut v, backend);
            encode_media_cmd(&v)
        }
        Command::Decode(a) => {
            let mut v = argv("decode-media", &[&a.input, &a.output], &[]);
            add_backend(&mut v, backend);
            decode_media_cmd(&v)
        }
        Command::Play(a) => {
            eprintln!(
                "note: `avelune play` is a convenience path that transcodes/buffers through FFmpeg; browser/native low-latency presentation is a separate integration concern"
            );
            let mut v = argv("play", &[&a.input], &[]);
            add_backend(&mut v, backend);
            play_cmd(&v)
        }
        Command::Inspect(a) => {
            let v = argv("inspect", &[&a.input], &[]);
            inspect_cmd(&v, false)
        }
        Command::Verify(a) => {
            let mut v = argv("verify", &[&a.input], &[]);
            add_backend(&mut v, backend);
            inspect_cmd(&v, true)
        }
        Command::Frames(a) => {
            let v = argv("frames", &[&a.input], &[]);
            frames_cmd(&v)
        }
        Command::Benchmark(a) => {
            let mut v = argv("benchmark", &[&a.input], &[]);
            add_backend(&mut v, backend);
            benchmark_cmd(&v)
        }
        Command::Reindex(a) => {
            let v = argv("reindex", &[&a.input, &a.output], &[]);
            reindex_cmd(&v, false)
        }
        Command::Repair(a) => {
            let v = argv("repair", &[&a.input, &a.output], &[]);
            reindex_cmd(&v, true)
        }
        Command::Conformance(a) => {
            let v = argv("conformance", &[&a.directory], &[]);
            conformance_cmd(&v)
        }
        Command::FuzzSmoke(a) => {
            let it = a.iterations.to_string();
            let mut v = argv("fuzz-smoke", &[&a.input, &it], &[]);
            add_backend(&mut v, backend);
            fuzz_cmd(&v)
        }
        Command::Completions(a) => {
            let mut cmd = Cli::command();
            generate(a.shell, &mut cmd, "avelune", &mut io::stdout());
            Ok(())
        }
        Command::Raw { command } => match command {
            RawCommand::EncodeY4m(a) => {
                let mut v = argv(
                    "encode-y4m",
                    &[&a.input, &a.output],
                    &[
                        ("--q", Some(a.q.to_string())),
                        ("--epoch", a.epoch.map(|v| v.to_string())),
                        ("--preset", Some(a.preset.as_str().to_owned())),
                    ],
                );
                add_backend(&mut v, backend);
                encode_y4m_cmd(&v)
            }
            RawCommand::DecodeY4m(a) => {
                let mut v = argv("decode-y4m", &[&a.input, &a.output], &[]);
                add_backend(&mut v, backend);
                decode_y4m_cmd(&v)
            }
            RawCommand::EncodeAudio(a) => {
                let mut v = argv(
                    "encode-audio",
                    &[&a.input, &a.output],
                    &[
                        ("--q", Some(a.q.to_string())),
                        ("--seconds", a.seconds),
                        ("--epoch-seconds", Some(a.epoch_seconds.to_string())),
                    ],
                );
                add_backend(&mut v, backend);
                encode_audio_cmd(&v)
            }
            RawCommand::DecodeAudio(a) => {
                let mut v = argv("decode-audio", &[&a.input, &a.output], &[]);
                add_backend(&mut v, backend);
                decode_audio_cmd(&v)
            }
        },
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            color_error(&e);
            ExitCode::FAILURE
        }
    }
}
