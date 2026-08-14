use avelune::{audio::v1 as audio, video::v1 as video};
use avelune_reference::{
    audio as ref_audio, video_decoder as ref_decoder, video_encoder as ref_encoder,
};
use std::collections::VecDeque;

fn frame(seed: u64, w: u32, h: u32) -> video::Frame420 {
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
    video::Frame420::from_planes(w, h, make(y_len), make(c_len), make(c_len)).unwrap()
}

fn to_ref_encoder(f: &video::Frame420) -> ref_encoder::Frame420 {
    ref_encoder::Frame420 {
        width: f.width,
        height: f.height,
        y: f.y().to_vec(),
        u: f.u().to_vec(),
        v: f.v().to_vec(),
    }
}

fn assert_ref(f: &video::Frame420, r: &ref_decoder::Frame420) {
    assert_eq!(f.width, r.width);
    assert_eq!(f.height, r.height);
    assert_eq!(f.y(), r.y);
    assert_eq!(f.u(), r.u);
    assert_eq!(f.v(), r.v);
}

#[test]
fn canonical_video_encoder_matches_independent_decoder() {
    for &(w, h) in &[(2, 2), (16, 16), (18, 10), (64, 48)] {
        let f = frame(0x9912_3344 ^ u64::from(w), w, h);
        for &q in &[1u16, 17, 96] {
            let encoded = video::encode(
                7,
                &f,
                &[],
                video::EncodeOptions {
                    qstep: q,
                    motion_radius: 2,
                    max_refs: 0,
                    preset: video::EncoderPreset::Balanced,
                    allow_palette: true,
                },
            )
            .unwrap();
            let (_, canonical, _) = video::decode(&encoded.packet, &[]).unwrap();
            assert_eq!(canonical, encoded.reconstructed);
            let (_, reference, _) = ref_decoder::decode(&encoded.packet, &[]).unwrap();
            assert_ref(&canonical, &reference);
        }
    }
}

#[test]
fn independent_encoder_decodes_identically_in_canonical_and_oracle() {
    for seed in 1..12u64 {
        let source = frame(seed, 34, 18);
        let reference_source = to_ref_encoder(&source);
        let encoded = ref_encoder::encode(
            seed,
            &reference_source,
            &[],
            ref_encoder::EncodeOptions {
                qstep: if seed % 3 == 0 { 1 } else { 73 },
                motion_radius: 2,
                max_refs: 0,
                allow_palette: true,
            },
        )
        .unwrap();
        let (_, canonical, _) = video::decode(&encoded.packet, &[]).unwrap();
        assert_eq!(canonical.y(), encoded.reconstructed.y);
        assert_eq!(canonical.u(), encoded.reconstructed.u);
        assert_eq!(canonical.v(), encoded.reconstructed.v);
        let (_, reference, _) = ref_decoder::decode(&encoded.packet, &[]).unwrap();
        assert_ref(&canonical, &reference);
    }
}

#[test]
fn canonical_stateful_video_chain_is_exact() {
    let mut enc = video::VideoEncoder::new(video::EncodeOptions {
        qstep: 1,
        motion_radius: 3,
        max_refs: 4,
        preset: video::EncoderPreset::Quality,
        allow_palette: true,
    });
    let mut dec = video::VideoDecoder::new();
    for id in 1..=8u64 {
        let mut f = frame(99 + id, 32, 24);
        if id > 1 {
            for (i, y) in f.y_mut().iter_mut().enumerate() {
                if i % 5 != 0 {
                    *y = (i as u8).wrapping_add(id as u8);
                }
            }
        }
        let encoded = enc.encode(id, &f).unwrap();
        let (_, decoded, _) = dec.decode(&encoded.packet).unwrap();
        assert_eq!(decoded, f);
        assert!(dec.reference_count() <= 4);
    }
}

#[test]
fn canonical_and_reference_audio_cross_decode() {
    for channels in [1u8, 2] {
        for q in [1u16, 31, 256] {
            let pcm: Vec<i16> = (0..(257 * channels as usize))
                .map(|i| ((i as i32 * 313 + 17) % 60000 - 30000) as i16)
                .collect();
            let packet = audio::encode(
                &pcm,
                audio::EncodeOptions {
                    sample_rate: 48_000,
                    channels,
                    qstep: q,
                    mid_side: channels == 2,
                },
            )
            .unwrap();
            assert_eq!(
                audio::decode(&packet).unwrap().2,
                ref_audio::decode(&packet).unwrap().2
            );

            let reference_packet = ref_audio::encode(
                &pcm,
                ref_audio::EncodeOptions {
                    sample_rate: 48_000,
                    channels,
                    qstep: q,
                    mid_side: channels == 2,
                },
            )
            .unwrap();
            assert_eq!(
                audio::decode(&reference_packet).unwrap().2,
                ref_audio::decode(&reference_packet).unwrap().2
            );
        }
    }
}

