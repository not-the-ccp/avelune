use super::{prediction::*, *};

#[derive(Clone, Debug)]
enum BlockRec {
    Residual {
        mode: u8,
        ref_idx: usize,
        dx2: i32,
        dy2: i32,
        qcoeff: [i32; 64],
    },
    Palette {
        colors: Vec<u8>,
        idx: Vec<u8>,
    },
}

fn palette_for(src: &[u8], w: usize, h: usize, bx: usize, by: usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut colors = Vec::new();
    let mut idx = Vec::new();
    for y in 0..BLOCK {
        if by + y >= h {
            break;
        }
        for x in 0..BLOCK {
            if bx + x >= w {
                break;
            }
            let v = src[(by + y) * w + bx + x];
            let k = match colors.iter().position(|&c| c == v) {
                Some(k) => k,
                None => {
                    if colors.len() == 4 {
                        return None;
                    }
                    colors.push(v);
                    colors.len() - 1
                }
            };
            idx.push(k as u8);
        }
    }
    if idx.len() >= 16 {
        Some((colors, idx))
    } else {
        None
    }
}

fn uvarint_len(mut value: u64) -> usize {
    let mut len = 1usize;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

fn svarint_i32_len(value: i32) -> usize {
    let zigzag = ((value << 1) ^ (value >> 31)) as u32;
    uvarint_len(u64::from(zigzag))
}

fn palette_raw_rate(colors: &[u8], idx: &[u8]) -> usize {
    // one control byte plus data lane: count, colors, sample count, packed 2-bit indices
    1 + 1 + colors.len() + 1 + idx.len().div_ceil(4)
}

#[derive(Clone)]
struct ResidualEval {
    mode: u8,
    ref_idx: usize,
    dx2: i32,
    dy2: i32,
    qcoeff: [i32; 64],
    samples: [u8; 64],
    distortion: u64,
    raw_rate: usize,
}

fn prediction_sample(
    recon: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    x: usize,
    y: usize,
    mode: u8,
    ref_idx: usize,
    dx2: i32,
    dy2: i32,
) -> u8 {
    if mode == 3 {
        sample_half(
            refs[ref_idx],
            w,
            h,
            ((bx + x) as i32) * 2 + dx2,
            ((by + y) as i32) * 2 + dy2,
        )
    } else {
        intra_sample(recon, w, h, bx, by, x, y, mode)
    }
}

fn evaluate_residual_candidate(
    src: &[u8],
    recon: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
    mode: u8,
    ref_idx: usize,
    dx2: i32,
    dy2: i32,
    q: i32,
    kernels: crate::kernels::KernelSet,
) -> ResidualEval {
    let mut residual = [0i32; 64];
    let mut prediction = [128u8; 64];
    let inter_prediction = if mode == 3 {
        halfpel_prediction_block(refs[ref_idx], w, h, bx, by, dx2, dy2, kernels)
    } else {
        None
    };
    let intra_prediction = if mode == 3 {
        None
    } else {
        Some(intra_prediction_block(recon, w, h, bx, by, mode))
    };
    for y in 0..BLOCK {
        for x in 0..BLOCK {
            let i = y * 8 + x;
            if bx + x >= w || by + y >= h {
                continue;
            }
            let pred = if let Some(block) = &inter_prediction {
                block[i]
            } else if let Some(block) = &intra_prediction {
                block[i]
            } else {
                prediction_sample(recon, refs, w, h, bx, by, x, y, mode, ref_idx, dx2, dy2)
            };
            prediction[i] = pred;
            residual[i] = i32::from(src[(by + y) * w + bx + x]) - i32::from(pred);
        }
    }
    let coeff = wht2(residual);
    let mut qcoeff = [0i32; 64];
    for i in 0..64 {
        qcoeff[i] = div_round(coeff[i], q);
    }
    let mut samples = [128u8; 64];
    let mut distortion = 0u64;
    if q == 1 {
        // The integer WHT pair is exactly reversible at q=1. Every residual predictor
        // reconstructs the source block identically, so avoid an unnecessary inverse transform
        // while rate-distortion selection compares syntax cost.
        for y in 0..BLOCK {
            for x in 0..BLOCK {
                if bx + x < w && by + y < h {
                    samples[y * 8 + x] = src[(by + y) * w + bx + x];
                }
            }
        }
    } else {
        let mut deq = [0i32; 64];
        for i in 0..64 {
            deq[i] = qcoeff[i] * q;
        }
        let reconstructed_residual = inv_wht2(deq, kernels);
        for y in 0..BLOCK {
            for x in 0..BLOCK {
                let i = y * 8 + x;
                if bx + x >= w || by + y >= h {
                    continue;
                }
                let reconstructed = clip8(i32::from(prediction[i]) + reconstructed_residual[i]);
                samples[i] = reconstructed;
                let delta = i32::from(src[(by + y) * w + bx + x]) - i32::from(reconstructed);
                distortion += u64::from(delta.unsigned_abs()).pow(2);
            }
        }
    }
    let nz = qcoeff.iter().filter(|&&v| v != 0).count();
    let mut raw_rate = 1 + uvarint_len(nz as u64); // control mode + nz
    if mode == 3 {
        raw_rate += 1 + svarint_i32_len(dx2) + svarint_i32_len(dy2);
    }
    for (i, &v) in qcoeff.iter().enumerate() {
        if v != 0 {
            raw_rate += uvarint_len(i as u64) + svarint_i32_len(v);
        }
    }
    ResidualEval {
        mode,
        ref_idx,
        dx2,
        dy2,
        qcoeff,
        samples,
        distortion,
        raw_rate,
    }
}

fn apply_residual_candidate(
    eval: &ResidualEval,
    recon: &mut [u8],
    w: usize,
    h: usize,
    bx: usize,
    by: usize,
) {
    for y in 0..BLOCK {
        for x in 0..BLOCK {
            if bx + x >= w || by + y >= h {
                continue;
            }
            recon[(by + y) * w + bx + x] = eval.samples[y * 8 + x];
        }
    }
}

fn rdo_lambda(q: i32) -> u64 {
    // Deliberately conservative. Raw syntax bytes are only a proxy for post-rANS rate, so rate
    // nudges decisions rather than overwhelming sample-domain distortion at lossy quantizers.
    ((i64::from(q) * i64::from(q)) / 512).max(1) as u64
}

fn encode_plane(
    src: &[u8],
    refs: &[&[u8]],
    w: usize,
    h: usize,
    opt: EncodeOptions,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> (Vec<BlockRec>, Vec<bool>) {
    debug_assert_eq!(recon.len(), w * h);
    recon.fill(128);
    let mut blocks = Vec::new();
    let mut used = vec![false; refs.len()];
    let q = i32::from(opt.qstep.max(1));
    let lambda = rdo_lambda(q);
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            let palette = if opt.allow_palette {
                palette_for(src, w, h, bx, by)
            } else {
                None
            };

            let mut best_intra = (sad_intra(src, recon, w, h, bx, by, 0), 0u8);
            for mode in [1u8, 2u8] {
                let sad = sad_intra(src, recon, w, h, bx, by, mode);
                if sad < best_intra.0 {
                    best_intra = (sad, mode);
                }
            }

            let radius = i32::from(opt.motion_radius);
            let mut best_inter: Option<(u32, usize, i32, i32)> = None;
            for (ri, reference) in refs.iter().enumerate() {
                let (sad, dx2, dy2) =
                    search_inter_motion(src, reference, w, h, bx, by, radius, opt.preset, kernels);
                if best_inter.is_none_or(|current| sad < current.0) {
                    best_inter = Some((sad, ri, dx2, dy2));
                }
            }

            if matches!(opt.preset, EncoderPreset::Fast) {
                let (mode, ref_idx, dx2, dy2, best_sad) = match best_inter {
                    Some((sad, ri, dx2, dy2)) if sad < best_intra.0 => (3, ri, dx2, dy2, sad),
                    _ => (best_intra.1, 0, 0, 0, best_intra.0),
                };
                if let Some((colors, idx)) = palette {
                    let valid = ((w - bx).min(BLOCK)) * ((h - by).min(BLOCK));
                    if best_sad > (valid as u32) * 4 {
                        let mut k = 0;
                        for y in 0..BLOCK {
                            if by + y >= h {
                                break;
                            }
                            for x in 0..BLOCK {
                                if bx + x >= w {
                                    break;
                                }
                                recon[(by + y) * w + bx + x] = colors[idx[k] as usize];
                                k += 1;
                            }
                        }
                        blocks.push(BlockRec::Palette { colors, idx });
                        continue;
                    }
                }
                let eval = evaluate_residual_candidate(
                    src, recon, refs, w, h, bx, by, mode, ref_idx, dx2, dy2, q, kernels,
                );
                apply_residual_candidate(&eval, recon, w, h, bx, by);
                if eval.mode == 3 {
                    used[eval.ref_idx] = true;
                }
                blocks.push(BlockRec::Residual {
                    mode: eval.mode,
                    ref_idx: eval.ref_idx,
                    dx2: eval.dx2,
                    dy2: eval.dy2,
                    qcoeff: eval.qcoeff,
                });
                continue;
            }

            let inter_candidate = best_inter;
            let primary_is_inter = inter_candidate.is_some_and(|(sad, _, _, _)| sad < best_intra.0);
            let (primary_mode, primary_ref, primary_dx2, primary_dy2, primary_sad) =
                if let Some((sad, ri, dx2, dy2)) = inter_candidate.filter(|_| primary_is_inter) {
                    (3, ri, dx2, dy2, sad)
                } else {
                    (best_intra.1, 0, 0, 0, best_intra.0)
                };
            let mut chosen = evaluate_residual_candidate(
                src,
                recon,
                refs,
                w,
                h,
                bx,
                by,
                primary_mode,
                primary_ref,
                primary_dx2,
                primary_dy2,
                q,
                kernels,
            );
            let primary_dependency = if chosen.mode == 3 && !used[chosen.ref_idx] {
                8
            } else {
                0
            };
            let mut chosen_score = chosen.distortion.saturating_add(
                lambda.saturating_mul((chosen.raw_rate + primary_dependency) as u64),
            );

            let alternate = if primary_is_inter {
                Some((best_intra.0, best_intra.1, 0usize, 0i32, 0i32))
            } else {
                inter_candidate.map(|(sad, ri, dx2, dy2)| (sad, 3u8, ri, dx2, dy2))
            };
            if let Some((alternate_sad, mode, ri, dx2, dy2)) = alternate {
                let sad_gap = primary_sad.abs_diff(alternate_sad);
                let compare_alternate = q == 1
                    || matches!(opt.preset, EncoderPreset::Quality)
                    || sad_gap <= (q as u32).saturating_mul(2);
                if compare_alternate {
                    let alt = evaluate_residual_candidate(
                        src, recon, refs, w, h, bx, by, mode, ri, dx2, dy2, q, kernels,
                    );
                    let dependency_cost = if alt.mode == 3 && !used[alt.ref_idx] {
                        8
                    } else {
                        0
                    };
                    let score = alt.distortion.saturating_add(
                        lambda.saturating_mul((alt.raw_rate + dependency_cost) as u64),
                    );
                    if score < chosen_score {
                        chosen = alt;
                        chosen_score = score;
                    }
                }
            }

            if let Some((colors, idx)) = palette {
                let palette_score = lambda.saturating_mul(palette_raw_rate(&colors, &idx) as u64);
                if palette_score < chosen_score {
                    let mut k = 0;
                    for y in 0..BLOCK {
                        if by + y >= h {
                            break;
                        }
                        for x in 0..BLOCK {
                            if bx + x >= w {
                                break;
                            }
                            recon[(by + y) * w + bx + x] = colors[idx[k] as usize];
                            k += 1;
                        }
                    }
                    blocks.push(BlockRec::Palette { colors, idx });
                    continue;
                }
            }

            apply_residual_candidate(&chosen, recon, w, h, bx, by);
            if chosen.mode == 3 {
                used[chosen.ref_idx] = true;
            }
            blocks.push(BlockRec::Residual {
                mode: chosen.mode,
                ref_idx: chosen.ref_idx,
                dx2: chosen.dx2,
                dy2: chosen.dy2,
                qcoeff: chosen.qcoeff,
            });
        }
    }
    (blocks, used)
}

