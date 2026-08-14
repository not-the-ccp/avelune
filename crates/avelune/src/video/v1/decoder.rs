use super::{prediction::*, *};

fn decode_plane_single_into(
    tokens: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> Result<(), VideoError> {
    if recon.len() != w.checked_mul(h).ok_or(VideoError::BadDimensions)? {
        return Err(VideoError::PlaneLength);
    }
    recon.fill(128);
    let mut pos = 0usize;
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let mode = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)?;
            pos += 1;
            if mode == 4 {
                let n = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)? as usize;
                pos += 1;
                if n == 0 || n > 4 || tokens.len() < pos + n + 1 {
                    return Err(VideoError::BadPalette);
                }
                let colors = &tokens[pos..pos + n];
                pos += n;
                let count = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)? as usize;
                pos += 1;
                let expected = (w - bx).min(8) * (h - by).min(8);
                if count != expected {
                    return Err(VideoError::BadPalette);
                }
                let bytes = (count + 3) / 4;
                if tokens.len() < pos + bytes {
                    return Err(VideoError::UnexpectedEof);
                }
                let packed = &tokens[pos..pos + bytes];
                pos += bytes;
                let mut k = 0;
                for y in 0..8 {
                    if by + y >= h {
                        break;
                    }
                    for x in 0..8 {
                        if bx + x >= w {
                            break;
                        }
                        let ci = (packed[k / 4] >> ((k % 4) * 2)) & 3;
                        if ci as usize >= n {
                            return Err(VideoError::BadPalette);
                        }
                        recon[(by + y) * w + bx + x] = colors[ci as usize];
                        k += 1
                    }
                }
                continue;
            }
            if mode > 3 {
                return Err(VideoError::BadMode);
            }
            let (mut ri, mut dx, mut dy) = (0usize, 0i32, 0i32);
            if mode == 3 {
                ri = *tokens.get(pos).ok_or(VideoError::UnexpectedEof)? as usize;
                pos += 1;
                if ri >= refs.len() {
                    return Err(VideoError::BadMode);
                }
                dx = get_svarint_i32(tokens, &mut pos)?;
                dy = get_svarint_i32(tokens, &mut pos)?;
                if dx.unsigned_abs() > 64 || dy.unsigned_abs() > 64 {
                    return Err(VideoError::BadMode);
                }
            }
            let nz = usize::try_from(get_uvarint(tokens, &mut pos)?)
                .map_err(|_| VideoError::BadCoefficient)?;
            if nz > 64 {
                return Err(VideoError::BadCoefficient);
            }
            let mut qc = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = usize::try_from(get_uvarint(tokens, &mut pos)?)
                    .map_err(|_| VideoError::BadCoefficient)?;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(VideoError::BadCoefficient);
                }
                let v = get_svarint_i32(tokens, &mut pos)?;
                if v.unsigned_abs() > 1_000_000 {
                    return Err(VideoError::BadCoefficient);
                }
                qc[i] = v;
                last = Some(i)
            }
            if nz == 0 {
                if mode == 3 {
                    if let Some((sx, sy)) = integer_motion_origin(w, h, bx, by, dx, dy) {
                        let bw = (w - bx).min(8);
                        let bh = (h - by).min(8);
                        for y in 0..bh {
                            let src = &refs[ri][(sy + y) * w + sx..(sy + y) * w + sx + bw];
                            let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + bw];
                            dst.copy_from_slice(src)
                        }
                    } else if let Some(block) =
                        halfpel_prediction_block(refs[ri], w, h, bx, by, dx, dy, kernels)
                    {
                        for y in 0..8 {
                            let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + 8];
                            dst.copy_from_slice(&block[y * 8..y * 8 + 8]);
                        }
                    } else {
                        for y in 0..8 {
                            for x in 0..8 {
                                if bx + x >= w || by + y >= h {
                                    continue;
                                }
                                recon[(by + y) * w + bx + x] = sample_half(
                                    refs[ri],
                                    w,
                                    h,
                                    ((bx + x) as i32) * 2 + dx,
                                    ((by + y) as i32) * 2 + dy,
                                )
                            }
                        }
                    }
                } else {
                    let block = intra_prediction_block(recon, w, h, bx, by, mode);
                    let bw = (w - bx).min(8);
                    let bh = (h - by).min(8);
                    for y in 0..bh {
                        let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + bw];
                        dst.copy_from_slice(&block[y * 8..y * 8 + bw]);
                    }
                }
                continue;
            }
            let mut deq = [0i32; 64];
            for i in 0..64 {
                deq[i] = qc[i].checked_mul(q).ok_or(VideoError::BadCoefficient)?;
                if deq[i].unsigned_abs() > 33_554_431 {
                    return Err(VideoError::BadCoefficient);
                }
            }
            let rr = inv_wht2(deq, kernels);
            let fast = if mode == 3 {
                integer_motion_origin(w, h, bx, by, dx, dy)
            } else {
                None
            };
            let half_fast = if mode == 3 && fast.is_none() {
                halfpel_prediction_block(refs[ri], w, h, bx, by, dx, dy, kernels)
            } else {
                None
            };
            let intra_fast = if mode == 3 {
                None
            } else {
                Some(intra_prediction_block(recon, w, h, bx, by, mode))
            };
            for y in 0..8 {
                for x in 0..8 {
                    if bx + x >= w || by + y >= h {
                        continue;
                    }
                    let pred = if mode == 3 {
                        if let Some((sx, sy)) = fast {
                            refs[ri][(sy + y) * w + sx + x]
                        } else if let Some(block) = &half_fast {
                            block[y * 8 + x]
                        } else {
                            sample_half(
                                refs[ri],
                                w,
                                h,
                                ((bx + x) as i32) * 2 + dx,
                                ((by + y) as i32) * 2 + dy,
                            )
                        }
                    } else if let Some(block) = &intra_fast {
                        block[y * 8 + x]
                    } else {
                        unreachable!("intra prediction is materialized for non-inter modes")
                    };
                    recon[(by + y) * w + bx + x] = clip8(i32::from(pred) + rr[y * 8 + x]);
                }
            }
        }
    }
    if pos != tokens.len() {
        return Err(VideoError::TrailingData);
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn decode_plane_single(
    tokens: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
    kernels: crate::kernels::KernelSet,
) -> Result<Vec<u8>, VideoError> {
    let mut recon = vec![0u8; w.checked_mul(h).ok_or(VideoError::BadDimensions)?];
    decode_plane_single_into(tokens, refs, w, h, q, kernels, &mut recon)?;
    Ok(recon)
}

fn decode_plane_into(
    control: &[u8],
    data: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    q: i32,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> Result<(), VideoError> {
    if recon.len() != w.checked_mul(h).ok_or(VideoError::BadDimensions)? {
        return Err(VideoError::PlaneLength);
    }
    recon.fill(128);
    let mut cp = 0usize;
    let mut dp = 0usize;
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let mode = *control.get(cp).ok_or(VideoError::UnexpectedEof)?;
            cp += 1;
            if mode == 4 {
                let n = *data.get(dp).ok_or(VideoError::UnexpectedEof)? as usize;
                dp += 1;
                if n == 0 || n > 4 || data.len() < dp + n + 1 {
                    return Err(VideoError::BadPalette);
                }
                let colors = &data[dp..dp + n];
                dp += n;
                let count = *data.get(dp).ok_or(VideoError::UnexpectedEof)? as usize;
                dp += 1;
                let expected = (w - bx).min(8) * (h - by).min(8);
                if count != expected {
                    return Err(VideoError::BadPalette);
                }
                let bytes = (count + 3) / 4;
                if data.len() < dp + bytes {
                    return Err(VideoError::UnexpectedEof);
                }
                let packed = &data[dp..dp + bytes];
                dp += bytes;
                let mut k = 0;
                for y in 0..8 {
                    if by + y >= h {
                        break;
                    }
                    for x in 0..8 {
                        if bx + x >= w {
                            break;
                        }
                        let ci = (packed[k / 4] >> ((k % 4) * 2)) & 3;
                        if ci as usize >= n {
                            return Err(VideoError::BadPalette);
                        }
                        recon[(by + y) * w + bx + x] = colors[ci as usize];
                        k += 1;
                    }
                }
                continue;
            }
            if mode > 3 {
                return Err(VideoError::BadMode);
            }
            let (mut ri, mut dx, mut dy) = (0usize, 0i32, 0i32);
            if mode == 3 {
                ri = *data.get(dp).ok_or(VideoError::UnexpectedEof)? as usize;
                dp += 1;
                if ri >= refs.len() {
                    return Err(VideoError::BadMode);
                }
                dx = get_svarint_i32(data, &mut dp)?;
                dy = get_svarint_i32(data, &mut dp)?;
                if dx.unsigned_abs() > 64 || dy.unsigned_abs() > 64 {
                    return Err(VideoError::BadMode);
                }
            }
            let nz = usize::try_from(get_uvarint(data, &mut dp)?)
                .map_err(|_| VideoError::BadCoefficient)?;
            if nz > 64 {
                return Err(VideoError::BadCoefficient);
            }
            let mut qc = [0i32; 64];
            let mut last = None;
            for _ in 0..nz {
                let i = usize::try_from(get_uvarint(data, &mut dp)?)
                    .map_err(|_| VideoError::BadCoefficient)?;
                if i >= 64 || last.is_some_and(|x| i <= x) {
                    return Err(VideoError::BadCoefficient);
                }
                let v = get_svarint_i32(data, &mut dp)?;
                if v.unsigned_abs() > 1_000_000 {
                    return Err(VideoError::BadCoefficient);
                }
                qc[i] = v;
                last = Some(i);
            }
            if nz == 0 {
                if mode == 3 {
                    if let Some((sx, sy)) = integer_motion_origin(w, h, bx, by, dx, dy) {
                        let bw = (w - bx).min(8);
                        let bh = (h - by).min(8);
                        for y in 0..bh {
                            let src = &refs[ri][(sy + y) * w + sx..(sy + y) * w + sx + bw];
                            let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + bw];
                            dst.copy_from_slice(src)
                        }
                    } else if let Some(block) =
                        halfpel_prediction_block(refs[ri], w, h, bx, by, dx, dy, kernels)
                    {
                        for y in 0..8 {
                            let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + 8];
                            dst.copy_from_slice(&block[y * 8..y * 8 + 8]);
                        }
                    } else {
                        for y in 0..8 {
                            for x in 0..8 {
                                if bx + x >= w || by + y >= h {
                                    continue;
                                }
                                recon[(by + y) * w + bx + x] = sample_half(
                                    refs[ri],
                                    w,
                                    h,
                                    ((bx + x) as i32) * 2 + dx,
                                    ((by + y) as i32) * 2 + dy,
                                )
                            }
                        }
                    }
                } else {
                    let block = intra_prediction_block(recon, w, h, bx, by, mode);
                    let bw = (w - bx).min(8);
                    let bh = (h - by).min(8);
                    for y in 0..bh {
                        let dst = &mut recon[(by + y) * w + bx..(by + y) * w + bx + bw];
                        dst.copy_from_slice(&block[y * 8..y * 8 + bw]);
                    }
                }
                continue;
            }
            let mut deq = [0i32; 64];
            for i in 0..64 {
                deq[i] = qc[i].checked_mul(q).ok_or(VideoError::BadCoefficient)?;
                if deq[i].unsigned_abs() > 33_554_431 {
                    return Err(VideoError::BadCoefficient);
                }
            }
            let rr = inv_wht2(deq, kernels);
            let fast = if mode == 3 {
                integer_motion_origin(w, h, bx, by, dx, dy)
            } else {
                None
            };
            let half_fast = if mode == 3 && fast.is_none() {
                halfpel_prediction_block(refs[ri], w, h, bx, by, dx, dy, kernels)
            } else {
                None
            };
            let intra_fast = if mode == 3 {
                None
            } else {
                Some(intra_prediction_block(recon, w, h, bx, by, mode))
            };
            for y in 0..8 {
                for x in 0..8 {
                    if bx + x >= w || by + y >= h {
                        continue;
                    }
                    let pred = if mode == 3 {
                        if let Some((sx, sy)) = fast {
                            refs[ri][(sy + y) * w + sx + x]
                        } else if let Some(block) = &half_fast {
                            block[y * 8 + x]
                        } else {
                            sample_half(
                                refs[ri],
                                w,
                                h,
                                ((bx + x) as i32) * 2 + dx,
                                ((by + y) as i32) * 2 + dy,
                            )
                        }
                    } else if let Some(block) = &intra_fast {
                        block[y * 8 + x]
                    } else {
                        unreachable!("intra prediction is materialized for non-inter modes")
                    };
                    recon[(by + y) * w + bx + x] = clip8(i32::from(pred) + rr[y * 8 + x]);
                }
            }
        }
    }
    if cp != control.len() || dp != data.len() {
        return Err(VideoError::TrailingData);
    }
    Ok(())
}

