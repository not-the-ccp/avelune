use avelune_audio_v1 as ra;
use avelune_prod::{audio::v1 as pa, video::v1 as pv};
use avelune_video_ref_v1 as rr;
use avelune_video_v1 as rv;

fn frame(seed: u64, w: u32, h: u32) -> pv::Frame420 {
    let y_len = (w * h) as usize;
    let c_len = y_len / 4;
    let mut x = seed;
    let mut make = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    };
    pv::Frame420::from_planes(w, h, make(y_len), make(c_len), make(c_len)).unwrap()
}
fn to_rv(f: &pv::Frame420) -> rv::Frame420 {
    rv::Frame420 {
        width: f.width,
        height: f.height,
        y: f.y().to_vec(),
        u: f.u().to_vec(),
        v: f.v().to_vec(),
    }
}
fn assert_rr(f: &pv::Frame420, r: &rr::Frame420) {
    assert_eq!(f.width, r.width);
    assert_eq!(f.height, r.height);
    assert_eq!(f.y(), r.y);
    assert_eq!(f.u(), r.u);
    assert_eq!(f.v(), r.v);
}
fn assert_rv(f: &pv::Frame420, r: &rv::Frame420) {
    assert_eq!(f.width, r.width);
    assert_eq!(f.height, r.height);
    assert_eq!(f.y(), r.y);
    assert_eq!(f.u(), r.u);
    assert_eq!(f.v(), r.v);
}

#[test]
fn video_prod_encoder_matches_both_reference_decoders() {
    for &(w, h) in &[(2, 2), (16, 16), (18, 10), (64, 48)] {
        let f = frame(0x9912_3344 ^ u64::from(w), w, h);
        for &q in &[1u16, 17, 96] {
            let e = pv::encode(
                7,
                &f,
                &[],
                pv::EncodeOptions {
                    qstep: q,
                    motion_radius: 2,
                    max_refs: 0,
                    preset: pv::EncoderPreset::Balanced,
                    allow_palette: true,
                },
            )
            .unwrap();
            let (_, pd, _) = pv::decode(&e.packet, &[]).unwrap();
            assert_eq!(pd, e.reconstructed);
            let (_, rd, _) = rr::decode(&e.packet, &[]).unwrap();
            assert_rr(&pd, &rd);
            let (_, od, _) = rv::decode(&e.packet, &[]).unwrap();
            assert_rv(&pd, &od);
        }
    }
}

#[test]
fn reference_video_encoder_decodes_identically_in_prod() {
    for seed in 1..12u64 {
        let f = frame(seed, 34, 18);
        let rf = to_rv(&f);
        let e = rv::encode(
            seed,
            &rf,
            &[],
            rv::EncodeOptions {
                qstep: if seed % 3 == 0 { 1 } else { 73 },
                motion_radius: 2,
                max_refs: 0,
                allow_palette: true,
            },
        )
        .unwrap();
        let (_, pd, _) = pv::decode(&e.packet, &[]).unwrap();
        assert_rv(&pd, &e.reconstructed);
        let (_, rd, _) = rr::decode(&e.packet, &[]).unwrap();
        assert_rr(&pd, &rd);
    }
}

#[test]
fn stateful_video_reference_chain_is_exact() {
    let mut enc = pv::VideoEncoder::new(pv::EncodeOptions {
        qstep: 1,
        motion_radius: 3,
        max_refs: 4,
        preset: pv::EncoderPreset::Quality,
        allow_palette: true,
    });
    let mut dec = pv::VideoDecoder::new();
    for id in 1..=8u64 {
        let mut f = frame(99 + id, 32, 24);
        if id > 1 {
            for (i, y) in f.y_mut().iter_mut().enumerate() {
                if i % 5 != 0 {
                    *y = (i as u8).wrapping_add(id as u8);
                }
            }
        }
        let e = enc.encode(id, &f).unwrap();
        let (_, d, _) = dec.decode(&e.packet).unwrap();
        assert_eq!(d, f);
        assert!(dec.reference_count() <= 4);
    }
}