#[derive(Clone, Copy)]
enum BlockLayout {
    Single,
    Split,
}

enum SerializedBlocks {
    Single(Vec<u8>),
    Split { control: Vec<u8>, data: Vec<u8> },
}

struct BlockWriter {
    layout: BlockLayout,
    control: Vec<u8>,
    data: Vec<u8>,
}

impl BlockWriter {
    fn new(layout: BlockLayout, block_count: usize) -> Self {
        Self {
            layout,
            control: if matches!(layout, BlockLayout::Split) {
                Vec::with_capacity(block_count)
            } else {
                Vec::new()
            },
            data: Vec::new(),
        }
    }

    fn mode(&mut self, value: u8) {
        match self.layout {
            BlockLayout::Single => self.data.push(value),
            BlockLayout::Split => self.control.push(value),
        }
    }

    fn data_u8(&mut self, value: u8) {
        self.data.push(value);
    }

    fn data_extend(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    fn uvarint(&mut self, value: u64) {
        put_uvarint(value, &mut self.data);
    }

    fn svarint_i32(&mut self, value: i32) {
        put_svarint_i32(value, &mut self.data);
    }

    fn finish(self) -> SerializedBlocks {
        match self.layout {
            BlockLayout::Single => SerializedBlocks::Single(self.data),
            BlockLayout::Split => SerializedBlocks::Split {
                control: self.control,
                data: self.data,
            },
        }
    }
}

fn serialize_blocks(blocks: &[BlockRec], remap: &[usize], layout: BlockLayout) -> SerializedBlocks {
    let mut out = BlockWriter::new(layout, blocks.len());
    for block in blocks {
        match block {
            BlockRec::Palette { colors, idx } => {
                out.mode(4);
                out.data_u8(colors.len() as u8);
                out.data_extend(colors);
                out.data_u8(idx.len() as u8);
                let mut packed = 0u8;
                let mut shift = 0;
                for &value in idx {
                    packed |= (value & 3) << shift;
                    shift += 2;
                    if shift == 8 {
                        out.data_u8(packed);
                        packed = 0;
                        shift = 0;
                    }
                }
                if shift != 0 {
                    out.data_u8(packed);
                }
            }
            BlockRec::Residual {
                mode,
                ref_idx,
                dx2,
                dy2,
                qcoeff,
            } => {
                out.mode(*mode);
                if *mode == 3 {
                    out.data_u8(remap[*ref_idx] as u8);
                    out.svarint_i32(*dx2);
                    out.svarint_i32(*dy2);
                }
                let nz = qcoeff.iter().filter(|&&value| value != 0).count();
                out.uvarint(nz as u64);
                for (i, &value) in qcoeff.iter().enumerate() {
                    if value != 0 {
                        out.uvarint(i as u64);
                        out.svarint_i32(value);
                    }
                }
            }
        }
    }
    out.finish()
}

fn encode_one_plane(
    p: usize,
    frame: &Frame420,
    refs: &[(u64, &Frame420)],
    opt: EncodeOptions,
    kernels: crate::kernels::KernelSet,
    recon: &mut [u8],
) -> (Vec<BlockRec>, Vec<bool>) {
    let (w, h) = plane_dims(frame, p);
    let rp: Vec<&[u8]> = refs.iter().map(|(_, r)| plane(r, p)).collect();
    encode_plane(plane(frame, p), &rp, w, h, opt, kernels, recon)
}

/// Encodes one frame using the supplied reconstructed reference pictures.
pub fn encode(
    frame_id: u64,
    frame: &Frame420,
    references: &[(u64, &Frame420)],
    opt: EncodeOptions,
) -> Result<EncodedFrame, VideoError> {
    encode_with_threads(
        frame_id,
        frame,
        references,
        opt,
        crate::config::ThreadPolicy::Auto,
        crate::kernels::KernelSet::auto(),
        None,
        None,
    )
}

pub(super) fn encode_with_threads(
    frame_id: u64,
    frame: &Frame420,
    references: &[(u64, &Frame420)],
    opt: EncodeOptions,
    thread_policy: crate::config::ThreadPolicy,
    kernels: crate::kernels::KernelSet,
    scheduler: Option<&crate::scheduler::Scheduler>,
    frame_pool: Option<&mut Vec<Frame420>>,
) -> Result<EncodedFrame, VideoError> {
    frame.validate()?;
    if opt.qstep == 0 {
        return Err(VideoError::BadQuantizer);
    }
    if opt.motion_radius > 32 {
        return Err(VideoError::BadHeader);
    }
    let maxrefs = usize::from(opt.max_refs.min(4));
    let refs = &references[..references.len().min(maxrefs)];
    for (i, (id, r)) in refs.iter().enumerate() {
        if *id == frame_id || refs[..i].iter().any(|(seen, _)| seen == id) {
            return Err(VideoError::BadHeader);
        }
        r.validate()?;
        if r.width != frame.width || r.height != frame.height {
            return Err(VideoError::ReferenceShape);
        }
    }

    let mut reconstructed = if let Some(pool) = frame_pool {
        if let Some(i) = pool.iter().position(|candidate| {
            candidate.width == frame.width && candidate.height == frame.height
        }) {
            pool.swap_remove(i)
        } else {
            Frame420::new(frame.width, frame.height)?
        }
    } else {
        Frame420::new(frame.width, frame.height)?
    };
    let mut view = reconstructed.view_mut();
    let y = view.y.contiguous_mut().ok_or(VideoError::PlaneLength)?;
    let u = view.u.contiguous_mut().ok_or(VideoError::PlaneLength)?;
    let v = view.v.contiguous_mut().ok_or(VideoError::PlaneLength)?;
    let (p0, p1, p2) = if parallel_planes(frame.width, frame.height, thread_policy) {
        if let Some(scheduler) = scheduler {
            scheduler.run_three(
                || encode_one_plane(0, frame, refs, opt, kernels, y),
                || encode_one_plane(1, frame, refs, opt, kernels, u),
                || encode_one_plane(2, frame, refs, opt, kernels, v),
            )
        } else {
            std::thread::scope(|scope| {
                let h0 = scope.spawn(|| encode_one_plane(0, frame, refs, opt, kernels, y));
                let h1 = scope.spawn(|| encode_one_plane(1, frame, refs, opt, kernels, u));
                let h2 = scope.spawn(|| encode_one_plane(2, frame, refs, opt, kernels, v));
                let a = match h0.join() {
                    Ok(value) => value,
                    Err(payload) => std::panic::resume_unwind(payload),
                };
                let b = match h1.join() {
                    Ok(value) => value,
                    Err(payload) => std::panic::resume_unwind(payload),
                };
                let c = match h2.join() {
                    Ok(value) => value,
                    Err(payload) => std::panic::resume_unwind(payload),
                };
                Ok::<_, VideoError>((a, b, c))
            })?
        }
    } else {
        (
            encode_one_plane(0, frame, refs, opt, kernels, y),
            encode_one_plane(1, frame, refs, opt, kernels, u),
            encode_one_plane(2, frame, refs, opt, kernels, v),
        )
    };
    let plane_results = [p0, p1, p2];
    let mut all_used = vec![false; refs.len()];
    for (_, used) in &plane_results {
        for (dst, src) in all_used.iter_mut().zip(used) {
            *dst |= *src;
        }
    }
    let used_indices: Vec<usize> = all_used
        .iter()
        .enumerate()
        .filter_map(|(i, &u)| u.then_some(i))
        .collect();
    let mut remap = vec![0usize; refs.len()];
    for (new, &old) in used_indices.iter().enumerate() {
        remap[old] = new;
    }
    let dependencies: Vec<u64> = used_indices.iter().map(|&i| refs[i].0).collect();
    let mut out = Vec::new();
    out.extend(CODEC_MAGIC);
    out.extend(frame_id.to_le_bytes());
    out.extend((frame.width as u16).to_le_bytes());
    out.extend((frame.height as u16).to_le_bytes());
    out.extend(opt.qstep.to_le_bytes());
    out.push(dependencies.len() as u8);
    out.push(1); // bit 0: separate control/data entropy lanes
    for &id in &dependencies {
        out.extend(id.to_le_bytes());
    }
    for (blocks, _) in &plane_results {
        let single_raw = match serialize_blocks(blocks, &remap, BlockLayout::Single) {
            SerializedBlocks::Single(bytes) => bytes,
            SerializedBlocks::Split { .. } => unreachable!(),
        };
        let (control, data) = match serialize_blocks(blocks, &remap, BlockLayout::Split) {
            SerializedBlocks::Split { control, data } => (control, data),
            SerializedBlocks::Single(_) => unreachable!(),
        };
        let single = entropy_compress(&single_raw);
        let cc = entropy_compress(&control);
        let dc = entropy_compress(&data);
        if 4 + single.len() <= 8 + cc.len() + dc.len() {
            out.push(0);
            out.extend((single.len() as u32).to_le_bytes());
            out.extend(single);
        } else {
            out.push(1);
            out.extend((cc.len() as u32).to_le_bytes());
            out.extend(cc);
            out.extend((dc.len() as u32).to_le_bytes());
            out.extend(dc);
        }
    }
    Ok(EncodedFrame {
        packet: out,
        reconstructed,
        dependencies,
    })
}
