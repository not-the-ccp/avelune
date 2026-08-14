#![forbid(unsafe_code)]

mod codec;
mod container_tools;
mod error;
mod ffmpeg;
mod io_util;
mod y4m;

use std::{
    io::{self, IsTerminal},
    process::{Command as ProcessCommand, ExitCode},
};

use avelune::video::v1::EncoderPreset;
use clap::{Args as ClapArgs, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};

use crate::{
    error::{CliError, Result},
    io_util::{read_all, write_all},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PresetArg {
    Fast,
    Balanced,
    Quality,
}
impl From<PresetArg> for EncoderPreset {
    fn from(value: PresetArg) -> Self {
        match value {
            PresetArg::Fast => Self::Fast,
            PresetArg::Balanced => Self::Balanced,
            PresetArg::Quality => Self::Quality,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "avelune",
    version,
    about = "Encode, decode, inspect, and validate Avelune Draft Generation 1 media",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Encode an ordinary media file through FFmpeg into Avelune.
    Encode(MediaEncodeArgs),
    /// Decode a single-video/single-audio Avelune container into an ordinary media file.
    Decode(FilePairArgs),
    /// Convenience playback path through FFmpeg/ffplay.
    Play(FileInputArgs),
    /// Show front-index, stream, and epoch metadata.
    Inspect(FileInputArgs),
    /// Run the canonical end-to-end stream decoder over every packet.
    Verify(FileInputArgs),
    /// Print one concise line per coded video frame.
    Frames(FileInputArgs),
    /// Rebuild the front index from valid packets.
    Reindex(FilePairArgs),
    /// Best-effort packet resynchronization followed by checked front-index rebuilding.
    Repair(FilePairArgs),
    /// Generate shell completions to stdout.
    Completions(CompletionsArgs),
    /// Raw workflows that avoid the ordinary-media bridge.
    Raw {
        #[command(subcommand)]
        command: RawCommands,
    },
}

#[derive(Debug, Subcommand)]
enum RawCommands {
    /// Encode 8-bit 4:2:0 Y4M into Avelune.
    EncodeY4m(RawVideoEncodeArgs),
    /// Decode a single video stream to Y4M.
    DecodeY4m(FilePairArgs),
    /// Encode an ordinary audio file through FFmpeg into audio-only Avelune.
    EncodeAudio(RawAudioEncodeArgs),
    /// Decode a single audio stream to interleaved signed 16-bit little-endian PCM.
    DecodeAudio(FilePairArgs),
}

#[derive(Debug, ClapArgs)]
struct FileInputArgs {
    input: String,
}
#[derive(Debug, ClapArgs)]
struct FilePairArgs {
    input: String,
    output: String,
}
#[derive(Debug, ClapArgs)]
struct MediaEncodeArgs {
    input: String,
    output: String,
    #[arg(long)]
    seconds: Option<String>,
    #[arg(long)]
    size: Option<String>,
    #[arg(long, default_value_t = 96)]
    video_q: u16,
    #[arg(long, default_value_t = 1)]
    audio_q: u16,
    #[arg(long)]
    epoch: Option<usize>,
    #[arg(long, value_enum, default_value_t=PresetArg::Balanced)]
    preset: PresetArg,
}
#[derive(Debug, ClapArgs)]
struct RawVideoEncodeArgs {
    input: String,
    output: String,
    #[arg(short = 'q', long, default_value_t = 96)]
    q: u16,
    #[arg(long)]
    epoch: Option<usize>,
    #[arg(long, value_enum, default_value_t=PresetArg::Balanced)]
    preset: PresetArg,
}
#[derive(Debug, ClapArgs)]
struct RawAudioEncodeArgs {
    input: String,
    output: String,
    #[arg(short = 'q', long, default_value_t = 1)]
    q: u16,
    #[arg(long)]
    seconds: Option<String>,
    #[arg(long, default_value_t = 2)]
    epoch_seconds: usize,
}
#[derive(Debug, ClapArgs)]
struct CompletionsArgs {
    #[arg(value_enum)]
    shell: Shell,
}

fn default_epoch_frames(video: &y4m::Y4m) -> usize {
    let fps = u64::from(video.fps_n / video.fps_d.max(1));
    usize::try_from(fps.saturating_mul(2))
        .unwrap_or(usize::MAX)
        .max(1)
}

fn encode_media(args: MediaEncodeArgs) -> Result<()> {
    let video = ffmpeg::extract_video(&args.input, args.seconds.as_deref(), args.size.as_deref())?;
    let audio = ffmpeg::extract_audio(&args.input, args.seconds.as_deref())?;
    let epoch = args.epoch.unwrap_or_else(|| default_epoch_frames(&video));
    let encoded = codec::encode_av(
        &video,
        audio
            .as_ref()
            .map(|a| (a.samples.as_slice(), a.channels, a.rate)),
        args.video_q,
        args.audio_q,
        epoch,
        args.preset.into(),
    )?;
    write_all(&args.output, &encoded)?;
    eprintln!(
        "encoded frames={} bytes={} epochs<=~{} qv={} qa={}",
        video.frames.len(),
        encoded.len(),
        epoch,
        args.video_q,
        args.audio_q
    );
    Ok(())
}

fn encode_y4m(args: RawVideoEncodeArgs) -> Result<()> {
    let video = y4m::parse(&read_all(&args.input)?)?;
    let epoch = args.epoch.unwrap_or_else(|| default_epoch_frames(&video));
    let encoded = codec::encode_av(&video, None, args.q, 1, epoch, args.preset.into())?;
    write_all(&args.output, &encoded)
}

fn encode_audio(args: RawAudioEncodeArgs) -> Result<()> {
    let audio = ffmpeg::extract_audio(&args.input, args.seconds.as_deref())?
        .ok_or_else(|| CliError::message("input has no audio stream"))?;
    let encoded = codec::encode_audio_only(
        &audio.samples,
        audio.channels,
        audio.rate,
        args.q,
        args.epoch_seconds,
    )?;
    write_all(&args.output, &encoded)
}

fn decode_media(input: &str, output: &str) -> Result<()> {
    let bytes = read_all(input)?;
    let media = codec::decode(&bytes)?;
    ffmpeg::write_media(&media, output)
}

fn play(input: &str) -> Result<()> {
    eprintln!(
        "note: `avelune play` is a convenience transcode path; the browser player is the indexed low-latency integration"
    );
    let temp = io_util::TempPath::new("play.mkv")?;
    let path = temp.path().to_string_lossy().into_owned();
    decode_media(input, &path)?;
    let mut command = ProcessCommand::new("ffplay");
    command
        .args(["-autoexit", "-loglevel", "warning"])
        .arg(temp.path());
    io_util::run_checked(&mut command, "ffplay")
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Encode(args) => encode_media(args),
        Commands::Decode(args) => decode_media(&args.input, &args.output),
        Commands::Play(args) => play(&args.input),
        Commands::Inspect(args) => container_tools::inspect(&read_all(&args.input)?),
        Commands::Verify(args) => container_tools::verify(&read_all(&args.input)?),
        Commands::Frames(args) => container_tools::frames(&read_all(&args.input)?),
        Commands::Reindex(args) => {
            let out = container_tools::reindex(&read_all(&args.input)?, false)?;
            write_all(&args.output, &out)
        }
        Commands::Repair(args) => {
            let out = container_tools::reindex(&read_all(&args.input)?, true)?;
            write_all(&args.output, &out)
        }
        Commands::Completions(args) => {
            let mut command = Cli::command();
            generate(args.shell, &mut command, "avelune", &mut io::stdout());
            Ok(())
        }
        Commands::Raw { command } => match command {
            RawCommands::EncodeY4m(args) => encode_y4m(args),
            RawCommands::DecodeY4m(args) => {
                let media = codec::decode(&read_all(&args.input)?)?;
                let video = media
                    .video
                    .ok_or_else(|| CliError::message("container has no video stream"))?;
                write_all(&args.output, &y4m::emit(&video))
            }
            RawCommands::EncodeAudio(args) => encode_audio(args),
            RawCommands::DecodeAudio(args) => {
                let media = codec::decode(&read_all(&args.input)?)?;
                let audio = media
                    .audio
                    .ok_or_else(|| CliError::message("container has no audio stream"))?;
                let mut raw = Vec::with_capacity(audio.samples.len() * 2);
                for sample in audio.samples {
                    raw.extend_from_slice(&sample.to_le_bytes());
                }
                write_all(&args.output, &raw)
            }
        },
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let color = io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
            if color {
                eprintln!("\x1b[1;31merror:\x1b[0m {error}");
            } else {
                eprintln!("error: {error}");
            }
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::FAILURE
        }
    }
}
