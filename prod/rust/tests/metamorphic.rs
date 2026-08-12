use avelune_prod::{container::v1 as c, video::v1 as v};

fn frame(seed: u64, w: u32, h: u32) -> v::Frame420 {
    let mut x = seed;
    let mut make = |n: usize| {
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect::<Vec<_>>()
    };
    let y = (w * h) as usize;
    let ch = y / 4;
    v::Frame420::from_planes(w, h, make(y), make(ch), make(ch)).unwrap()
}

#[test]
fn lossless_decode_reencode_is_content_idempotent() {
    let src = frame(0x1234_5678, 34, 26);
    let opt = v::EncodeOptions {
        qstep: 1,
        motion_radius: 2,
        max_refs: 0,
        ..Default::default()
    };
    let a = v::encode(9, &src, &[], opt).unwrap();
    let (_, decoded, _) = v::decode(&a.packet, &[]).unwrap();
    let b = v::encode(10, &decoded, &[], opt).unwrap();
    let (_, decoded2, _) = v::decode(&b.packet, &[]).unwrap();
    assert_eq!(decoded, src);
    assert_eq!(decoded2, src);
}

fn one_epoch_file() -> Vec<u8> {
    let p = c::Packet {
        kind: c::PacketKind::EpochStart,
        flags: 0,
        stream_id: 0,
        pts: 0,
        duration: 1000,
        payload: 7u32.to_le_bytes().to_vec(),
    };
    let mut epoch = Vec::new();
    c::encode_packet_checked(&p, &mut epoch).unwrap();
    c::build_file_checked(Vec::new(), vec![(7, 0, 1000, epoch)]).unwrap()
}

fn parse_fragmented(bytes: &[u8], chunks: &[usize], reserve: bool) -> Vec<c::Packet> {
    let mut parser = c::StreamParser::default();
    let mut out = Vec::new();
    let mut pos = 0;
    let mut ci = 0;
    while pos < bytes.len() {
        let n = chunks[ci % chunks.len()].max(1).min(bytes.len() - pos);
        let frag = &bytes[pos..pos + n];
        if reserve {
            parser.reserve_fragment(n).unwrap().copy_from_slice(frag);
            parser
                .commit_reserved(n, |view| {
                    out.push(view.to_owned());
                    Ok::<(), std::convert::Infallible>(())
                })
                .unwrap();
        } else {
            out.extend(parser.push(frag).unwrap());
        }
        pos += n;
        ci += 1;
    }
    parser.finish().unwrap();
    out
}

#[test]
fn transport_fragmentation_is_semantically_irrelevant() {
    let bytes = one_epoch_file();
    let baseline = parse_fragmented(&bytes, &[bytes.len()], false);
    for chunks in [&[1usize][..], &[2, 3, 5, 7, 11][..], &[31, 1, 4, 1, 59][..]] {
        assert_eq!(parse_fragmented(&bytes, chunks, false), baseline);
        assert_eq!(parse_fragmented(&bytes, chunks, true), baseline);
    }
}

#[test]
fn parser_reset_file_replays_same_bytes_without_state_leak() {
    let bytes = one_epoch_file();
    let mut p = c::StreamParser::default();
    let first = p.push(&bytes).unwrap();
    p.finish().unwrap();
    let mut p = c::StreamParser::default();
    let second = p.push(&bytes).unwrap();
    p.finish().unwrap();
    assert_eq!(first, second);
}

#[test]
fn epoch_reset_prevents_video_reference_leakage() {
    let f1 = frame(1, 32, 24);
    let f2 = frame(2, 32, 24);
    let mut enc = v::VideoEncoder::new(v::EncodeOptions {
        qstep: 1,
        motion_radius: 2,
        max_refs: 4,
        ..Default::default()
    });
    let p1 = enc.encode(1, &f1).unwrap().packet;
    let _p2 = enc.encode(2, &f2).unwrap().packet;
    let mut dec = v::VideoDecoder::new();
    assert_eq!(dec.decode(&p1).unwrap().1, f1);
    assert_eq!(dec.reference_count(), 1);
    dec.reset_epoch();
    assert_eq!(dec.reference_count(), 0);
    // Replaying a self-contained first packet after reset must behave identically.
    assert_eq!(dec.decode(&p1).unwrap().1, f1);
}
