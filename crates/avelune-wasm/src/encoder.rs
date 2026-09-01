use avelune::{
    audio::v1::{self as audio, EncodeOptions as AudioEncodeOptions},
    container::v1::{
        self as container, ChromaLocation, Packet, PacketKind, StreamDesc, StreamKind, TIMEBASE,
        VideoMeta,
    },
    limits::Limits,
    video::v1::{EncodeOptions, EncoderPreset, Frame420, VideoEncoder},
};
use std::{cell::RefCell, collections::VecDeque};

#[derive(Debug)]
struct EncodedFrame {
    frame_id: u64,
    pts: u64,
    duration: u32,
    payload: Vec<u8>,
}

struct VideoEncoderState {
    width: u32,
    height: u32,
    fps_n: u32,
    fps_d: u32,
    meta0: u32,
    frame_us: u64,
    epoch_frames: u64,
    options: EncodeOptions,
    encoder: VideoEncoder,
    frame_index: u64,
    epoch_start_frame: u64,
    epoch_id: u32,
    pending: Vec<EncodedFrame>,
    epochs: Vec<(u32, u64, u32, Vec<u8>)>,
    frame_bytes: usize,
    input: Vec<u8>,
    output: Vec<u8>,
    finished: bool,
    error: String,
}

impl VideoEncoderState {
    fn new(
        width: u32,
        height: u32,
        fps_flags: u32,
        qstep: u16,
        preset: EncoderPreset,
        epoch_frames: u64,
        meta0: u32,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err("video dimensions must be non-zero and even for 4:2:0".into());
        }
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| "video dimensions overflow".to_string())?;
        if pixels > Limits::default().max_frame_pixels {
            return Err("video dimensions exceed the canonical frame-pixel limit".into());
        }
        let fps_n = fps_flags >> 16;
        let fps_d = fps_flags & 65_535;
        if fps_n == 0 || fps_d == 0 {
            return Err(
                "packed frame rate must contain non-zero 16-bit numerator and denominator".into(),
            );
        }
        if qstep == 0 {
            return Err("video quantizer step must be > 0".into());
        }
        if epoch_frames == 0 {
            return Err("epoch frame count must be > 0".into());
        }
        VideoMeta::unpack(meta0).map_err(|e| format!("invalid packed video metadata: {e}"))?;
        let frame_us = 1_000_000u64
            .checked_mul(u64::from(fps_d))
            .ok_or_else(|| "frame duration overflow".to_string())?
            / u64::from(fps_n);
        if frame_us == 0 || frame_us > u64::from(u32::MAX) {
            return Err(
                "frame rate cannot be represented by the Draft Gen 1 microsecond timebase".into(),
            );
        }
        let options = EncodeOptions::for_preset(qstep, preset);
        let y =
            usize::try_from(pixels).map_err(|_| "frame size exceeds address space".to_string())?;
        let frame_bytes = y
            .checked_add(y / 2)
            .ok_or_else(|| "frame byte size overflow".to_string())?;
        Ok(Self {
            width,
            height,
            fps_n,
            fps_d,
            meta0,
            frame_us,
            epoch_frames,
            options,
            encoder: VideoEncoder::new(options),
            frame_index: 0,
            epoch_start_frame: 0,
            epoch_id: 0,
            pending: Vec::new(),
            epochs: Vec::new(),
            frame_bytes,
            input: vec![0; frame_bytes],
            output: Vec::new(),
            finished: false,
            error: String::new(),
        })
    }

    fn fail(&mut self, error: impl std::fmt::Display) -> i32 {
        self.error = error.to_string();
        -1
    }

    fn push_frame(&mut self) -> Result<(), String> {
        if self.finished {
            return Err("encoder is already finished".into());
        }
        if self.frame_index - self.epoch_start_frame >= self.epoch_frames {
            self.finish_epoch()?;
        }
        let input = std::mem::take(&mut self.input);
        let frame = match Frame420::from_tightly_packed(self.width, self.height, input) {
            Ok(frame) => frame,
            Err(error) => {
                self.input = vec![0; self.frame_bytes];
                return Err(error.to_string());
            }
        };
        let frame_id = self.frame_index;
        let encoded = self.encoder.encode_shared(frame_id, &frame);
        self.input = frame.into_tightly_packed();
        let encoded = encoded.map_err(|e| e.to_string())?;
        let pts = frame_id
            .checked_mul(self.frame_us)
            .ok_or_else(|| "frame timestamp overflow".to_string())?;
        self.pending.push(EncodedFrame {
            frame_id,
            pts,
            duration: self.frame_us as u32,
            payload: encoded.packet,
        });
        self.frame_index += 1;
        Ok(())
    }

    fn finish_epoch(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            self.epoch_start_frame = self.frame_index;
            self.encoder = VideoEncoder::new(self.options);
            return Ok(());
        }
        let pts = self
            .epoch_start_frame
            .checked_mul(self.frame_us)
            .ok_or_else(|| "epoch timestamp overflow".to_string())?;
        let end_pts = self
            .frame_index
            .checked_mul(self.frame_us)
            .ok_or_else(|| "epoch timestamp overflow".to_string())?;
        let duration = u32::try_from(end_pts - pts)
            .map_err(|_| "epoch duration exceeds Draft Gen 1 range".to_string())?;
        let mut bytes = Vec::new();
        container::encode_packet_checked(
            &Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts,
                duration,
                payload: self.epoch_id.to_le_bytes().to_vec(),
            },
            &mut bytes,
        )
        .map_err(|e| e.to_string())?;
        for frame in self.pending.drain(..) {
            debug_assert!(frame.frame_id >= self.epoch_start_frame);
            container::encode_packet_checked(
                &Packet {
                    kind: PacketKind::VideoFrame,
                    flags: 0,
                    stream_id: 1,
                    pts: frame.pts,
                    duration: frame.duration,
                    payload: frame.payload,
                },
                &mut bytes,
            )
            .map_err(|e| e.to_string())?;
        }
        self.epochs.push((self.epoch_id, pts, duration, bytes));
        self.epoch_id = self
            .epoch_id
            .checked_add(1)
            .ok_or_else(|| "epoch identifier overflow".to_string())?;
        self.epoch_start_frame = self.frame_index;
        self.encoder = VideoEncoder::new(self.options);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        if self.frame_index == 0 {
            return Err("cannot finish an empty video".into());
        }
        self.finish_epoch()?;
        let streams = vec![StreamDesc {
            id: 1,
            kind: StreamKind::Video,
            codec: 1,
            timescale: TIMEBASE,
            param0: self.width,
            param1: self.height,
            flags: (self.fps_n << 16) | self.fps_d,
            meta0: self.meta0,
        }];
        self.output = container::build_file_checked(streams, std::mem::take(&mut self.epochs))
            .map_err(|e| e.to_string())?;
        self.finished = true;
        Ok(())
    }
}