#[derive(Debug, Default)]
pub(super) struct PlaneEntropyScratch {
    control_or_single: EntropyScratch,
    data: EntropyScratch,
}

#[derive(Clone, Copy)]
enum PlaneEnvelope<'a> {
    Single(&'a [u8]),
    Split(&'a [u8], &'a [u8]),
}

fn parse_plane_envelopes<'a>(
    input: &'a [u8],
    pos: &mut usize,
) -> Result<[PlaneEnvelope<'a>; 3], VideoError> {
    let mut planes = Vec::with_capacity(3);
    for _ in 0..3 {
        let layout = *input.get(*pos).ok_or(VideoError::UnexpectedEof)?;
        *pos += 1;
        if input.len() < *pos + 4 {
            return Err(VideoError::UnexpectedEof);
        }
        let first = u32::from_le_bytes(input[*pos..*pos + 4].try_into().unwrap()) as usize;
        *pos += 4;
        let first_end = pos.checked_add(first).ok_or(VideoError::OutputTooLarge)?;
        if first_end > input.len() {
            return Err(VideoError::UnexpectedEof);
        }
        let first_bytes = &input[*pos..first_end];
        *pos = first_end;
        match layout {
            0 => planes.push(PlaneEnvelope::Single(first_bytes)),
            1 => {
                if input.len() < *pos + 4 {
                    return Err(VideoError::UnexpectedEof);
                }
                let second = u32::from_le_bytes(input[*pos..*pos + 4].try_into().unwrap()) as usize;
                *pos += 4;
                let second_end = pos.checked_add(second).ok_or(VideoError::OutputTooLarge)?;
                if second_end > input.len() {
                    return Err(VideoError::UnexpectedEof);
                }
                let second_bytes = &input[*pos..second_end];
                *pos = second_end;
                planes.push(PlaneEnvelope::Split(first_bytes, second_bytes));
            }
            _ => return Err(VideoError::BadHeader),
        }
    }
    planes.try_into().map_err(|_| VideoError::BadHeader)
}

