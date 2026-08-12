use avelune_audio_v1 as ref_audio;
use avelune_prod::{
    audio::v1 as audio,
    bitstream::v1 as bs,
    config::{Config, CpuBackend, ThreadPolicy},
    video::v1 as video,
};
use avelune_video_ref_v1 as ref_video;
use proptest::prelude::*;

fn mk_frame(w: u32, h: u32, bytes: &[u8]) -> video::Frame420 {
    let y_len = (w * h) as usize;
    let c_len = y_len / 4;
    let mut src = bytes.iter().copied().cycle();
    let mut take = |n: usize| (0..n).map(|_| src.next().unwrap_or(0)).collect::<Vec<_>>();
    video::Frame420::from_planes(w, h, take(y_len), take(c_len), take(c_len)).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    #[test]
    fn canonical_uvarint_roundtrip(v in any::<u64>()) {
        let mut out = Vec::new();
        bs::put_uvarint(v, &mut out);
        let mut pos = 0;
        prop_assert_eq!(bs::get_uvarint(&out, &mut pos).unwrap(), v);
        prop_assert_eq!(pos, out.len());
    }

    #[test]
    fn canonical_svarint_roundtrip(v in any::<i32>()) {
        let mut out = Vec::new();
        bs::put_svarint_i32(v, &mut out);
        let mut pos = 0;
        prop_assert_eq!(bs::get_svarint_i32(&out, &mut pos).unwrap(), v);
        prop_assert_eq!(pos, out.len());
    }

    #[test]
    fn entropy_roundtrip(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let coded = bs::entropy_compress(&data);
        prop_assert_eq!(bs::entropy_decompress(&coded, data.len().max(1) * 16 + 4096).unwrap(), data);
    }

    #[test]
    fn random_lossless_video_cross_decodes(
        w_blocks in 1u32..9,
        h_blocks in 1u32..7,
        data in prop::collection::vec(any::<u8>(), 1..4096),
        preset in prop_oneof![Just(video::EncoderPreset::Fast), Just(video::EncoderPreset::Balanced), Just(video::EncoderPreset::Quality)],
    ) {
        let w = w_blocks * 2;
        let h = h_blocks * 2;
        let src = mk_frame(w, h, &data);
        let encoded = video::encode(11, &src, &[], video::EncodeOptions {
            qstep: 1, motion_radius: 2, max_refs: 0, preset, allow_palette: true,
        }).unwrap();
        let (_, prod, _) = video::decode(&encoded.packet, &[]).unwrap();
        prop_assert_eq!(&prod, &src);
        let (_, reference, _) = ref_video::decode(&encoded.packet, &[]).unwrap();
        prop_assert_eq!(reference.y.as_slice(), src.y());
        prop_assert_eq!(reference.u.as_slice(), src.u());
        prop_assert_eq!(reference.v.as_slice(), src.v());
    }

    #[test]
    fn random_lossless_audio_cross_decodes(
        channels in 1u8..=8,
        frames in 1usize..512,
        seed in any::<u64>(),
    ) {
        let mut x = seed;
        let mut pcm = Vec::with_capacity(frames * channels as usize);
        for _ in 0..pcm.capacity() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            pcm.push(x as i16);
        }
        let opt = audio::EncodeOptions { sample_rate: 48_000, channels, qstep: 1, mid_side: channels == 2 };
        let packet = audio::encode(&pcm, opt).unwrap();
        let (_, _, prod) = audio::decode(&packet).unwrap();
        let (_, _, reference) = ref_audio::decode(&packet).unwrap();
        prop_assert_eq!(&prod, &pcm);
        prop_assert_eq!(reference, pcm);
    }
}

#[test]
fn scalar_and_auto_decoder_reconstruct_identically() {
    for seed in 0..32u8 {
        let src = mk_frame(34, 26, &[seed, seed.wrapping_mul(17), 255 - seed]);
        let packet = video::encode(
            1,
            &src,
            &[],
            video::EncodeOptions {
                qstep: 73,
                ..Default::default()
            },
        )
        .unwrap()
        .packet;
        let mut scalar = video::VideoDecoder::with_config(Config {
            cpu: CpuBackend::Scalar,
            threads: ThreadPolicy::Single,
            ..Default::default()
        })
        .unwrap();
        let mut auto = video::VideoDecoder::new();
        assert_eq!(
            scalar.decode(&packet).unwrap().1,
            auto.decode(&packet).unwrap().1
        );
    }
}

#[test]
fn encoder_thread_policy_does_not_change_reconstruction() {
    let src = mk_frame(128, 96, &[0, 1, 2, 3, 250, 251, 252, 253]);
    let opt = video::EncodeOptions {
        qstep: 96,
        motion_radius: 3,
        ..Default::default()
    };
    let mut single = video::VideoEncoder::with_config(
        opt,
        Config {
            threads: ThreadPolicy::Single,
            ..Default::default()
        },
    )
    .unwrap();
    let mut bounded = video::VideoEncoder::with_config(
        opt,
        Config {
            threads: ThreadPolicy::Max(3),
            ..Default::default()
        },
    )
    .unwrap();
    let a = single.encode(7, &src).unwrap();
    let b = bounded.encode(7, &src).unwrap();
    assert_eq!(a.reconstructed, b.reconstructed);
    let da = video::decode(&a.packet, &[]).unwrap().1;
    let db = video::decode(&b.packet, &[]).unwrap().1;
    assert_eq!(da, db);
}