thread_local! {
    static ENCODERS: RefCell<Vec<Option<VideoEncoderState>>> = const { RefCell::new(Vec::new()) };
    static CREATE_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn fail_create(message: impl Into<String>) -> u32 {
    CREATE_ERROR.with(|error| *error.borrow_mut() = message.into());
    0
}

fn with_encoder<R>(handle: u32, default: R, f: impl FnOnce(&mut VideoEncoderState) -> R) -> R {
    if handle == 0 {
        return default;
    }
    ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        encoders
            .get_mut(handle as usize - 1)
            .and_then(Option::as_mut)
            .map(f)
            .unwrap_or(default)
    })
}

/// Packs chroma location (`0` unspecified, `1` left, `2` center) and a full-range flag into the
/// canonical Draft Generation 1 video metadata field. Returns `u32::MAX` for an unknown chroma
/// location.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_pack_meta0(chroma_location: u32, full_range: u32) -> u32 {
    let chroma = match chroma_location {
        0 => ChromaLocation::Unspecified,
        1 => ChromaLocation::Left,
        2 => ChromaLocation::Center,
        _ => return u32::MAX,
    };
    VideoMeta {
        chroma,
        full_range: full_range != 0,
        ..VideoMeta::default()
    }
    .pack()
}

/// Creates a video-only Draft Gen 1 encoder. Preset is 0=fast, 1=balanced, 2=quality.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_create(
    width: u32,
    height: u32,
    fps_flags: u32,
    qstep: u32,
    preset: u32,
    epoch_frames: u32,
    meta0: u32,
) -> u32 {
    CREATE_ERROR.with(|error| error.borrow_mut().clear());
    let preset = match preset {
        0 => EncoderPreset::Fast,
        1 => EncoderPreset::Balanced,
        2 => EncoderPreset::Quality,
        _ => return fail_create("unknown encoder preset"),
    };
    let qstep = match u16::try_from(qstep) {
        Ok(value) => value,
        Err(_) => return fail_create("video quantizer step must fit u16"),
    };
    let state = match VideoEncoderState::new(
        width,
        height,
        fps_flags,
        qstep,
        preset,
        u64::from(epoch_frames),
        meta0,
    ) {
        Ok(state) => state,
        Err(error) => return fail_create(error),
    };
    ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        if let Some((index, slot)) = encoders
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.is_none())
        {
            *slot = Some(state);
            (index + 1) as u32
        } else {
            encoders.push(Some(state));
            encoders.len() as u32
        }
    })
}

/// Returns the UTF-8 byte pointer for the most recent failed [`video_encoder_create`] call.
/// Pair with [`video_encoder_create_error_len`]; a zero length means the pointer must not be read.
/// The pointer remains valid only until another create attempt mutates this error or WebAssembly
/// memory growth invalidates the caller's linear-memory view.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_create_error_ptr() -> *const u8 {
    CREATE_ERROR.with(|error| error.borrow().as_ptr())
}

/// Returns the UTF-8 byte length for the most recent failed [`video_encoder_create`] call.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_create_error_len() -> u32 {
    CREATE_ERROR.with(|error| error.borrow().len().try_into().unwrap_or(0))
}

/// Destroys an encoder handle. Returns `0` on success and `-1` for an invalid/already-destroyed handle.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_destroy(handle: u32) -> i32 {
    if handle == 0 {
        return -1;
    }
    ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        let Some(slot) = encoders.get_mut(handle as usize - 1) else {
            return -1;
        };
        if slot.take().is_some() { 0 } else { -1 }
    })
}

/// Returns the writable tightly packed Y/U/V staging-buffer length, or `0` for an invalid handle.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_frame_len(handle: u32) -> u32 {
    with_encoder(handle, 0, |encoder| {
        encoder.input.len().try_into().unwrap_or(0)
    })
}

/// Returns the writable frame staging-buffer pointer, or null for an invalid handle.
///
/// Pair this pointer with [`video_encoder_frame_len`]. Reacquire the pointer after any exported
/// call that can allocate/grow WebAssembly memory before constructing a new JavaScript view.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_frame_ptr(handle: u32) -> *mut u8 {
    with_encoder(handle, std::ptr::null_mut(), |encoder| {
        encoder.input.as_mut_ptr()
    })
}

/// Encodes the frame currently stored in the staging buffer. Returns `0` on success and `-1` on
/// failure; inspect [`video_encoder_last_error_ptr`] and [`video_encoder_last_error_len`] after a
/// failure.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_push_frame(handle: u32) -> i32 {
    with_encoder(handle, -1, |encoder| match encoder.push_frame() {
        Ok(()) => 0,
        Err(error) => encoder.fail(error),
    })
}

/// Finalizes the current epoch/container output. Returns `0` on success and `-1` on failure.
/// Output pointers must be reacquired after this call because finalization may grow memory.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_finish(handle: u32) -> i32 {
    with_encoder(handle, -1, |encoder| match encoder.finish() {
        Ok(()) => 0,
        Err(error) => encoder.fail(error),
    })
}

/// Returns the encoded output pointer, or null for an invalid handle.
///
/// Pair with [`video_encoder_output_len`]. The pointer is valid until a later encoder call mutates
/// output or any WebAssembly memory growth invalidates the caller's linear-memory view.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_output_ptr(handle: u32) -> *const u8 {
    with_encoder(handle, std::ptr::null(), |encoder| encoder.output.as_ptr())
}

/// Returns the current encoded output length, or `0` for an invalid handle/empty output.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_output_len(handle: u32) -> u32 {
    with_encoder(handle, 0, |encoder| {
        encoder.output.len().try_into().unwrap_or(0)
    })
}