fn decode_one_plane_into(
    p: usize,
    envelope: PlaneEnvelope<'_>,
    refs: &[&Frame420],
    w: u32,
    h: u32,
    q: u16,
    scratch: &mut PlaneEntropyScratch,
    kernels: crate::kernels::KernelSet,
    max_entropy_bytes: usize,
    recon: &mut [u8],
) -> Result<(), VideoError> {
    let (pw, ph) = if p == 0 {
        (w as usize, h as usize)
    } else {
        (w as usize / 2, h as usize / 2)
    };
    let rplanes: Vec<&[u8]> = refs.iter().map(|r| plane(r, p)).collect();
    match envelope {
        PlaneEnvelope::Single(bytes) => {
            let tokens = entropy_decompress_with_scratch(
                bytes,
                max_entropy_bytes,
                &mut scratch.control_or_single,
            )?;
            decode_plane_single_into(tokens, &rplanes, pw, ph, i32::from(q), kernels, recon)
        }
        PlaneEnvelope::Split(control_bytes, data_bytes) => {
            let control = entropy_decompress_with_scratch(
                control_bytes,
                max_entropy_bytes,
                &mut scratch.control_or_single,
            )?;
            let data =
                entropy_decompress_with_scratch(data_bytes, max_entropy_bytes, &mut scratch.data)?;
            decode_plane_into(
                control,
                data,
                &rplanes,
                pw,
                ph,
                i32::from(q),
                kernels,
                recon,
            )
        }
    }
}

