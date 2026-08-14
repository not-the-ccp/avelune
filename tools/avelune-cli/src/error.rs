use std::{convert::Infallible, error::Error, fmt, io, process::ExitStatus};

use avelune::{
    audio::v1::AudioError,
    container::v1::{ContainerError, StreamDecodeError},
    video::v1::VideoError,
};

#[derive(Debug)]
pub enum CliError {
    Io { context: String, source: io::Error },
    Process { command: String, status: ExitStatus },
    Message(String),
    Container(ContainerError),
    Video(VideoError),
    Audio(AudioError),
    Stream(StreamDecodeError<Infallible>),
}

impl CliError {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Process { command, status } => write!(f, "{command} failed with {status}"),
            Self::Message(message) => f.write_str(message),
            Self::Container(e) => write!(f, "container: {e}"),
            Self::Video(e) => write!(f, "video: {e}"),
            Self::Audio(e) => write!(f, "audio: {e}"),
            Self::Stream(e) => write!(f, "{e}"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Container(e) => Some(e),
            Self::Video(e) => Some(e),
            Self::Audio(e) => Some(e),
            Self::Stream(e) => Some(e),
            Self::Process { .. } | Self::Message(_) => None,
        }
    }
}

impl From<ContainerError> for CliError {
    fn from(value: ContainerError) -> Self {
        Self::Container(value)
    }
}
impl From<VideoError> for CliError {
    fn from(value: VideoError) -> Self {
        Self::Video(value)
    }
}
impl From<AudioError> for CliError {
    fn from(value: AudioError) -> Self {
        Self::Audio(value)
    }
}
impl From<StreamDecodeError<Infallible>> for CliError {
    fn from(value: StreamDecodeError<Infallible>) -> Self {
        Self::Stream(value)
    }
}

pub type Result<T> = std::result::Result<T, CliError>;
