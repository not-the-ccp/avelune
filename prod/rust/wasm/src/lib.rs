//! Incremental production WebAssembly ABI for Avelune Draft Generation 1.
//!
//! The ABI uses integer handles and caller-filled input storage. No Rust raw pointer is
//! dereferenced by this crate: JavaScript writes into the returned linear-memory region,
//! then calls `decoder_push`. Pointers to popped outputs remain valid until the next pop
//! of the same media type or destruction of the handle.
#![deny(unsafe_op_in_unsafe_fn)]

use avelune_prod::{
    container::v1::{ContainerStreamDecoder, DecodedOutput},
    video::v1::Frame420,
};
use std::{cell::RefCell, collections::VecDeque, sync::Arc};

const MAX_PENDING_OUTPUTS: usize = 128;

#[derive(Debug)]
struct AudioOutput {
    pts: u64,
    duration: u32,
    rate: u32,
    channels: u8,
    pcm: Vec<i16>,
}
struct DecoderState {
    decoder: ContainerStreamDecoder,
    video_queue: VecDeque<(u64, u64, Arc<Frame420>)>,
    audio_queue: VecDeque<AudioOutput>,
    last_video: Option<(u64, u64, Arc<Frame420>)>,
    last_audio: Option<AudioOutput>,
    error: String,
}
impl DecoderState {
    fn new() -> Self {
        Self {
            decoder: ContainerStreamDecoder::new(),
            video_queue: VecDeque::new(),
            audio_queue: VecDeque::new(),
            last_video: None,
            last_audio: None,
            error: String::new(),
        }
    }
    fn process(&mut self, len: usize) -> i32 {
        let Self {
            decoder,
            video_queue,
            audio_queue,
            error,
            ..
        } = self;
        let result = decoder.commit_reserved(len, |output| {
            match output {
                DecodedOutput::Video {
                    pts,
                    frame_id,
                    frame,
                    ..
                } => {
                    if video_queue.len() >= MAX_PENDING_OUTPUTS {
                        return Err("decoded video output queue limit exceeded");
                    }
                    video_queue.push_back((pts, frame_id, frame));
                }
                DecodedOutput::Audio {
                    pts,
                    duration,
                    sample_rate,
                    channels,
                    pcm,
                    ..
                } => {
                    if audio_queue.len() >= MAX_PENDING_OUTPUTS {
                        return Err("decoded audio output queue limit exceeded");
                    }
                    audio_queue.push_back(AudioOutput {
                        pts,
                        duration,
                        rate: sample_rate,
                        channels,
                        pcm,
                    });
                }
                DecodedOutput::EpochStart { .. } | DecodedOutput::Metadata { .. } => {}
            }
            Ok::<(), &str>(())
        });
        match result {
            Ok(()) => {
                error.clear();
                0
            }
            Err(e) => {
                *error = e.to_string();
                -1
            }
        }
    }
}

thread_local! { static HANDLES: RefCell<Vec<Option<DecoderState>>> = const { RefCell::new(Vec::new()) }; }

fn with_state<R>(handle: u32, f: impl FnOnce(&mut DecoderState) -> R) -> Option<R> {
    if handle == 0 {
        return None;
    }
    HANDLES.with(|h| {
        let mut h = h.borrow_mut();
        h.get_mut(handle as usize - 1)?.as_mut().map(f)
    })
}

/// ABI version as `0xMMMM_mmmm`.
#[unsafe(no_mangle)]
pub extern "C" fn avelune_prod_abi_version() -> u32 {
    0x0001_0000
}

/// Fixed Draft Generation 1 container header length used for the first range request.
#[unsafe(no_mangle)]
pub extern "C" fn container_fixed_header_len() -> u32 {
    32
}

/// Creates an incremental decoder and returns a nonzero handle.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_create() -> u32 {
    HANDLES.with(|h| {
        let mut h = h.borrow_mut();
        if let Some((i, slot)) = h.iter_mut().enumerate().find(|(_, x)| x.is_none()) {
            *slot = Some(DecoderState::new());
            (i + 1) as u32
        } else {
            h.push(Some(DecoderState::new()));
            h.len() as u32
        }
    })
}

