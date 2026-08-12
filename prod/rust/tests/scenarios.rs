//! Deterministic content-family scenarios that random property generation is unlikely to hit reliably.
use avelune_prod::video::v1 as p;
use avelune_video_ref_v1 as r;
use avelune_video_v1 as v;

#[derive(Clone, Copy)]
enum Kind {
    Static,
    Cut,
    Fade,
    Grain,
    Gradient,
    Checker,
    Ui,
    Chroma,
    Pan,
    Flash,
    LowLight,
    Impulse,
}

fn make(kind: Kind, t: usize, w: u32, h: u32) -> p::Frame420 {
    let (wu, hu) = (w as usize, h as usize);
    let mut y = vec![0u8; wu * hu];
    let mut u = vec![128u8; wu * hu / 4];
    let mut vv = vec![128u8; wu * hu / 4];
    for yy in 0..hu {
        for xx in 0..wu {
            let i = yy * wu + xx;
            y[i] = match kind {
                Kind::Static => ((xx * 3 + yy * 5) & 255) as u8,
                Kind::Cut => ((xx * 3 + yy * 5 + if t < 3 { 0 } else { 117 }) & 255) as u8,
                Kind::Fade => (((xx * 5 + yy * 3) & 255) * t.min(7) / 7) as u8,
                Kind::Grain => {
                    let mut z = (i as u64) ^ ((t as u64) << 32) ^ 0x9e3779b97f4a7c15;
                    z ^= z << 13;
                    z ^= z >> 7;
                    z ^= z << 17;
                    z as u8
                }
                Kind::Gradient => ((xx * 255 / wu.max(2).saturating_sub(1) + t) & 255) as u8,
                Kind::Checker => {
                    if ((xx / 4 + yy / 4 + t / 2) & 1) == 0 {
                        16
                    } else {
                        235
                    }
                }
                Kind::Ui => {
                    if ((xx / 12 + yy / 8 + t / 3) & 1) == 0 {
                        32
                    } else {
                        224
                    }
                }
                Kind::Chroma => 128,
                Kind::Pan => (((xx + t * 2) * 7 + yy * 3) & 255) as u8,
                Kind::Flash => {
                    if t == 3 {
                        255
                    } else {
                        ((xx + yy) & 63) as u8 + 32
                    }
                }
                Kind::LowLight => (((xx * 13 + yy * 7 + t * 3) & 31) + 4) as u8,
                Kind::Impulse => {
                    if (xx + yy * wu) == (t * 97) % (wu * hu) {
                        255
                    } else {
                        0
                    }
                }
            };
        }
    }
    let cw = wu / 2;
    let ch = hu / 2;
    if matches!(kind, Kind::Chroma) {
        for yy in 0..ch {
            for xx in 0..cw {
                let i = yy * cw + xx;
                u[i] = ((xx * 29 + t * 11) & 255) as u8;
                vv[i] = ((yy * 31 + 255 - t * 7) & 255) as u8;
            }
        }
    }
    p::Frame420::from_planes(w, h, y, u, vv).unwrap()
}
fn assert_v(g: &p::Frame420, x: &v::Frame420) {
    assert_eq!(g.y(), x.y);
    assert_eq!(g.u(), x.u);
    assert_eq!(g.v(), x.v);
}
fn assert_r(g: &p::Frame420, x: &r::Frame420) {
    assert_eq!(g.y(), x.y);
    assert_eq!(g.u(), x.u);
    assert_eq!(g.v(), x.v);
}

#[test]
fn deterministic_content_families_cross_decode() {
    let kinds = [
        Kind::Static,
        Kind::Cut,
        Kind::Fade,
        Kind::Grain,
        Kind::Gradient,
        Kind::Checker,
        Kind::Ui,
        Kind::Chroma,
        Kind::Pan,
        Kind::Flash,
        Kind::LowLight,
        Kind::Impulse,
    ];
    for (ki, kind) in kinds.into_iter().enumerate() {
        for &q in &[1u16, 48, 96, 192, 511] {
            for &(w, h) in &[(34u32, 26u32), (18, 10)] {
                for t in 0..6usize {
                    let src = make(kind, t, w, h);
                    let e = p::encode(
                        (ki * 100 + t) as u64 + 1,
                        &src,
                        &[],
                        p::EncodeOptions {
                            qstep: q,
                            motion_radius: 3,
                            max_refs: 0,
                            preset: p::EncoderPreset::Balanced,
                            allow_palette: true,
                        },
                    )
                    .unwrap();
                    let (_, pd, _) = p::decode(&e.packet, &[]).unwrap();
                    let (_, vd, _) = v::decode(&e.packet, &[]).unwrap();
                    let (_, rd, _) = r::decode(&e.packet, &[]).unwrap();
                    assert_eq!(pd, e.reconstructed);
                    assert_v(&pd, &vd);
                    assert_r(&pd, &rd);
                    if q == 1 {
                        assert_eq!(pd, src);
                    }
                }
            }
        }
    }
}

#[test]
fn extreme_aspect_and_partial_shapes_remain_lossless() {
    for &(w, h) in &[(2u32, 8192u32), (8192, 2), (2, 2), (18, 10), (66, 34)] {
        let src = make(Kind::Gradient, 2, w, h);
        let e = p::encode(
            99,
            &src,
            &[],
            p::EncodeOptions {
                qstep: 1,
                motion_radius: 0,
                max_refs: 0,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, got, _) = p::decode(&e.packet, &[]).unwrap();
        assert_eq!(got, src);
    }
}