/// Decodes one ALV1 frame packet using the supplied immutable reference pictures.
pub fn decode(
    input: &[u8],
    references: &[(u64, &Frame420)],
) -> Result<(u64, Frame420, Vec<u64>), VideoError> {
    let mut scratch: [PlaneEntropyScratch; 3] =
        std::array::from_fn(|_| PlaneEntropyScratch::default());
    decode_with_threads(
        input,
        references,
        crate::config::ThreadPolicy::Auto,
        &mut scratch,
        crate::kernels::KernelSet::auto(),
        None,
        None,
        crate::limits::Limits::default().max_frame_pixels,
        crate::limits::Limits::default().max_entropy_bytes,
    )
}

pub(super) fn decode_with_threads(
    input: &[u8],
    references: &[(u64, &Frame420)],
    thread_policy: crate::config::ThreadPolicy,
    scratch: &mut [PlaneEntropyScratch; 3],
    kernels: crate::kernels::KernelSet,
    scheduler: Option<&crate::scheduler::Scheduler>,
    frame_pool: Option<&mut Vec<Frame420>>,
    max_frame_pixels: u64,
    max_entropy_bytes: usize,
) -> Result<(u64, Frame420, Vec<u64>), VideoError> {
    if input.len() < 20 {
        return Err(VideoError::UnexpectedEof);
    }
    if input[..4] != CODEC_MAGIC {
        return Err(VideoError::BadHeader);
    }
    let frame_id = u64::from_le_bytes(input[4..12].try_into().unwrap());
    let w = u16::from_le_bytes(input[12..14].try_into().unwrap()) as u32;
    let h = u16::from_le_bytes(input[14..16].try_into().unwrap()) as u32;
    let q = u16::from_le_bytes(input[16..18].try_into().unwrap());
    if q == 0 {
        return Err(VideoError::BadQuantizer);
    }
    sizes(w, h)?;
    if u64::from(w) * u64::from(h) > max_frame_pixels {
        return Err(VideoError::OutputTooLarge);
    }
    let rc = input[18] as usize;
    let flags = input[19];
    if rc > 4 || flags != 1 {
        return Err(VideoError::BadHeader);
    }
    let mut pos = 20usize;
    let mut dep = Vec::with_capacity(rc);
    let mut refs = Vec::with_capacity(rc);
    for _ in 0..rc {
        if input.len() < pos + 8 {
            return Err(VideoError::UnexpectedEof);
        }
        let id = u64::from_le_bytes(input[pos..pos + 8].try_into().unwrap());
        pos += 8;
        if dep.contains(&id) || id == frame_id {
            return Err(VideoError::BadHeader);
        }
        let r = references
            .iter()
            .find(|(rid, _)| *rid == id)
            .ok_or(VideoError::ReferenceMissing(id))?
            .1;
        if r.width != w || r.height != h {
            return Err(VideoError::ReferenceShape);
        }
        dep.push(id);
        refs.push(r);
    }

    // Validate and discover all plane envelopes before entropy allocation/reconstruction. This
    // makes attacker-controlled lengths cheap to reject and exposes three independent jobs.
    let envelopes = parse_plane_envelopes(input, &mut pos)?;
    if pos != input.len() {
        return Err(VideoError::TrailingData);
    }

    let [scratch0, scratch1, scratch2] = scratch;
    let mut frame = if let Some(pool) = frame_pool {
        if let Some(index) = pool.iter().position(|f| f.width == w && f.height == h) {
            pool.swap_remove(index)
        } else {
            Frame420::new(w, h)?
        }
    } else {
        Frame420::new(w, h)?
    };
    {
        let crate::buffer::Frame420ViewMut {
            mut y,
            mut u,
            mut v,
        } = frame.view_mut();
        let y = y.contiguous_mut().ok_or(VideoError::PlaneLength)?;
        let u = u.contiguous_mut().ok_or(VideoError::PlaneLength)?;
        let v = v.contiguous_mut().ok_or(VideoError::PlaneLength)?;
        if parallel_planes(w, h, thread_policy) {
            if let Some(scheduler) = scheduler {
                let (yr, ur, vr) = scheduler.run_three(
                    || {
                        decode_one_plane_into(
                            0,
                            envelopes[0],
                            &refs,
                            w,
                            h,
                            q,
                            scratch0,
                            kernels,
                            max_entropy_bytes,
                            y,
                        )
                    },
                    || {
                        decode_one_plane_into(
                            1,
                            envelopes[1],
                            &refs,
                            w,
                            h,
                            q,
                            scratch1,
                            kernels,
                            max_entropy_bytes,
                            u,
                        )
                    },
                    || {
                        decode_one_plane_into(
                            2,
                            envelopes[2],
                            &refs,
                            w,
                            h,
                            q,
                            scratch2,
                            kernels,
                            max_entropy_bytes,
                            v,
                        )
                    },
                );
                yr?;
                ur?;
                vr?;
            } else {
                std::thread::scope(|scope| {
                    let h0 = scope.spawn(|| {
                        decode_one_plane_into(
                            0,
                            envelopes[0],
                            &refs,
                            w,
                            h,
                            q,
                            scratch0,
                            kernels,
                            max_entropy_bytes,
                            y,
                        )
                    });
                    let h1 = scope.spawn(|| {
                        decode_one_plane_into(
                            1,
                            envelopes[1],
                            &refs,
                            w,
                            h,
                            q,
                            scratch1,
                            kernels,
                            max_entropy_bytes,
                            u,
                        )
                    });
                    let h2 = scope.spawn(|| {
                        decode_one_plane_into(
                            2,
                            envelopes[2],
                            &refs,
                            w,
                            h,
                            q,
                            scratch2,
                            kernels,
                            max_entropy_bytes,
                            v,
                        )
                    });
                    h0.join().map_err(|_| VideoError::BadHeader)??;
                    h1.join().map_err(|_| VideoError::BadHeader)??;
                    h2.join().map_err(|_| VideoError::BadHeader)??;
                    Ok::<(), VideoError>(())
                })?;
            }
        } else {
            decode_one_plane_into(
                0,
                envelopes[0],
                &refs,
                w,
                h,
                q,
                scratch0,
                kernels,
                max_entropy_bytes,
                y,
            )?;
            decode_one_plane_into(
                1,
                envelopes[1],
                &refs,
                w,
                h,
                q,
                scratch1,
                kernels,
                max_entropy_bytes,
                u,
            )?;
            decode_one_plane_into(
                2,
                envelopes[2],
                &refs,
                w,
                h,
                q,
                scratch2,
                kernels,
                max_entropy_bytes,
                v,
            )?;
        }
    }
    Ok((frame_id, frame, dep))
}