/// Returns the current last-error UTF-8 byte pointer, or null for an invalid handle.
///
/// Read only when [`video_encoder_last_error_len`] is nonzero and pair the pointer with that exact
/// length. Reacquire it after any later encoder call or WebAssembly memory growth.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_last_error_ptr(handle: u32) -> *const u8 {
    with_encoder(handle, std::ptr::null(), |encoder| encoder.error.as_ptr())
}

/// Returns the current last-error UTF-8 byte length. A zero length means there is no readable
/// last-error string and the corresponding pointer must not be dereferenced.
#[unsafe(no_mangle)]
pub extern "C" fn video_encoder_last_error_len(handle: u32) -> u32 {
    with_encoder(handle, 0, |encoder| {
        encoder.error.len().try_into().unwrap_or(0)
    })
}

#[derive(Debug)]
struct AvEncoderState {
    width: u32,
    height: u32,
    fps_n: u32,
    fps_d: u32,
    meta0: u32,
    frame_us: u64,
    epoch_frames: u64,
    video_options: EncodeOptions,
    video_encoder: VideoEncoder,
    frame_index: u64,
    encoded_video: Vec<EncodedFrame>,
    frame_bytes: usize,
    video_input: Vec<u8>,
    audio_rate: u32,
    audio_channels: u8,
    audio_qstep: u16,
    audio_staging: Vec<i16>,
    audio_samples: Vec<i16>,
    output: Vec<u8>,
    finished: bool,
    error: String,
}

