//! Stable-Rust deterministic mutation campaign. This complements libFuzzer by running on every CI job.
use avelune_prod::{audio::v1 as audio, container::v1 as container, video::v1 as video};

fn frame() -> video::Frame420 {
    let w = 32u32;
    let h = 24u32;
    let y = (0..(w * h) as usize)
        .map(|i| (i as u8).wrapping_mul(37))
        .collect();
    let u = (0..(w * h / 4) as usize)
        .map(|i| (i as u8).wrapping_mul(11))
        .collect();
    let v = (0..(w * h / 4) as usize)
        .map(|i| 255u8.wrapping_sub(i as u8))
        .collect();
    video::Frame420::from_planes(w, h, y, u, v).unwrap()
}

fn mutations(seed: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for cut in 0..=seed.len().min(96) {
        out.push(seed[..cut].to_vec());
    }
    if seed.is_empty() {
        return out;
    }
    let stride = (seed.len() / 97).max(1);
    for i in (0..seed.len()).step_by(stride).take(128) {
        for mask in [1u8, 0x80, 0xff] {
            let mut x = seed.to_vec();
            x[i] ^= mask;
            out.push(x);
        }
        let mut x = seed.to_vec();
        x.insert(i, 0xff);
        out.push(x);
        if seed.len() > 1 {
            let mut x = seed.to_vec();
            x.remove(i);
            out.push(x);
        }
    }
    for n in [1usize, 7, 64, 1024] {
        let mut x = seed.to_vec();
        x.extend(std::iter::repeat_n(0xff, n));
        out.push(x);
    }
    out
}

#[test]
fn deterministic_mutation_campaign_never_panics_or_changes_classification() {
    let f = frame();
    let vp = video::encode(
        1,
        &f,
        &[],
        video::EncodeOptions {
            qstep: 73,
            ..Default::default()
        },
    )
    .unwrap()
    .packet;
    let pcm: Vec<i16> = (0..512)
        .map(|i| ((i * 997) % 65536) as u16 as i16)
        .collect();
    let ap = audio::encode(
        &pcm,
        audio::EncodeOptions {
            sample_rate: 48_000,
            channels: 2,
            qstep: 1,
            mid_side: true,
        },
    )
    .unwrap();

    for bytes in mutations(&vp) {
        let a = format!("{:?}", video::decode(&bytes, &[]).map(|_| ()));
        let b = format!("{:?}", video::decode(&bytes, &[]).map(|_| ()));
        assert_eq!(a, b);
    }
    for bytes in mutations(&ap) {
        let a = format!("{:?}", audio::decode(&bytes).map(|_| ()));
        let b = format!("{:?}", audio::decode(&bytes).map(|_| ()));
        assert_eq!(a, b);
    }
    // Container decoder: every mutated prefix must deterministically reject or parse without panicking.
    let seed = container::build_file(Vec::new(), Vec::new());
    for bytes in mutations(&seed) {
        let a = format!("{:?}", container::parse_file_prefix(&bytes));
        let b = format!("{:?}", container::parse_file_prefix(&bytes));
        assert_eq!(a, b);
    }
}
