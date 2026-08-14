use super::*;

/// One decoded output emitted by [`ContainerStreamDecoder`].
#[derive(Debug)]
pub enum DecodedOutput {
    /// Validated epoch boundary. Video reference state has already been reset.
    EpochStart {
        /// Epoch identifier carried by the EpochStart payload.
        id: u32,
        /// Container presentation timestamp.
        pts: u64,
        /// Indexed epoch duration when supplied by the packet.
        duration: u32,
    },
    /// Reconstructed ALV1 video frame.
    Video {
        /// Declared container stream identifier.
        stream_id: u16,
        /// Container presentation timestamp.
        pts: u64,
        /// Packet duration.
        duration: u32,
        /// Immutable ALV1 frame identifier.
        frame_id: u64,
        /// Shared reconstruction; the decoder may retain the same allocation as a reference.
        frame: std::sync::Arc<crate::video::v1::Frame420>,
        /// Immutable reference frame IDs required by this packet.
        dependencies: Vec<u64>,
    },
    /// Decoded ALA1 PCM packet.
    Audio {
        /// Declared container stream identifier.
        stream_id: u16,
        /// Container presentation timestamp.
        pts: u64,
        /// Packet duration.
        duration: u32,
        /// Decoded PCM rate.
        sample_rate: u32,
        /// Decoded channel count.
        channels: u8,
        /// Interleaved signed-16 PCM.
        pcm: Vec<i16>,
    },
    /// Metadata packet. Metadata semantics are not otherwise interpreted by Draft Generation 1.
    Metadata {
        /// Declared stream identifier (zero may denote container-global metadata).
        stream_id: u16,
        /// Container presentation timestamp.
        pts: u64,
        /// Packet duration.
        duration: u32,
        /// Owned metadata payload because parser storage is reused after the callback.
        payload: Vec<u8>,
    },
}

/// Error from the combined streaming container/codec decoder.
#[derive(Debug)]
pub enum StreamDecodeError<E> {
    /// Container framing, checksum, routing, epoch, or resource failure.
    Container(ContainerError),
    /// ALV1 syntax/reconstruction failure.
    Video(crate::video::v1::VideoError),
    /// ALA1 syntax/reconstruction failure.
    Audio(crate::audio::v1::AudioError),
    /// Error returned by the caller's decoded-output consumer.
    Consumer(E),
}

impl<E: std::fmt::Display> std::fmt::Display for StreamDecodeError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container(e) => write!(f, "container: {e}"),
            Self::Video(e) => write!(f, "video: {e}"),
            Self::Audio(e) => write!(f, "audio: {e}"),
            Self::Consumer(e) => write!(f, "consumer: {e}"),
        }
    }
}

impl<E> std::error::Error for StreamDecodeError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Container(e) => Some(e),
            Self::Video(e) => Some(e),
            Self::Audio(e) => Some(e),
            Self::Consumer(e) => Some(e),
        }
    }
}

#[derive(Debug)]
enum DecodeConsumerError<E> {
    Container(ContainerError),
    Video(crate::video::v1::VideoError),
    Audio(crate::audio::v1::AudioError),
    Consumer(E),
}

/// Stateful end-to-end Draft Generation 1 streaming decoder.
///
/// This owns the incremental container parser, epoch validator, ALV1 reference history and ALA1
/// entropy scratch. It is the preferred embedding surface when input arrives incrementally.
/// Transport code may either copy fragments with [`Self::push_each`] or write directly into the
/// parser-owned tail returned by [`Self::reserve_fragment`] and then call [`Self::commit_reserved`].
#[derive(Debug)]
pub struct ContainerStreamDecoder {
    parser: StreamParser,
    epoch: EpochTracker,
    config: crate::config::Config,
    video: std::collections::BTreeMap<u16, crate::video::v1::VideoDecoder>,
    audio: std::collections::BTreeMap<u16, crate::audio::v1::AudioDecoder>,
}