impl AvEncoderState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        width: u32,
        height: u32,
        fps_flags: u32,
        video_qstep: u16,
        preset: EncoderPreset,
        epoch_frames: u64,
        meta0: u32,
        audio_rate: u32,
        audio_channels: u32,
        audio_qstep: u16,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err("video dimensions must be non-zero and even for 4:2:0".into());
        }
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| "video dimensions overflow".to_string())?;
        if pixels > Limits::default().max_frame_pixels {
            return Err("video dimensions exceed the canonical frame-pixel limit".into());
        }
        let fps_n = fps_flags >> 16;
        let fps_d = fps_flags & 65_535;
        if fps_n == 0 || fps_d == 0 {
            return Err(
                "packed frame rate must contain non-zero 16-bit numerator and denominator".into(),
            );
        }
        if video_qstep == 0 {
            return Err("video quantizer step must be > 0".into());
        }
        if epoch_frames == 0 {
            return Err("epoch frame count must be > 0".into());
        }
        VideoMeta::unpack(meta0).map_err(|e| format!("invalid packed video metadata: {e}"))?;
        let frame_us = 1_000_000u64
            .checked_mul(u64::from(fps_d))
            .ok_or_else(|| "frame duration overflow".to_string())?
            / u64::from(fps_n);
        if frame_us == 0 || frame_us > u64::from(u32::MAX) {
            return Err(
                "frame rate cannot be represented by the Draft Gen 1 microsecond timebase".into(),
            );
        }
        let audio_channels = u8::try_from(audio_channels)
            .map_err(|_| "audio channel count must be in 0..=8".to_string())?;
        let audio_enabled = audio_rate != 0 || audio_channels != 0;
        if audio_enabled {
            if audio_rate == 0 {
                return Err("audio sample rate must be > 0 when audio is enabled".into());
            }
            if audio_channels == 0 || audio_channels > 8 {
                return Err("audio channel count must be in 1..=8 when audio is enabled".into());
            }
            if audio_qstep == 0 {
                return Err("audio quantizer step must be > 0 when audio is enabled".into());
            }
        }
        let video_options = EncodeOptions::for_preset(video_qstep, preset);
        let y =
            usize::try_from(pixels).map_err(|_| "frame size exceeds address space".to_string())?;
        let frame_bytes = y
            .checked_add(y / 2)
            .ok_or_else(|| "frame byte size overflow".to_string())?;
        Ok(Self {
            width,
            height,
            fps_n,
            fps_d,
            meta0,
            frame_us,
            epoch_frames,
            video_options,
            video_encoder: VideoEncoder::new(video_options),
            frame_index: 0,
            encoded_video: Vec::new(),
            frame_bytes,
            video_input: vec![0; frame_bytes],
            audio_rate,
            audio_channels,
            audio_qstep,
            audio_staging: Vec::new(),
            audio_samples: Vec::new(),
            output: Vec::new(),
            finished: false,
            error: String::new(),
        })
    }

    fn audio_enabled(&self) -> bool {
        self.audio_rate != 0
    }

    fn fail(&mut self, error: impl std::fmt::Display) -> i32 {
        self.error = error.to_string();
        -1
    }

    fn push_video_frame(&mut self) -> Result<(), String> {
        if self.finished {
            return Err("encoder is already finished".into());
        }
        if self.frame_index > 0 && self.frame_index.is_multiple_of(self.epoch_frames) {
            self.video_encoder = VideoEncoder::new(self.video_options);
        }
        let input = std::mem::take(&mut self.video_input);
        let frame = match Frame420::from_tightly_packed(self.width, self.height, input) {
            Ok(frame) => frame,
            Err(error) => {
                self.video_input = vec![0; self.frame_bytes];
                return Err(error.to_string());
            }
        };
        let frame_id = self.frame_index;
        let encoded = self.video_encoder.encode_shared(frame_id, &frame);
        self.video_input = frame.into_tightly_packed();
        let encoded = encoded.map_err(|e| e.to_string())?;
        let pts = frame_id
            .checked_mul(self.frame_us)
            .ok_or_else(|| "frame timestamp overflow".to_string())?;
        self.encoded_video.push(EncodedFrame {
            frame_id,
            pts,
            duration: self.frame_us as u32,
            payload: encoded.packet,
        });
        self.frame_index += 1;
        Ok(())
    }

    fn reserve_audio(&mut self, sample_count: usize) -> Result<*mut i16, String> {
        if self.finished {
            return Err("encoder is already finished".into());
        }
        if !self.audio_enabled() {
            return Err("audio is disabled for this encoder".into());
        }
        let max_samples = 128usize * 1024 * 1024 / std::mem::size_of::<i16>();
        if sample_count > max_samples {
            return Err("one browser audio staging request exceeds 128 MiB".into());
        }
        self.audio_staging.resize(sample_count, 0);
        Ok(self.audio_staging.as_mut_ptr())
    }

    fn push_audio(&mut self, sample_count: usize) -> Result<(), String> {
        if !self.audio_enabled() {
            return Err("audio is disabled for this encoder".into());
        }
        if sample_count != self.audio_staging.len() {
            return Err("audio push length does not match the current staging buffer".into());
        }
        let channels = usize::from(self.audio_channels);
        if !sample_count.is_multiple_of(channels) {
            return Err("audio sample count is not aligned to the configured channels".into());
        }
        self.audio_samples.extend_from_slice(&self.audio_staging);
        self.audio_staging.clear();
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        if self.encoded_video.is_empty() {
            return Err("cannot finish media with no video frames".into());
        }
        let total_video_frames = self.encoded_video.len() as u64;
        let epoch_count = total_video_frames.div_ceil(self.epoch_frames);
        let mut epochs = Vec::with_capacity(epoch_count as usize);
        let audio_channels = usize::from(self.audio_channels);
        let total_audio_frames = if self.audio_enabled() {
            if !self.audio_samples.len().is_multiple_of(audio_channels) {
                return Err("audio samples are not aligned to the configured channels".into());
            }
            self.audio_samples.len() / audio_channels
        } else {
            0
        };

        for epoch_index in 0..epoch_count {
            let start_frame = epoch_index * self.epoch_frames;
            let end_frame = (start_frame + self.epoch_frames).min(total_video_frames);
            let pts = start_frame
                .checked_mul(self.frame_us)
                .ok_or_else(|| "epoch timestamp overflow".to_string())?;
            let end_pts = end_frame
                .checked_mul(self.frame_us)
                .ok_or_else(|| "epoch timestamp overflow".to_string())?;
            let duration = u32::try_from(end_pts - pts)
                .map_err(|_| "epoch duration exceeds Draft Gen 1 range".to_string())?;
            let epoch_id = u32::try_from(epoch_index)
                .map_err(|_| "too many browser encoder epochs".to_string())?;
            let mut packets = Vec::new();
            packets.push(Packet {
                kind: PacketKind::EpochStart,
                flags: 0,
                stream_id: 0,
                pts,
                duration,
                payload: epoch_id.to_le_bytes().to_vec(),
            });
            for frame in &self.encoded_video[start_frame as usize..end_frame as usize] {
                packets.push(Packet {
                    kind: PacketKind::VideoFrame,
                    flags: 0,
                    stream_id: 1,
                    pts: frame.pts,
                    duration: frame.duration,
                    payload: frame.payload.clone(),
                });
            }
            if self.audio_enabled() {
                let rate = u64::from(self.audio_rate);
                let start_audio =
                    ((pts * rate) / 1_000_000).min(total_audio_frames as u64) as usize;
                let end_audio =
                    ((end_pts * rate) / 1_000_000).min(total_audio_frames as u64) as usize;
                let mut af = start_audio;
                while af < end_audio {
                    let n = (end_audio - af).min(960);
                    let begin = af * audio_channels;
                    let finish = (af + n) * audio_channels;
                    let payload = audio::encode(
                        &self.audio_samples[begin..finish],
                        AudioEncodeOptions {
                            sample_rate: self.audio_rate,
                            channels: self.audio_channels,
                            qstep: self.audio_qstep,
                            mid_side: self.audio_channels == 2,
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    packets.push(Packet {
                        kind: PacketKind::AudioFrame,
                        flags: 0,
                        stream_id: 2,
                        pts: af as u64 * 1_000_000 / rate,
                        duration: u32::try_from(n as u64 * 1_000_000 / rate)
                            .map_err(|_| "audio packet duration overflow".to_string())?,
                        payload,
                    });
                    af += n;
                }
            }
            packets.sort_by_key(|packet| {
                (
                    u8::from(packet.kind != PacketKind::EpochStart),
                    packet.pts,
                    match packet.kind {
                        PacketKind::VideoFrame => 0u8,
                        PacketKind::AudioFrame => 1,
                        PacketKind::Metadata => 2,
                        PacketKind::EpochStart => 0,
                    },
                )
            });
            let mut bytes = Vec::new();
            for packet in packets {
                container::encode_packet_checked(&packet, &mut bytes).map_err(|e| e.to_string())?;
            }
            epochs.push((epoch_id, pts, duration, bytes));
        }

        let mut streams = vec![StreamDesc {
            id: 1,
            kind: StreamKind::Video,
            codec: 1,
            timescale: TIMEBASE,
            param0: self.width,
            param1: self.height,
            flags: (self.fps_n << 16) | self.fps_d,
            meta0: self.meta0,
        }];
        if self.audio_enabled() {
            streams.push(StreamDesc {
                id: 2,
                kind: StreamKind::Audio,
                codec: 1,
                timescale: TIMEBASE,
                param0: self.audio_rate,
                param1: u32::from(self.audio_channels),
                flags: 0,
                meta0: 0,
            });
        }
        self.output = container::build_file_checked(streams, epochs).map_err(|e| e.to_string())?;
        self.finished = true;
        Ok(())
    }
}

thread_local! {
    static AV_ENCODERS: RefCell<Vec<Option<AvEncoderState>>> = const { RefCell::new(Vec::new()) };
    static AV_CREATE_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn fail_av_create(message: impl Into<String>) -> u32 {
    AV_CREATE_ERROR.with(|error| *error.borrow_mut() = message.into());
    0
}

fn with_av_encoder<R>(handle: u32, default: R, f: impl FnOnce(&mut AvEncoderState) -> R) -> R {
    if handle == 0 {
        return default;
    }
    AV_ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        encoders
            .get_mut(handle as usize - 1)
            .and_then(Option::as_mut)
            .map(f)
            .unwrap_or(default)
    })
}