/// Destroys a decoder handle. Returns zero on success.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_destroy(handle: u32) -> i32 {
    if handle == 0 {
        return -1;
    }
    HANDLES.with(|h| {
        let mut h = h.borrow_mut();
        let Some(slot) = h.get_mut(handle.saturating_sub(1) as usize) else {
            return -1;
        };
        if slot.take().is_some() { 0 } else { -1 }
    })
}

/// Reserves writable parser-tail storage and returns its linear-memory pointer.
#[unsafe(no_mangle)]
pub extern "C" fn input_reserve(handle: u32, len: u32) -> usize {
    with_state(handle, |s| match s.decoder.reserve_fragment(len as usize) {
        Ok(buf) => buf.as_mut_ptr() as usize,
        Err(e) => {
            s.error = format!("container: {e}");
            0
        }
    })
    .unwrap_or(0)
}

/// Pushes the first `len` bytes of the reserved input region through the incremental container/codec pipeline.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_push(handle: u32, len: u32) -> i32 {
    with_state(handle, |s| s.process(len as usize)).unwrap_or(-1)
}

/// Validated front-index byte length once the fixed header has been parsed, or zero.
#[unsafe(no_mangle)]
pub extern "C" fn container_front_len(handle: u32) -> u32 {
    with_state(handle, |s| {
        s.decoder.file_header().map_or(0, |h| h.front_len)
    })
    .unwrap_or(0)
}
/// Number of validated stream descriptors in the parsed front index.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_count(handle: u32) -> u32 {
    with_state(handle, |s| {
        s.decoder
            .front_index()
            .map_or(0, |f| f.streams.len() as u32)
    })
    .unwrap_or(0)
}
/// Number of validated epoch entries in the parsed front index.
#[unsafe(no_mangle)]
pub extern "C" fn container_epoch_count(handle: u32) -> u32 {
    with_state(handle, |s| {
        s.decoder.front_index().map_or(0, |f| f.epochs.len() as u32)
    })
    .unwrap_or(0)
}
fn with_stream<R: Copy>(
    handle: u32,
    index: u32,
    empty: R,
    f: impl FnOnce(&avelune_prod::container::v1::StreamDesc) -> R,
) -> R {
    with_state(handle, |s| {
        s.decoder
            .front_index()
            .and_then(|front| front.streams.get(index as usize))
            .map_or(empty, f)
    })
    .unwrap_or(empty)
}
fn with_epoch<R: Copy>(
    handle: u32,
    index: u32,
    empty: R,
    f: impl FnOnce(&avelune_prod::container::v1::EpochIndex) -> R,
) -> R {
    with_state(handle, |s| {
        s.decoder
            .front_index()
            .and_then(|front| front.epochs.get(index as usize))
            .map_or(empty, f)
    })
    .unwrap_or(empty)
}
/// Stream identifier at `index`, or zero if unavailable.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_id(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| u32::from(s.id))
}
/// Stream kind at `index`, or zero if unavailable.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_kind(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| s.kind as u32)
}
/// Stream codec generation at `index`, or zero if unavailable.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_codec(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| u32::from(s.codec))
}
/// Stream timescale at `index`, or zero if unavailable.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_timescale(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| s.timescale)
}
/// Stream parameter 0 at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_param0(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| s.param0)
}
/// Stream parameter 1 at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_param1(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| s.param1)
}
/// Stream flags at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_flags(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| s.flags)
}
/// Stream metadata word at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_stream_meta0(handle: u32, index: u32) -> u32 {
    with_stream(handle, index, 0, |s| s.meta0)
}
/// Epoch identifier at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_epoch_id(handle: u32, index: u32) -> u32 {
    with_epoch(handle, index, 0, |e| e.id)
}
/// Epoch presentation timestamp at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_epoch_pts(handle: u32, index: u32) -> u64 {
    with_epoch(handle, index, 0, |e| e.pts)
}
/// Epoch duration at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_epoch_duration(handle: u32, index: u32) -> u32 {
    with_epoch(handle, index, 0, |e| e.duration)
}
/// Epoch absolute byte offset at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_epoch_offset(handle: u32, index: u32) -> u64 {
    with_epoch(handle, index, 0, |e| e.offset)
}
/// Epoch byte length at `index`.
#[unsafe(no_mangle)]
pub extern "C" fn container_epoch_len(handle: u32, index: u32) -> u64 {
    with_epoch(handle, index, 0, |e| e.len)
}

