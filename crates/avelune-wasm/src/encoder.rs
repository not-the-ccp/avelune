use avelune::{
    container::v1::{
        self as container, Packet, PacketKind, StreamDesc, StreamKind, TIMEBASE, VideoMeta,
    },
    limits::Limits,
    video::v1::{EncodeOptions, EncoderPreset, Frame420, VideoEncoder},
};
use std::cell::RefCell;

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