fn shifted_from(prev: &video::Frame420, dx: usize, dy: usize) -> video::Frame420 {
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
    video::Frame420::from_planes(
        prev.width,
        prev.height,
        shift_plane(prev.y(), w, h, dx, dy),
        shift_plane(prev.u(), w / 2, h / 2, dx.min(1), dy.min(1)),
        shift_plane(prev.v(), w / 2, h / 2, dx.min(1), dy.min(1)),
    )
    .unwrap()
}

fn palette_frame(seed: u64, w: u32, h: u32) -> video::Frame420 {
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
    video::Frame420::from_planes(w, h, y, u, v).unwrap()
}

#[test]
fn randomized_valid_stateful_video_matches_oracle() {
    for &(w, h) in &[(18u32, 10u32), (34, 26), (66, 34)] {
        let mut enc = video::VideoEncoder::new(video::EncodeOptions {
            qstep: 1,
            motion_radius: 3,
            max_refs: 4,
            preset: video::EncoderPreset::Quality,
            allow_palette: true,
        });
        let mut canonical = video::VideoDecoder::new();
        let mut reference_history: VecDeque<(u64, ref_decoder::Frame420)> = VecDeque::new();
        let mut prev = frame(0xdead_beef ^ u64::from(w), w, h);

        for id in 1..=36u64 {
            let source = match id % 6 {
                0 => palette_frame(id, w, h),
                1 if id > 1 => shifted_from(&prev, (id as usize) & 1, ((id >> 1) as usize) & 1),
                2 => video::Frame420::from_planes(
                    w,
                    h,
                    vec![(id * 17) as u8; (w * h) as usize],
                    vec![(id * 29) as u8; (w * h / 4) as usize],
                    vec![(id * 43) as u8; (w * h / 4) as usize],
                )
                .unwrap(),
                _ => frame(0x9e37_79b9_u64.wrapping_mul(id + u64::from(w)), w, h),
            };
            let qstep = [1, 17, 73, 193][(id % 4) as usize];
            enc.set_options(video::EncodeOptions {
                qstep,
                motion_radius: 3,
                max_refs: 4,
                preset: video::EncoderPreset::Quality,
                allow_palette: true,
            });
            let encoded = enc.encode(id, &source).unwrap();
            assert!(!encoded.dependencies.contains(&id));
            for (i, dep) in encoded.dependencies.iter().enumerate() {
                assert!(!encoded.dependencies[..i].contains(dep));
            }

            let (_, got, deps) = canonical.decode(&encoded.packet).unwrap();
            assert_eq!(deps, encoded.dependencies);
            assert_eq!(got, encoded.reconstructed);

            let refs: Vec<_> = reference_history.iter().map(|(rid, f)| (*rid, f)).collect();
            let (_, reference, reference_deps) =
                ref_decoder::decode(&encoded.packet, &refs).unwrap();
            assert_eq!(reference_deps, encoded.dependencies);
            assert_ref(&got, &reference);
            reference_history.push_back((id, reference));
            while reference_history.len() > 4 {
                reference_history.pop_front();
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
        let options = audio::EncodeOptions {
            sample_rate: rate,
            channels,
            qstep,
            mid_side: channels == 2 && case % 2 == 0,
        };
        let packet = match audio::encode(&pcm, options) {
            Ok(packet) => packet,
            Err(audio::AudioError::SampleOverflow) => {
                let reference_packet = ref_audio::encode(
                    &pcm,
                    ref_audio::EncodeOptions {
                        sample_rate: rate,
                        channels,
                        qstep,
                        mid_side: channels == 2 && case % 2 == 0,
                    },
                )
                .unwrap();
                assert!(matches!(
                    audio::decode(&reference_packet),
                    Err(audio::AudioError::SampleOverflow)
                ));
                assert!(matches!(
                    ref_audio::decode(&reference_packet),
                    Err(ref_audio::AudioError::SampleOverflow)
                ));
                guarded_overflows += 1;
                continue;
            }
            Err(e) => panic!(
                "unexpected canonical encode error: case={case} channels={channels} frames={frames} rate={rate} q={qstep}: {e:?}"
            ),
        };
        let (_, pch, canonical) = audio::decode(&packet).unwrap();
        let (_, rch, reference) = ref_audio::decode(&packet).unwrap();
        assert_eq!(pch, channels);
        assert_eq!(rch, channels);
        assert_eq!(canonical, reference);
        if qstep == 1 {
            assert_eq!(canonical, pcm);
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