#[test]
fn audio_prod_and_reference_cross_decode() {
    for channels in [1u8, 2] {
        for q in [1u16, 31, 256] {
            let pcm: Vec<i16> = (0..(257 * channels as usize))
                .map(|i| ((i as i32 * 313 + 17) % 60000 - 30000) as i16)
                .collect();
            let po = pa::EncodeOptions {
                sample_rate: 48_000,
                channels,
                qstep: q,
                mid_side: channels == 2,
            };
            let pp = pa::encode(&pcm, po).unwrap();
            let (_, _, pd) = pa::decode(&pp).unwrap();
            let (_, _, rd) = ra::decode(&pp).unwrap();
            assert_eq!(pd, rd);
            let ro = ra::EncodeOptions {
                sample_rate: 48_000,
                channels,
                qstep: q,
                mid_side: channels == 2,
            };
            let rp = ra::encode(&pcm, ro).unwrap();
            let (_, _, pdr) = pa::decode(&rp).unwrap();
            let (_, _, rdr) = ra::decode(&rp).unwrap();
            assert_eq!(pdr, rdr);
        }
    }
}

fn shifted_from(prev: &pv::Frame420, dx: usize, dy: usize) -> pv::Frame420 {
    fn shift_plane(src: &[u8], w: usize, h: usize, dx: usize, dy: usize) -> Vec<u8> {
        let mut out = vec![0; src.len()];
        for y in 0..h {
            for x in 0..w {
                let sx = x.saturating_sub(dx).min(w - 1);
                let sy = y.saturating_sub(dy).min(h - 1);
                out[y * w + x] = src[sy * w + sx];
            }
        }
        out
    }
    let w = prev.width as usize;
    let h = prev.height as usize;
    pv::Frame420::from_planes(
        prev.width,
        prev.height,
        shift_plane(prev.y(), w, h, dx, dy),
        shift_plane(prev.u(), w / 2, h / 2, dx.min(1), dy.min(1)),
        shift_plane(prev.v(), w / 2, h / 2, dx.min(1), dy.min(1)),
    )
    .unwrap()
}

fn palette_frame(seed: u64, w: u32, h: u32) -> pv::Frame420 {
    let colors = [16u8, 64, 192, 235];
    let y = (0..(w * h) as usize)
        .map(|i| colors[(i + seed as usize) & 3])
        .collect();
    let c_len = (w * h / 4) as usize;
    let u = (0..c_len)
        .map(|i| colors[(i * 3 + seed as usize) & 3])
        .collect();
    let v = (0..c_len)
        .map(|i| colors[(i * 5 + 1 + seed as usize) & 3])
        .collect();
    pv::Frame420::from_planes(w, h, y, u, v).unwrap()
}

#[test]
fn randomized_valid_stateful_video_matches_all_decoders() {
    for &(w, h) in &[(18u32, 10u32), (34, 26), (66, 34)] {
        let mut enc = pv::VideoEncoder::new(pv::EncodeOptions {
            qstep: 1,
            motion_radius: 3,
            max_refs: 4,
            preset: pv::EncoderPreset::Quality,
            allow_palette: true,
        });
        let mut prod = pv::VideoDecoder::new();
        let mut rv_hist: std::collections::VecDeque<(u64, rv::Frame420)> =
            std::collections::VecDeque::new();
        let mut rr_hist: std::collections::VecDeque<(u64, rr::Frame420)> =
            std::collections::VecDeque::new();
        let mut prev = frame(0xdead_beef ^ u64::from(w), w, h);

        for id in 1..=36u64 {
            let source = match id % 6 {
                0 => palette_frame(id, w, h),
                1 if id > 1 => shifted_from(&prev, (id as usize) & 1, ((id >> 1) as usize) & 1),
                2 => pv::Frame420::from_planes(
                    w,
                    h,
                    vec![(id * 17) as u8; (w * h) as usize],
                    vec![(id * 29) as u8; (w * h / 4) as usize],
                    vec![(id * 43) as u8; (w * h / 4) as usize],
                )
                .unwrap(),
                _ => frame(0x9e37_79b9_u64.wrapping_mul(id + u64::from(w)), w, h),
            };
            let qstep = match id % 4 {
                0 => 1,
                1 => 17,
                2 => 73,
                _ => 193,
            };
            enc.set_options(pv::EncodeOptions {
                qstep,
                motion_radius: 3,
                max_refs: 4,
                preset: pv::EncoderPreset::Quality,
                allow_palette: true,
            });
            let encoded = enc.encode(id, &source).unwrap();
            assert!(!encoded.dependencies.contains(&id));
            for (i, dep) in encoded.dependencies.iter().enumerate() {
                assert!(!encoded.dependencies[..i].contains(dep));
            }

            let (_, pd, pdeps) = prod.decode(&encoded.packet).unwrap();
            assert_eq!(pdeps, encoded.dependencies);
            assert_eq!(pd, encoded.reconstructed);

            let rv_refs: Vec<(u64, &rv::Frame420)> =
                rv_hist.iter().map(|(rid, f)| (*rid, f)).collect();
            let (_, vd, vdeps) = rv::decode(&encoded.packet, &rv_refs).unwrap();
            assert_eq!(vdeps, encoded.dependencies);
            assert_rv(&pd, &vd);

            let rr_refs: Vec<(u64, &rr::Frame420)> =
                rr_hist.iter().map(|(rid, f)| (*rid, f)).collect();
            let (_, rd, rdeps) = rr::decode(&encoded.packet, &rr_refs).unwrap();
            assert_eq!(rdeps, encoded.dependencies);
            assert_rr(&pd, &rd);

            rv_hist.push_back((id, vd));
            rr_hist.push_back((id, rd));
            while rv_hist.len() > 4 {
                rv_hist.pop_front();
            }
            while rr_hist.len() > 4 {
                rr_hist.pop_front();
            }
            prev = source;
        }
    }
}