/// Resets codec references and switches the container parser to standalone-epoch-range mode.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_seek_reset(handle: u32) -> i32 {
    with_state(handle, |s| {
        s.decoder.reset_epoch_range(None);
        s.video_queue.clear();
        s.audio_queue.clear();
        s.last_video = None;
        s.last_audio = None;
        s.error.clear();
        0
    })
    .unwrap_or(-1)
}

/// Resets for one indexed epoch range and requires its `EpochStart` payload to match `epoch_id`.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_seek_reset_epoch(handle: u32, epoch_id: u32) -> i32 {
    with_state(handle, |s| {
        s.decoder.reset_epoch_range(Some(epoch_id));
        s.video_queue.clear();
        s.audio_queue.clear();
        s.last_video = None;
        s.last_audio = None;
        s.error.clear();
        0
    })
    .unwrap_or(-1)
}

/// Pops one decoded video frame into stable getter storage; returns 1, 0 if empty, or -1 for a bad handle.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_pop_video(handle: u32) -> i32 {
    with_state(handle, |s| {
        if let Some(v) = s.video_queue.pop_front() {
            s.last_video = Some(v);
            1
        } else {
            0
        }
    })
    .unwrap_or(-1)
}
/// Pops one decoded audio packet into stable getter storage; returns 1, 0 if empty, or -1 for a bad handle.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_pop_audio(handle: u32) -> i32 {
    with_state(handle, |s| {
        if let Some(v) = s.audio_queue.pop_front() {
            s.last_audio = Some(v);
            1
        } else {
            0
        }
    })
    .unwrap_or(-1)
}

fn with_video<R: Copy>(handle: u32, f: impl FnOnce(u64, u64, &Frame420) -> R, empty: R) -> R {
    with_state(handle, |s| {
        s.last_video
            .as_ref()
            .map_or(empty, |(pts, id, v)| f(*pts, *id, v.as_ref()))
    })
    .unwrap_or(empty)
}
/// Presentation timestamp of the last popped video frame.
#[unsafe(no_mangle)]
pub extern "C" fn video_pts(handle: u32) -> u64 {
    with_video(handle, |pts, _, _| pts, 0)
}
/// Last popped video frame ID.
#[unsafe(no_mangle)]
pub extern "C" fn video_frame_id(handle: u32) -> u64 {
    with_video(handle, |_, id, _| id, 0)
}
/// Last popped video width.
#[unsafe(no_mangle)]
pub extern "C" fn video_width(handle: u32) -> u32 {
    with_video(handle, |_, _, v| v.width, 0)
}
/// Last popped video height.
#[unsafe(no_mangle)]
pub extern "C" fn video_height(handle: u32) -> u32 {
    with_video(handle, |_, _, v| v.height, 0)
}
/// Last popped Y plane pointer.
#[unsafe(no_mangle)]
pub extern "C" fn video_y_ptr(handle: u32) -> usize {
    with_video(handle, |_, _, v| v.y().as_ptr() as usize, 0)
}
/// Last popped U plane pointer.
#[unsafe(no_mangle)]
pub extern "C" fn video_u_ptr(handle: u32) -> usize {
    with_video(handle, |_, _, v| v.u().as_ptr() as usize, 0)
}
/// Last popped V plane pointer.
#[unsafe(no_mangle)]
pub extern "C" fn video_v_ptr(handle: u32) -> usize {
    with_video(handle, |_, _, v| v.v().as_ptr() as usize, 0)
}
/// Last popped Y byte length.
#[unsafe(no_mangle)]
pub extern "C" fn video_y_len(handle: u32) -> u32 {
    with_video(handle, |_, _, v| v.y().len() as u32, 0)
}
/// Last popped U/V byte length.
#[unsafe(no_mangle)]
pub extern "C" fn video_uv_len(handle: u32) -> u32 {
    with_video(handle, |_, _, v| v.u().len() as u32, 0)
}

