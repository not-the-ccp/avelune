//! Minimal C-style WebAssembly ABI for the Avelune browser/reference player.
//!
//! The ABI intentionally exposes caller-managed input storage and decoded plane/sample pointers
//! so JavaScript can drive the reference decoder without binding-generator glue. Pointer values are
//! valid only until the next operation that can reallocate the corresponding internal buffer.

#![deny(unsafe_op_in_unsafe_fn)]
use avelune_video_v1::Frame420;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
};
thread_local! {
 static INPUT:RefCell<Vec<u8>>=const{RefCell::new(Vec::new())};
 static VREFS:RefCell<VecDeque<(u64,Frame420)>>=const{RefCell::new(VecDeque::new())};
 static VW:Cell<u32>=const{Cell::new(0)};static VH:Cell<u32>=const{Cell::new(0)};
 static AOUT:RefCell<Vec<i16>>=const{RefCell::new(Vec::new())};
 static ARATE:Cell<u32>=const{Cell::new(0)};static ACH:Cell<u32>=const{Cell::new(0)};
 static LAST_ERROR:Cell<i32>=const{Cell::new(0)};
}
#[unsafe(no_mangle)]
/// Returns the Draft Generation 1 ABI version as `0xMMMM_mmmm`.
pub extern "C" fn avelune_version() -> u32 {
    0x0001_0000
}
#[unsafe(no_mangle)]
/// Clears video reference history, decoded audio, and the last error code.
pub extern "C" fn avelune_reset() {
    VREFS.with(|x| x.borrow_mut().clear());
    AOUT.with(|x| x.borrow_mut().clear());
    LAST_ERROR.with(|x| x.set(0));
}
#[unsafe(no_mangle)]
/// Resizes the shared input buffer and returns its linear-memory pointer.
pub extern "C" fn avelune_input_resize(len: u32) -> usize {
    INPUT.with(|x| {
        let mut v = x.borrow_mut();
        v.resize(len as usize, 0);
        v.as_mut_ptr() as usize
    })
}
fn seterr(e: i32) -> i32 {
    LAST_ERROR.with(|x| x.set(e));
    e
}
#[unsafe(no_mangle)]
/// Returns the most recent small integer ABI error code.
pub extern "C" fn avelune_last_error() -> i32 {
    LAST_ERROR.with(Cell::get)
}
#[unsafe(no_mangle)]
/// Decodes the first `len` bytes in the shared input buffer as one ALV1 frame packet.
pub extern "C" fn avelune_decode_video_input(len: u32) -> i32 {
    let data = INPUT.with(|x| {
        let v = x.borrow();
        (len as usize <= v.len()).then(|| v[..len as usize].to_vec())
    });
    let Some(data) = data else { return seterr(-1) };
    let res = VREFS.with(|h| {
        let hb = h.borrow();
        let refs: Vec<(u64, &Frame420)> = hb.iter().map(|(id, f)| (*id, f)).collect();
        avelune_video_v1::decode(&data, &refs)
    });
    match res {
        Ok((id, f, _)) => {
            VW.with(|x| x.set(f.width));
            VH.with(|x| x.set(f.height));
            VREFS.with(|h| {
                let mut h = h.borrow_mut();
                h.push_back((id, f));
                while h.len() > 4 {
                    h.pop_front();
                }
            });
            seterr(0)
        }
        Err(_) => seterr(-2),
    }
}
fn with_last_plane<T>(which: u8, f: impl FnOnce(&[u8]) -> T, empty: T) -> T {
    VREFS.with(|h| {
        let h = h.borrow();
        let Some((_, v)) = h.back() else { return empty };
        let p = match which {
            0 => &v.y,
            1 => &v.u,
            _ => &v.v,
        };
        f(p)
    })
}
#[unsafe(no_mangle)]
/// Returns the luma-plane pointer of the most recently decoded video frame.
pub extern "C" fn avelune_video_y_ptr() -> usize {
    with_last_plane(0, |x| x.as_ptr() as usize, 0)
}
#[unsafe(no_mangle)]
/// Returns the U/Cb-plane pointer of the most recently decoded video frame.
pub extern "C" fn avelune_video_u_ptr() -> usize {
    with_last_plane(1, |x| x.as_ptr() as usize, 0)
}
#[unsafe(no_mangle)]
/// Returns the V/Cr-plane pointer of the most recently decoded video frame.
pub extern "C" fn avelune_video_v_ptr() -> usize {
    with_last_plane(2, |x| x.as_ptr() as usize, 0)
}
#[unsafe(no_mangle)]
/// Returns the luma-plane byte length.
pub extern "C" fn avelune_video_y_len() -> u32 {
    with_last_plane(0, |x| x.len() as u32, 0)
}
#[unsafe(no_mangle)]
/// Returns either chroma-plane byte length.
pub extern "C" fn avelune_video_uv_len() -> u32 {
    with_last_plane(1, |x| x.len() as u32, 0)
}
#[unsafe(no_mangle)]
/// Returns the most recently decoded luma width.
pub extern "C" fn avelune_video_width() -> u32 {
    VW.with(Cell::get)
}
#[unsafe(no_mangle)]
/// Returns the most recently decoded luma height.
pub extern "C" fn avelune_video_height() -> u32 {
    VH.with(Cell::get)
}
#[unsafe(no_mangle)]
/// Decodes the first `len` bytes in the shared input buffer as one ALA1 packet.
pub extern "C" fn avelune_decode_audio_input(len: u32) -> i32 {
    let data = INPUT.with(|x| {
        let v = x.borrow();
        (len as usize <= v.len()).then(|| v[..len as usize].to_vec())
    });
    let Some(data) = data else { return seterr(-1) };
    match avelune_audio_v1::decode(&data) {
        Ok((r, c, s)) => {
            ARATE.with(|x| x.set(r));
            ACH.with(|x| x.set(c as u32));
            AOUT.with(|x| *x.borrow_mut() = s);
            seterr(0)
        }
        Err(_) => seterr(-3),
    }
}
#[unsafe(no_mangle)]
/// Returns the pointer to the most recently decoded interleaved `i16` PCM samples.
pub extern "C" fn avelune_audio_ptr() -> usize {
    AOUT.with(|x| x.borrow().as_ptr() as usize)
}
#[unsafe(no_mangle)]
/// Returns the number of interleaved `i16` values in the decoded audio buffer.
pub extern "C" fn avelune_audio_len_samples() -> u32 {
    AOUT.with(|x| x.borrow().len() as u32)
}
#[unsafe(no_mangle)]
/// Returns the decoded audio sample rate in Hz.
pub extern "C" fn avelune_audio_rate() -> u32 {
    ARATE.with(Cell::get)
}
#[unsafe(no_mangle)]
/// Returns the decoded audio channel count.
pub extern "C" fn avelune_audio_channels() -> u32 {
    ACH.with(Cell::get)
}