impl Default for ContainerStreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainerStreamDecoder {
    /// Creates a decoder using automatic CPU/thread selection and default resource ceilings.
    pub fn new() -> Self {
        Self::with_config(crate::config::Config::default())
            .expect("the automatic Avelune CPU backend is always available")
    }

    /// Creates a decoder from one common runtime configuration.
    pub fn with_config(
        config: crate::config::Config,
    ) -> Result<Self, crate::config::BackendUnavailable> {
        let kernels = crate::config::kernel_set(config.cpu)?;
        Ok(Self {
            parser: StreamParser::with_limits(config.limits, kernels),
            epoch: EpochTracker::default(),
            config,
            video: std::collections::BTreeMap::new(),
            audio: std::collections::BTreeMap::new(),
        })
    }

    fn reset_video_references(&mut self) {
        for decoder in self.video.values_mut() {
            decoder.reset_epoch();
        }
    }

    /// Reserves parser-owned writable storage for a transport fragment.
    pub fn reserve_fragment(&mut self, len: usize) -> Result<&mut [u8], ContainerError> {
        self.parser.reserve_fragment(len)
    }

    /// Commits bytes from the latest reservation and synchronously emits decoded outputs.
    pub fn commit_reserved<E>(
        &mut self,
        written: usize,
        mut consume: impl FnMut(DecodedOutput) -> Result<(), E>,
    ) -> Result<(), StreamDecodeError<E>> {
        // The parser callback borrows parser storage, so packet information must be handled while
        // the remaining decoder state is borrowed separately. Keeping per-stream codec state in
        // maps makes stream identity an explicit part of the state machine rather than an
        // application-layer convention.
        let parser = &mut self.parser;
        let epoch = &mut self.epoch;
        let config = self.config;
        let video = &mut self.video;
        let audio = &mut self.audio;
        let result = parser.commit_reserved_context(written, |packet, stream| {
            let started = epoch
                .observe(packet)
                .map_err(DecodeConsumerError::<E>::Container)?;
            match packet.kind {
                PacketKind::EpochStart => {
                    let id =
                        started.ok_or(DecodeConsumerError::Container(ContainerError::BadEpoch))?;
                    for decoder in video.values_mut() {
                        decoder.reset_epoch();
                    }
                    consume(DecodedOutput::EpochStart {
                        id,
                        pts: packet.pts,
                        duration: packet.duration,
                    })
                    .map_err(DecodeConsumerError::Consumer)
                }
                PacketKind::VideoFrame => {
                    let stream =
                        stream.ok_or(DecodeConsumerError::Container(ContainerError::BadStream))?;
                    if stream.codec != 1
                        || stream.param0 == 0
                        || stream.param1 == 0
                        || !stream.param0.is_multiple_of(2)
                        || !stream.param1.is_multiple_of(2)
                        || u64::from(stream.param0) * u64::from(stream.param1)
                            > config.limits.max_frame_pixels
                    {
                        return Err(DecodeConsumerError::Container(ContainerError::BadStream));
                    }
                    let decoder = video.entry(packet.stream_id).or_insert_with(|| {
                        crate::video::v1::VideoDecoder::for_stream(
                            config,
                            stream.param0,
                            stream.param1,
                        )
                        .expect("ContainerStreamDecoder validates its CPU backend at construction")
                    });
                    let (frame_id, frame, dependencies) = decoder
                        .decode_shared(packet.payload)
                        .map_err(|error| match error {
                            crate::video::v1::VideoError::BadDimensions => {
                                DecodeConsumerError::Container(ContainerError::BadStream)
                            }
                            other => DecodeConsumerError::Video(other),
                        })?;
                    consume(DecodedOutput::Video {
                        stream_id: packet.stream_id,
                        pts: packet.pts,
                        duration: packet.duration,
                        frame_id,
                        frame,
                        dependencies,
                    })
                    .map_err(DecodeConsumerError::Consumer)
                }
                PacketKind::AudioFrame => {
                    let stream =
                        stream.ok_or(DecodeConsumerError::Container(ContainerError::BadStream))?;
                    let declared_channels = u8::try_from(stream.param1).ok();
                    if stream.codec != 1
                        || !(8_000..=384_000).contains(&stream.param0)
                        || !declared_channels.is_some_and(|channels| (1..=8).contains(&channels))
                    {
                        return Err(DecodeConsumerError::Container(ContainerError::BadStream));
                    }
                    let channels = declared_channels.expect("validated above");
                    let decoder = audio.entry(packet.stream_id).or_insert_with(|| {
                        crate::audio::v1::AudioDecoder::for_stream(
                            config.limits,
                            stream.param0,
                            channels,
                        )
                    });
                    let (sample_rate, channels, pcm) =
                        decoder
                            .decode(packet.payload)
                            .map_err(|error| match error {
                                crate::audio::v1::AudioError::BadRate
                                | crate::audio::v1::AudioError::BadChannels => {
                                    DecodeConsumerError::Container(ContainerError::BadStream)
                                }
                                other => DecodeConsumerError::Audio(other),
                            })?;
                    consume(DecodedOutput::Audio {
                        stream_id: packet.stream_id,
                        pts: packet.pts,
                        duration: packet.duration,
                        sample_rate,
                        channels,
                        pcm,
                    })
                    .map_err(DecodeConsumerError::Consumer)
                }
                PacketKind::Metadata => consume(DecodedOutput::Metadata {
                    stream_id: packet.stream_id,
                    pts: packet.pts,
                    duration: packet.duration,
                    payload: packet.payload.to_vec(),
                })
                .map_err(DecodeConsumerError::Consumer),
            }
        });
        match result {
            Ok(()) => Ok(()),
            Err(StreamPushError::Container(e)) => Err(StreamDecodeError::Container(e)),
            Err(StreamPushError::Consumer(DecodeConsumerError::Video(e))) => {
                Err(StreamDecodeError::Video(e))
            }
            Err(StreamPushError::Consumer(DecodeConsumerError::Audio(e))) => {
                Err(StreamDecodeError::Audio(e))
            }
            Err(StreamPushError::Consumer(DecodeConsumerError::Consumer(e))) => {
                Err(StreamDecodeError::Consumer(e))
            }
            Err(StreamPushError::Consumer(DecodeConsumerError::Container(e))) => {
                Err(StreamDecodeError::Container(e))
            }
        }
    }