/// Creates a browser A/V encoder. Set both `audio_rate` and `audio_channels` to zero for video-only.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_create(
    width: u32,
    height: u32,
    fps_flags: u32,
    video_qstep: u32,
    preset: u32,
    epoch_frames: u32,
    meta0: u32,
    audio_rate: u32,
    audio_channels: u32,
    audio_qstep: u32,
) -> u32 {
    let Ok(video_qstep) = u16::try_from(video_qstep) else {
        return fail_av_create("video quantizer step must fit u16");
    };
    let Ok(audio_qstep) = u16::try_from(audio_qstep) else {
        return fail_av_create("audio quantizer step must fit u16");
    };
    let preset = match preset {
        0 => EncoderPreset::Fast,
        1 => EncoderPreset::Balanced,
        2 => EncoderPreset::Quality,
        _ => return fail_av_create("unknown browser encoder preset"),
    };
    let state = match AvEncoderState::new(
        width,
        height,
        fps_flags,
        video_qstep,
        preset,
        u64::from(epoch_frames),
        meta0,
        audio_rate,
        audio_channels,
        audio_qstep,
    ) {
        Ok(state) => state,
        Err(error) => return fail_av_create(error),
    };
    AV_CREATE_ERROR.with(|error| error.borrow_mut().clear());
    AV_ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        if let Some((index, slot)) = encoders
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.is_none())
        {
            *slot = Some(state);
            return (index + 1).try_into().unwrap_or(0);
        }
        encoders.push(Some(state));
        encoders.len().try_into().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_create_error_ptr() -> *const u8 {
    AV_CREATE_ERROR.with(|error| error.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_create_error_len() -> u32 {
    AV_CREATE_ERROR.with(|error| error.borrow().len().try_into().unwrap_or(0))
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_destroy(handle: u32) -> i32 {
    if handle == 0 {
        return -1;
    }
    AV_ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        let Some(slot) = encoders.get_mut(handle as usize - 1) else {
            return -1;
        };
        if slot.take().is_some() { 0 } else { -1 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_video_frame_len(handle: u32) -> u32 {
    with_av_encoder(handle, 0, |encoder| {
        encoder.video_input.len().try_into().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_video_frame_ptr(handle: u32) -> *mut u8 {
    with_av_encoder(handle, std::ptr::null_mut(), |encoder| {
        encoder.video_input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_push_video_frame(handle: u32) -> i32 {
    with_av_encoder(handle, -1, |encoder| match encoder.push_video_frame() {
        Ok(()) => 0,
        Err(error) => encoder.fail(error),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_audio_reserve(handle: u32, sample_count: u32) -> *mut i16 {
    with_av_encoder(handle, std::ptr::null_mut(), |encoder| {
        match encoder.reserve_audio(sample_count as usize) {
            Ok(ptr) => ptr,
            Err(error) => {
                encoder.fail(error);
                std::ptr::null_mut()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_push_audio(handle: u32, sample_count: u32) -> i32 {
    with_av_encoder(handle, -1, |encoder| {
        match encoder.push_audio(sample_count as usize) {
            Ok(()) => 0,
            Err(error) => encoder.fail(error),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_finish(handle: u32) -> i32 {
    with_av_encoder(handle, -1, |encoder| match encoder.finish() {
        Ok(()) => 0,
        Err(error) => encoder.fail(error),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_output_ptr(handle: u32) -> *const u8 {
    with_av_encoder(handle, std::ptr::null(), |encoder| encoder.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_output_len(handle: u32) -> u32 {
    with_av_encoder(handle, 0, |encoder| {
        encoder.output.len().try_into().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_last_error_ptr(handle: u32) -> *const u8 {
    with_av_encoder(handle, std::ptr::null(), |encoder| encoder.error.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn av_encoder_last_error_len(handle: u32) -> u32 {
    with_av_encoder(handle, 0, |encoder| {
        encoder.error.len().try_into().unwrap_or(0)
    })
}

#[derive(Debug)]
struct StreamingAvEncoderState {
    width: u32,
    height: u32,
    fps_n: u32,
    fps_d: u32,
    meta0: u32,
    frame_us: u64,
    epoch_frames: u64,
    video_options: EncodeOptions,
    video_encoder: VideoEncoder,
    frame_index: u64,
    epoch_start_frame: u64,
    epoch_id: u32,
    pending_video: Vec<EncodedFrame>,
    frame_bytes: usize,
    video_input: Vec<u8>,
    audio_rate: u32,
    audio_channels: u8,
    audio_qstep: u16,
    audio_staging: Vec<i16>,
    audio_pending: Vec<i16>,
    audio_base_frame: u64,
    audio_total_frames: u64,
    ready_epochs: VecDeque<Vec<u8>>,
    epoch_records: Vec<(u32, u64, u32, u64)>,
    transfer: Vec<u8>,
    prefix: Vec<u8>,
    finished: bool,
    error: String,
}

impl StreamingAvEncoderState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        width: u32,
        height: u32,
        fps_flags: u32,
        video_qstep: u16,
        preset: EncoderPreset,
        epoch_frames: u64,
        meta0: u32,
        audio_rate: u32,
        audio_channels: u32,
        audio_qstep: u16,
    ) -> Result<Self, String> {
        // Reuse the established browser A/V encoder's validation and initial codec setup so the
        // streaming ABI cannot silently diverge in accepted configuration or metadata semantics.
        let base = AvEncoderState::new(
            width,
            height,
            fps_flags,
            video_qstep,
            preset,
            epoch_frames,
            meta0,
            audio_rate,
            audio_channels,
            audio_qstep,
        )?;
        Ok(Self {
            width: base.width,
            height: base.height,
            fps_n: base.fps_n,
            fps_d: base.fps_d,
            meta0: base.meta0,
            frame_us: base.frame_us,
            epoch_frames: base.epoch_frames,
            video_options: base.video_options,
            video_encoder: base.video_encoder,
            frame_index: 0,
            epoch_start_frame: 0,
            epoch_id: 0,
            pending_video: Vec::new(),
            frame_bytes: base.frame_bytes,
            video_input: base.video_input,
            audio_rate: base.audio_rate,
            audio_channels: base.audio_channels,
            audio_qstep: base.audio_qstep,
            audio_staging: Vec::new(),
            audio_pending: Vec::new(),
            audio_base_frame: 0,
            audio_total_frames: 0,
            ready_epochs: VecDeque::new(),
            epoch_records: Vec::new(),
            transfer: Vec::new(),
            prefix: Vec::new(),
            finished: false,
            error: String::new(),
        })
    }

    fn audio_enabled(&self) -> bool {
        self.audio_rate != 0
    }

    fn fail(&mut self, error: impl std::fmt::Display) -> i32 {
        self.error = error.to_string();
        -1
    }

    fn push_video_frame(&mut self) -> Result<(), String> {
        if self.finished {
            return Err("encoder is already finished".into());
        }
        if self.frame_index > 0 && self.frame_index.is_multiple_of(self.epoch_frames) {
            self.video_encoder = VideoEncoder::new(self.video_options);
        }
        let input = std::mem::take(&mut self.video_input);
        let frame = match Frame420::from_tightly_packed(self.width, self.height, input) {
            Ok(frame) => frame,
            Err(error) => {
                self.video_input = vec![0; self.frame_bytes];
                return Err(error.to_string());
            }
        };
        let frame_id = self.frame_index;
        let encoded = self.video_encoder.encode_shared(frame_id, &frame);
        self.video_input = frame.into_tightly_packed();
        let encoded = encoded.map_err(|e| e.to_string())?;
        let pts = frame_id
            .checked_mul(self.frame_us)
            .ok_or_else(|| "frame timestamp overflow".to_string())?;
        self.pending_video.push(EncodedFrame {
            frame_id,
            pts,
            duration: self.frame_us as u32,
            payload: encoded.packet,
        });
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or_else(|| "frame index overflow".to_string())?;
        self.try_finish_full_epochs()
    }

    fn reserve_audio(&mut self, sample_count: usize) -> Result<*mut i16, String> {
        if self.finished {
            return Err("encoder is already finished".into());
        }
        if !self.audio_enabled() {
            return Err("audio is disabled for this encoder".into());
        }
        // This is a staging buffer, not media retention. Keep a hard per-call bound so a malformed
        // JS caller cannot turn the streaming path back into an unbounded allocation.
        let max_samples = 16usize * 1024 * 1024 / std::mem::size_of::<i16>();
        if sample_count > max_samples {
            return Err("one streaming audio staging request exceeds 16 MiB".into());
        }
        self.audio_staging.resize(sample_count, 0);
        Ok(self.audio_staging.as_mut_ptr())
    }

    fn push_audio(&mut self, sample_count: usize) -> Result<(), String> {
        if !self.audio_enabled() {
            return Err("audio is disabled for this encoder".into());
        }
        if sample_count != self.audio_staging.len() {
            return Err("audio push length does not match the current staging buffer".into());
        }
        let channels = usize::from(self.audio_channels);
        if !sample_count.is_multiple_of(channels) {
            return Err("audio sample count is not aligned to the configured channels".into());
        }
        let frames = u64::try_from(sample_count / channels)
            .map_err(|_| "audio frame count exceeds u64".to_string())?;
        self.audio_pending.extend_from_slice(&self.audio_staging);
        self.audio_staging.clear();
        self.audio_total_frames = self
            .audio_total_frames
            .checked_add(frames)
            .ok_or_else(|| "audio frame index overflow".to_string())?;
        self.try_finish_full_epochs()
    }

    fn audio_frame_for_pts(&self, pts: u64) -> Result<u64, String> {
        pts.checked_mul(u64::from(self.audio_rate))
            .ok_or_else(|| "audio timestamp conversion overflow".to_string())
            .map(|scaled| scaled / 1_000_000)
    }

    fn try_finish_full_epochs(&mut self) -> Result<(), String> {
        loop {
            let end_frame = self
                .epoch_start_frame
                .checked_add(self.epoch_frames)
                .ok_or_else(|| "epoch frame index overflow".to_string())?;
            if self.frame_index < end_frame {
                return Ok(());
            }
            if self.audio_enabled() {
                let end_pts = end_frame
                    .checked_mul(self.frame_us)
                    .ok_or_else(|| "epoch timestamp overflow".to_string())?;
                let need_audio = self.audio_frame_for_pts(end_pts)?;
                if self.audio_total_frames < need_audio {
                    return Ok(());
                }
            }
            self.finish_epoch(end_frame, false)?;
        }
    }

    fn finish_epoch(&mut self, end_frame: u64, allow_short_audio: bool) -> Result<(), String> {
        if end_frame <= self.epoch_start_frame || end_frame > self.frame_index {
            return Err("invalid streaming epoch frame range".into());
        }
        let video_count = usize::try_from(end_frame - self.epoch_start_frame)
            .map_err(|_| "epoch frame count exceeds address space".to_string())?;
        if self.pending_video.len() < video_count {
            return Err("streaming encoder lost pending video frames".into());
        }
        let pts = self
            .epoch_start_frame
            .checked_mul(self.frame_us)
            .ok_or_else(|| "epoch timestamp overflow".to_string())?;
        let end_pts = end_frame
            .checked_mul(self.frame_us)
            .ok_or_else(|| "epoch timestamp overflow".to_string())?;
        let duration = u32::try_from(end_pts - pts)
            .map_err(|_| "epoch duration exceeds Draft Gen 1 range".to_string())?;
        let epoch_id = self.epoch_id;
        let mut packets = Vec::new();
        packets.push(Packet {
            kind: PacketKind::EpochStart,
            flags: 0,
            stream_id: 0,
            pts,
            duration,
            payload: epoch_id.to_le_bytes().to_vec(),
        });
        for frame in self.pending_video.drain(..video_count) {
            packets.push(Packet {
                kind: PacketKind::VideoFrame,
                flags: 0,
                stream_id: 1,
                pts: frame.pts,
                duration: frame.duration,
                payload: frame.payload,
            });
        }

        if self.audio_enabled() {
            let channels = usize::from(self.audio_channels);
            let start_audio = self.audio_frame_for_pts(pts)?;
            let target_end_audio = self.audio_frame_for_pts(end_pts)?;
            if !allow_short_audio && self.audio_total_frames < target_end_audio {
                return Err("streaming epoch finalized before enough audio arrived".into());
            }

            // Normally the previous epoch consumed exactly up to this boundary. During finalization
            // of a source whose audio ended early there can instead be a gap; discard any stale
            // samples and encode only the overlap that actually exists.
            if self.audio_base_frame < start_audio {
                let discard_frames = (start_audio - self.audio_base_frame).min(
                    self.audio_total_frames
                        .saturating_sub(self.audio_base_frame),
                );
                let discard_samples = usize::try_from(discard_frames)
                    .ok()
                    .and_then(|frames| frames.checked_mul(channels))
                    .ok_or_else(|| "audio discard size overflow".to_string())?;
                if discard_samples > self.audio_pending.len() {
                    return Err("streaming audio buffer accounting mismatch".into());
                }
                self.audio_pending.drain(..discard_samples);
                self.audio_base_frame += discard_frames;
            }

            let end_audio = target_end_audio.min(self.audio_total_frames);
            if end_audio > start_audio && self.audio_base_frame <= start_audio {
                let skip_frames = start_audio - self.audio_base_frame;
                let skip_samples = usize::try_from(skip_frames)
                    .ok()
                    .and_then(|frames| frames.checked_mul(channels))
                    .ok_or_else(|| "audio skip size overflow".to_string())?;
                let encode_frames = end_audio - start_audio;
                let encode_samples = usize::try_from(encode_frames)
                    .ok()
                    .and_then(|frames| frames.checked_mul(channels))
                    .ok_or_else(|| "audio epoch size overflow".to_string())?;
                let need = skip_samples
                    .checked_add(encode_samples)
                    .ok_or_else(|| "audio epoch slice overflow".to_string())?;
                if need > self.audio_pending.len() {
                    return Err("streaming audio buffer accounting mismatch".into());
                }
                let samples = &self.audio_pending[skip_samples..need];
                let mut af = start_audio;
                let mut sample_offset = 0usize;
                while af < end_audio {
                    let n = (end_audio - af).min(960);
                    let n_samples = usize::try_from(n)
                        .ok()
                        .and_then(|frames| frames.checked_mul(channels))
                        .ok_or_else(|| "audio packet size overflow".to_string())?;
                    let payload = audio::encode(
                        &samples[sample_offset..sample_offset + n_samples],
                        AudioEncodeOptions {
                            sample_rate: self.audio_rate,
                            channels: self.audio_channels,
                            qstep: self.audio_qstep,
                            mid_side: self.audio_channels == 2,
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    packets.push(Packet {
                        kind: PacketKind::AudioFrame,
                        flags: 0,
                        stream_id: 2,
                        pts: af * 1_000_000 / u64::from(self.audio_rate),
                        duration: u32::try_from(n * 1_000_000 / u64::from(self.audio_rate))
                            .map_err(|_| "audio packet duration overflow".to_string())?,
                        payload,
                    });
                    af += n;
                    sample_offset += n_samples;
                }
                self.audio_pending.drain(..need);
                self.audio_base_frame = end_audio;
            }
        }

        packets.sort_by_key(|packet| {
            (
                u8::from(packet.kind != PacketKind::EpochStart),
                packet.pts,
                match packet.kind {
                    PacketKind::VideoFrame => 0u8,
                    PacketKind::AudioFrame => 1,
                    PacketKind::Metadata => 2,
                    PacketKind::EpochStart => 0,
                },
            )
        });
        let mut bytes = Vec::new();
        for packet in packets {
            container::encode_packet_checked(&packet, &mut bytes).map_err(|e| e.to_string())?;
        }
        let len =
            u64::try_from(bytes.len()).map_err(|_| "epoch byte length exceeds u64".to_string())?;
        self.epoch_records.push((epoch_id, pts, duration, len));
        self.ready_epochs.push_back(bytes);
        self.epoch_id = self
            .epoch_id
            .checked_add(1)
            .ok_or_else(|| "epoch identifier overflow".to_string())?;
        self.epoch_start_frame = end_frame;
        Ok(())
    }

    fn build_prefix(&mut self) -> Result<(), String> {
        let mut streams = vec![StreamDesc {
            id: 1,
            kind: StreamKind::Video,
            codec: 1,
            timescale: TIMEBASE,
            param0: self.width,
            param1: self.height,
            flags: (self.fps_n << 16) | self.fps_d,
            meta0: self.meta0,
        }];
        if self.audio_enabled() {
            streams.push(StreamDesc {
                id: 2,
                kind: StreamKind::Audio,
                codec: 1,
                timescale: TIMEBASE,
                param0: self.audio_rate,
                param1: u32::from(self.audio_channels),
                flags: 0,
                meta0: 0,
            });
        }
        let dummy = self
            .epoch_records
            .iter()
            .map(|(id, pts, duration, _)| container::EpochIndex {
                id: *id,
                pts: *pts,
                duration: *duration,
                offset: 0,
                len: 0,
            })
            .collect::<Vec<_>>();
        let front0 = container::encode_front(&container::Front {
            streams: streams.clone(),
            epochs: dummy,
        });
        let mut offset = u64::try_from(container::FIXED_HEADER_LEN + front0.len())
            .map_err(|_| "container prefix length exceeds u64".to_string())?;
        let mut epochs = Vec::with_capacity(self.epoch_records.len());
        for (id, pts, duration, len) in &self.epoch_records {
            epochs.push(container::EpochIndex {
                id: *id,
                pts: *pts,
                duration: *duration,
                offset,
                len: *len,
            });
            offset = offset
                .checked_add(*len)
                .ok_or_else(|| "container output length overflow".to_string())?;
        }
        let front = container::encode_front(&container::Front {
            streams: streams.clone(),
            epochs,
        });
        if front.len() != front0.len() {
            return Err("streaming front-index size changed while assigning offsets".into());
        }
        self.prefix.clear();
        container::encode_header(
            &container::FileHeader {
                flags: 1,
                stream_count: u16::try_from(streams.len())
                    .map_err(|_| "too many browser streams".to_string())?,
                front_len: u32::try_from(front.len())
                    .map_err(|_| "front index exceeds u32".to_string())?,
            },
            &mut self.prefix,
        );
        self.prefix.extend(front);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        if self.frame_index == 0 {
            return Err("cannot finish media with no video frames".into());
        }
        // Any full epochs that could not be emitted during the run (for example because the source
        // audio ended early) are finalized here, then the final partial epoch is emitted.
        while self.epoch_start_frame < self.frame_index {
            let end_frame = self
                .epoch_start_frame
                .checked_add(self.epoch_frames)
                .ok_or_else(|| "epoch frame index overflow".to_string())?
                .min(self.frame_index);
            self.finish_epoch(end_frame, true)?;
        }
        self.build_prefix()?;
        self.finished = true;
        Ok(())
    }

    fn take_epoch(&mut self) -> bool {
        self.transfer = self.ready_epochs.pop_front().unwrap_or_default();
        !self.transfer.is_empty()
    }
}

thread_local! {
    static STREAM_AV_ENCODERS: RefCell<Vec<Option<StreamingAvEncoderState>>> = const { RefCell::new(Vec::new()) };
    static STREAM_AV_CREATE_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn fail_stream_av_create(message: impl Into<String>) -> u32 {
    STREAM_AV_CREATE_ERROR.with(|error| *error.borrow_mut() = message.into());
    0
}

fn with_stream_av_encoder<R>(
    handle: u32,
    default: R,
    f: impl FnOnce(&mut StreamingAvEncoderState) -> R,
) -> R {
    if handle == 0 {
        return default;
    }
    STREAM_AV_ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        encoders
            .get_mut(handle as usize - 1)
            .and_then(Option::as_mut)
            .map(f)
            .unwrap_or(default)
    })
}

/// Creates the bounded-intermediate browser A/V encoder used by the media importer.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_create(
    width: u32,
    height: u32,
    fps_flags: u32,
    video_qstep: u32,
    preset: u32,
    epoch_frames: u32,
    meta0: u32,
    audio_rate: u32,
    audio_channels: u32,
    audio_qstep: u32,
) -> u32 {
    let Ok(video_qstep) = u16::try_from(video_qstep) else {
        return fail_stream_av_create("video quantizer step must fit u16");
    };
    let Ok(audio_qstep) = u16::try_from(audio_qstep) else {
        return fail_stream_av_create("audio quantizer step must fit u16");
    };
    let preset = match preset {
        0 => EncoderPreset::Fast,
        1 => EncoderPreset::Balanced,
        2 => EncoderPreset::Quality,
        _ => return fail_stream_av_create("unknown browser encoder preset"),
    };
    let state = match StreamingAvEncoderState::new(
        width,
        height,
        fps_flags,
        video_qstep,
        preset,
        u64::from(epoch_frames),
        meta0,
        audio_rate,
        audio_channels,
        audio_qstep,
    ) {
        Ok(state) => state,
        Err(error) => return fail_stream_av_create(error),
    };
    STREAM_AV_CREATE_ERROR.with(|error| error.borrow_mut().clear());
    STREAM_AV_ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        if let Some((index, slot)) = encoders
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.is_none())
        {
            *slot = Some(state);
            return (index + 1).try_into().unwrap_or(0);
        }
        encoders.push(Some(state));
        encoders.len().try_into().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_create_error_ptr() -> *const u8 {
    STREAM_AV_CREATE_ERROR.with(|error| error.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_create_error_len() -> u32 {
    STREAM_AV_CREATE_ERROR.with(|error| error.borrow().len().try_into().unwrap_or(0))
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_destroy(handle: u32) -> i32 {
    if handle == 0 {
        return -1;
    }
    STREAM_AV_ENCODERS.with(|encoders| {
        let mut encoders = encoders.borrow_mut();
        let Some(slot) = encoders.get_mut(handle as usize - 1) else {
            return -1;
        };
        if slot.take().is_some() { 0 } else { -1 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_video_frame_len(handle: u32) -> u32 {
    with_stream_av_encoder(handle, 0, |encoder| {
        encoder.video_input.len().try_into().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_video_frame_ptr(handle: u32) -> *mut u8 {
    with_stream_av_encoder(handle, std::ptr::null_mut(), |encoder| {
        encoder.video_input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_push_video_frame(handle: u32) -> i32 {
    with_stream_av_encoder(handle, -1, |encoder| match encoder.push_video_frame() {
        Ok(()) => 0,
        Err(error) => encoder.fail(error),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_audio_reserve(handle: u32, sample_count: u32) -> *mut i16 {
    with_stream_av_encoder(handle, std::ptr::null_mut(), |encoder| {
        match encoder.reserve_audio(sample_count as usize) {
            Ok(ptr) => ptr,
            Err(error) => {
                encoder.fail(error);
                std::ptr::null_mut()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_push_audio(handle: u32, sample_count: u32) -> i32 {
    with_stream_av_encoder(handle, -1, |encoder| {
        match encoder.push_audio(sample_count as usize) {
            Ok(()) => 0,
            Err(error) => encoder.fail(error),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_ready_epochs(handle: u32) -> u32 {
    with_stream_av_encoder(handle, 0, |encoder| {
        encoder.ready_epochs.len().try_into().unwrap_or(u32::MAX)
    })
}

/// Moves the oldest ready epoch into the transfer buffer. Returns 1 when an epoch was moved, 0 when
/// none was ready, and -1 for an invalid handle.
#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_take_epoch(handle: u32) -> i32 {
    with_stream_av_encoder(handle, -1, |encoder| i32::from(encoder.take_epoch()))
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_epoch_ptr(handle: u32) -> *const u8 {
    with_stream_av_encoder(handle, std::ptr::null(), |encoder| {
        encoder.transfer.as_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_epoch_len(handle: u32) -> u32 {
    with_stream_av_encoder(handle, 0, |encoder| {
        encoder.transfer.len().try_into().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_finish(handle: u32) -> i32 {
    with_stream_av_encoder(handle, -1, |encoder| match encoder.finish() {
        Ok(()) => 0,
        Err(error) => encoder.fail(error),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_prefix_ptr(handle: u32) -> *const u8 {
    with_stream_av_encoder(handle, std::ptr::null(), |encoder| encoder.prefix.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_prefix_len(handle: u32) -> u32 {
    with_stream_av_encoder(handle, 0, |encoder| {
        encoder.prefix.len().try_into().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_last_error_ptr(handle: u32) -> *const u8 {
    with_stream_av_encoder(handle, std::ptr::null(), |encoder| encoder.error.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn av_stream_encoder_last_error_len(handle: u32) -> u32 {
    with_stream_av_encoder(handle, 0, |encoder| {
        encoder.error.len().try_into().unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    use super::video_encoder_pack_meta0;

    #[test]
    fn packs_browser_video_metadata_canonically() {
        assert_eq!(video_encoder_pack_meta0(0, 0), 0);
        assert_eq!(video_encoder_pack_meta0(1, 0), 1 << 13);
        assert_eq!(video_encoder_pack_meta0(2, 0), 2 << 13);
        assert_eq!(video_encoder_pack_meta0(0, 1), 1 << 12);
        assert_eq!(video_encoder_pack_meta0(2, 1), (1 << 12) | (2 << 13));
        assert_eq!(video_encoder_pack_meta0(3, 0), u32::MAX);
    }
}