#[test]
fn randomized_audio_syntax_range_matches_reference_decoder() {
    let sample_rates = [8_000u32, 44_100, 48_000, 96_000, 384_000];
    let frame_counts = [1usize, 63, 64, 65, 257, 1023, 4096];
    let qsteps = [1u16, 3, 31, 256, 4096, u16::MAX];
    let mut state = 0x6a09_e667_f3bc_c909u64;
    let mut valid_packets = 0usize;
    let mut guarded_overflows = 0usize;
    for case in 0..72usize {
        let channels = (case % 8 + 1) as u8;
        let frames = frame_counts[case % frame_counts.len()];
        let rate = sample_rates[(case / 3) % sample_rates.len()];
        let qstep = qsteps[(case / 5) % qsteps.len()];
        let mut pcm = Vec::with_capacity(frames * channels as usize);
        for _ in 0..pcm.capacity() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            pcm.push(state as i16);
        }
        let options = pa::EncodeOptions {
            sample_rate: rate,
            channels,
            qstep,
            mid_side: channels == 2 && case % 2 == 0,
        };
        let packet = match pa::encode(&pcm, options) {
            Ok(packet) => packet,
            Err(pa::AudioError::SampleOverflow) => {
                // Draft Generation 1 nearest scalar quantization can create a reconstruction
                // outside signed-16 range. Production refuses to emit that invalid packet.
                let reference_packet = ra::encode(
                    &pcm,
                    ra::EncodeOptions {
                        sample_rate: rate,
                        channels,
                        qstep,
                        mid_side: channels == 2 && case % 2 == 0,
                    },
                )
                .unwrap();
                assert!(matches!(
                    pa::decode(&reference_packet),
                    Err(pa::AudioError::SampleOverflow)
                ));
                assert!(matches!(
                    ra::decode(&reference_packet),
                    Err(ra::AudioError::SampleOverflow)
                ));
                guarded_overflows += 1;
                continue;
            }
            Err(e) => panic!(
                "unexpected prod encode error: case={case} channels={channels} frames={frames} rate={rate} q={qstep}: {e:?}"
            ),
        };
        let (_, pch, pd) = pa::decode(&packet).unwrap();
        let (_, rch, rd) = ra::decode(&packet).unwrap();
        assert_eq!(pch, channels);
        assert_eq!(rch, channels);
        assert_eq!(
            pd, rd,
            "case={case} channels={channels} frames={frames} q={qstep}"
        );
        if qstep == 1 {
            assert_eq!(pd, pcm);
        }
        valid_packets += 1;
    }
    assert!(
        valid_packets >= 48,
        "too few valid randomized packets: {valid_packets}"
    );
    assert!(
        guarded_overflows > 0,
        "fixture no longer exercises lossy overflow guard"
    );
}