    /// Copies one arbitrary transport fragment and emits every complete decoded output available.
    pub fn push_each<E>(
        &mut self,
        fragment: &[u8],
        consume: impl FnMut(DecodedOutput) -> Result<(), E>,
    ) -> Result<(), StreamDecodeError<E>> {
        self.reserve_fragment(fragment.len())
            .map_err(StreamDecodeError::Container)?
            .copy_from_slice(fragment);
        self.commit_reserved(fragment.len(), consume)
    }

    /// Resets to a new complete file stream, clearing codec state and previously parsed index data.
    pub fn reset_file(&mut self) {
        self.parser.reset_file();
        self.epoch.reset_file();
        self.video.clear();
        self.audio.clear();
    }

    /// Resets to one independently fetched indexed epoch range. Stream-local codec allocations are
    /// retained, while every video reference history is cleared as required by an epoch boundary.
    pub fn reset_epoch_range(&mut self, expected_epoch: Option<u32>) {
        self.parser.reset_epoch_range();
        self.epoch.reset_range(expected_epoch);
        self.reset_video_references();
    }

    /// Validated fixed header when the complete header has arrived.
    pub fn file_header(&self) -> Option<&FileHeader> {
        self.parser.file_header()
    }

    /// Validated front index when the complete prefix has arrived.
    pub fn front_index(&self) -> Option<&Front> {
        self.parser.front_index()
    }

    /// Current validated epoch identifier.
    pub const fn current_epoch(&self) -> Option<u32> {
        self.epoch.current()
    }

    /// Validates that the current file/range transaction ended on a complete packet boundary.
    pub fn finish_input(&self) -> Result<(), ContainerError> {
        self.parser.finish_input()
    }

    /// Consumes the decoder after validating complete input.
    pub fn finish(self) -> Result<(), ContainerError> {
        self.parser.finish()
    }
}