/// Presentation timestamp of the last popped audio packet.
#[unsafe(no_mangle)]
pub extern "C" fn audio_pts(handle: u32) -> u64 {
    with_state(handle, |s| s.last_audio.as_ref().map_or(0, |a| a.pts)).unwrap_or(0)
}
/// Container duration of the last popped audio packet.
#[unsafe(no_mangle)]
pub extern "C" fn audio_duration(handle: u32) -> u32 {
    with_state(handle, |s| s.last_audio.as_ref().map_or(0, |a| a.duration)).unwrap_or(0)
}
/// Last popped audio sample rate.
#[unsafe(no_mangle)]
pub extern "C" fn audio_rate(handle: u32) -> u32 {
    with_state(handle, |s| s.last_audio.as_ref().map_or(0, |a| a.rate)).unwrap_or(0)
}
/// Last popped audio channels.
#[unsafe(no_mangle)]
pub extern "C" fn audio_channels(handle: u32) -> u32 {
    with_state(handle, |s| {
        s.last_audio.as_ref().map_or(0, |a| u32::from(a.channels))
    })
    .unwrap_or(0)
}
/// Last popped interleaved PCM pointer.
#[unsafe(no_mangle)]
pub extern "C" fn audio_ptr(handle: u32) -> usize {
    with_state(handle, |s| {
        s.last_audio.as_ref().map_or(0, |a| a.pcm.as_ptr() as usize)
    })
    .unwrap_or(0)
}
/// Last popped interleaved `i16` sample count.
#[unsafe(no_mangle)]
pub extern "C" fn audio_len_samples(handle: u32) -> u32 {
    with_state(handle, |s| {
        s.last_audio.as_ref().map_or(0, |a| a.pcm.len() as u32)
    })
    .unwrap_or(0)
}

/// Pointer to the last UTF-8 error string for this handle.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_last_error_ptr(handle: u32) -> usize {
    with_state(handle, |s| s.error.as_ptr() as usize).unwrap_or(0)
}
/// Byte length of the last UTF-8 error string for this handle.
#[unsafe(no_mangle)]
pub extern "C" fn decoder_last_error_len(handle: u32) -> u32 {
    with_state(handle, |s| s.error.len() as u32).unwrap_or(0)
}

fn halfpel_probe_inputs() -> ([u8; 16 * 9], [u8; 16 * 8]) {
    let mut reference = [0u8; 16 * 9];
    let mut source = [0u8; 16 * 8];
    for (i, v) in reference.iter_mut().enumerate() {
        *v = (i as u8).wrapping_mul(31).wrapping_add(7);
    }
    for (i, v) in source.iter_mut().enumerate() {
        *v = (i as u8).wrapping_mul(13).wrapping_add(19);
    }
    (reference, source)
}

/// Deterministic exact half-sample prediction checksum for phase `0..=3`.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_halfpel_predict_probe(phase: u32) -> u64 {
    let (reference, _) = halfpel_probe_inputs();
    let fx = (phase & 1) as u8;
    let fy = ((phase >> 1) & 1) as u8;
    let Some(predicted) =
        avelune_prod::kernels::KernelSet::auto().halfpel_predict_8x8(&reference, 16, fx, fy)
    else {
        return u64::MAX;
    };
    predicted
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as u64 + 1) * u64::from(v))
        .sum()
}

/// Deterministic exact half-sample SAD for phase `0..=3`.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_halfpel_sad_probe(phase: u32) -> u64 {
    let (reference, source) = halfpel_probe_inputs();
    let fx = (phase & 1) as u8;
    let fy = ((phase >> 1) & 1) as u8;
    avelune_prod::kernels::KernelSet::auto()
        .halfpel_sad_8x8(&source, 16, &reference, 16, fx, fy)
        .unwrap_or(u64::MAX)
}

/// Runs the selected production SAD kernel on a deterministic validation vector.
/// The result is backend-independent and exercises the scalar/SIMD dispatch selected by the artifact.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_sad_probe() -> u64 {
    let mut a = [0_u8; 2048];
    let mut b = [0_u8; 2048];
    for i in 0..a.len() {
        a[i] = (i as u8).wrapping_mul(17).wrapping_add(3);
        b[i] = (i as u8).wrapping_mul(29).wrapping_add(11);
    }
    avelune_prod::kernels::KernelSet::auto().sad(&a, &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_handle_cannot_destroy_first_decoder() {
        let handle = decoder_create();
        assert_ne!(handle, 0);
        assert_eq!(decoder_destroy(0), -1);
        assert_eq!(decoder_destroy(handle), 0);
    }
}
