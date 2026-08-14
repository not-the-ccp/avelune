use avelune::video::v1 as video;

fn frame(id: u64) -> video::Frame420 {
    let w = 66u32;
    let h = 34u32;
    let y = (0..(w * h) as usize)
        .map(|i| {
            (i as u8)
                .wrapping_add(id as u8)
                .rotate_left((id & 7) as u32)
        })
        .collect();
    let u = (0..(w * h / 4) as usize)
        .map(|i| (i as u8).wrapping_mul(3).wrapping_add(id as u8))
        .collect();
    let v = (0..(w * h / 4) as usize)
        .map(|i| (i as u8).wrapping_mul(7).wrapping_sub(id as u8))
        .collect();
    video::Frame420::from_planes(w, h, y, u, v).unwrap()
}

#[test]
#[ignore = "scheduled/deep CI soak"]
fn thousand_frame_stateful_encode_decode_soak() {
    let mut enc = video::VideoEncoder::new(video::EncodeOptions {
        qstep: 1,
        motion_radius: 2,
        max_refs: 4,
        ..Default::default()
    });
    let mut dec = video::VideoDecoder::new();
    for id in 1..=1000u64 {
        if id % 127 == 0 {
            enc.reset_epoch();
            dec.reset_epoch();
        }
        let src = frame(id);
        let packet = enc.encode(id, &src).unwrap().packet;
        let (_, got, _) = dec.decode(&packet).unwrap();
        assert_eq!(got, src);
        assert!(dec.reference_count() <= 4);
        assert!(enc.pooled_frame_count() <= 4);
        assert!(dec.pooled_frame_count() <= 4);
    }
}

#[test]
#[ignore = "scheduled/deep CI soak"]
fn many_independent_decoders_do_not_share_state() {
    let src = frame(42);
    let packet = video::encode(
        42,
        &src,
        &[],
        video::EncodeOptions {
            qstep: 1,
            ..Default::default()
        },
    )
    .unwrap()
    .packet;
    std::thread::scope(|s| {
        for _ in 0..32 {
            let packet = &packet;
            let src = &src;
            s.spawn(move || {
                for _ in 0..100 {
                    let mut d = video::VideoDecoder::new();
                    assert_eq!(d.decode(packet).unwrap().1, *src);
                }
            });
        }
    });
}
