use avelune::{
    audio::v1 as audio, bitstream::v1 as bitstream, container::v1 as container, kernels::KernelSet,
    video::v1 as video,
};

fn frame() -> video::Frame420 {
    let (w, h) = (34u32, 18u32);
    let y: Vec<u8> = (0..w * h)
        .map(|i| (i.wrapping_mul(73) ^ (i >> 2)) as u8)
        .collect();
    let c: Vec<u8> = (0..w * h / 4)
        .map(|i| (i.wrapping_mul(29) ^ 0xa5) as u8)
        .collect();
    video::Frame420::from_planes(w, h, y, c.clone(), c.into_iter().rev().collect()).unwrap()
}

#[test]
fn every_video_truncation_is_rejected_without_panic() {
    let encoded = video::encode(
        9,
        &frame(),
        &[],
        video::EncodeOptions {
            qstep: 17,
            motion_radius: 2,
            max_refs: 0,
            preset: video::EncoderPreset::Balanced,
            allow_palette: true,
        },
    )
    .unwrap()
    .packet;
    for n in 0..encoded.len() {
        assert!(
            video::decode(&encoded[..n], &[]).is_err(),
            "accepted truncation at {n}/{}",
            encoded.len()
        );
    }
    assert!(video::decode(&encoded, &[]).is_ok());
}

#[test]
fn every_audio_truncation_is_rejected_without_panic() {
    let pcm: Vec<i16> = (0..511 * 2)
        .map(|i| ((i * 997) % 60001 - 30000) as i16)
        .collect();
    let encoded = audio::encode(
        &pcm,
        audio::EncodeOptions {
            sample_rate: 48_000,
            channels: 2,
            qstep: 31,
            mid_side: true,
        },
    )
    .unwrap();
    for n in 0..encoded.len() {
        assert!(
            audio::decode(&encoded[..n]).is_err(),
            "accepted truncation at {n}/{}",
            encoded.len()
        );
    }
    assert!(audio::decode(&encoded).is_ok());
}

#[test]
fn canonical_varints_reject_overlong_and_overflow_forms() {
    for bytes in [
        vec![0x80, 0x00],
        vec![0x81, 0x00],
        vec![0xff; 11],
        vec![0x80; 10],
    ] {
        let mut p = 0;
        assert!(
            bitstream::get_uvarint(&bytes, &mut p).is_err(),
            "accepted {bytes:02x?}"
        );
    }
}

#[test]
fn entropy_decoder_rejects_output_over_limit() {
    let source = vec![7u8; 4096];
    let coded = bitstream::entropy_compress(&source);
    assert!(bitstream::entropy_decompress(&coded, 4095).is_err());
    assert_eq!(bitstream::entropy_decompress(&coded, 4096).unwrap(), source);
}

#[test]
fn stream_parser_enforces_front_and_packet_resource_limits() {
    let mut header = Vec::new();
    container::encode_header(
        &container::FileHeader {
            flags: 1,
            stream_count: 0,
            front_len: 16 * 1024,
        },
        &mut header,
    );
    let mut parser = container::StreamParser::new(1024, 4096, KernelSet::scalar());
    assert!(matches!(
        parser.push(&header),
        Err(container::ContainerError::ResourceLimit)
    ));

    let mut packet = Vec::new();
    container::encode_packet(
        &container::Packet {
            kind: container::PacketKind::Metadata,
            flags: 0,
            stream_id: 0,
            pts: 0,
            duration: 0,
            payload: vec![0x55; 2048],
        },
        &mut packet,
    );
    let mut parser = container::StreamParser::new(64, 4096, KernelSet::scalar());
    parser.reset_epoch_range();
    assert!(matches!(
        parser.push(&packet),
        Err(container::ContainerError::PacketTooLarge(2048))
    ));
}

#[test]
fn mutation_smoke_never_panics() {
    let video_packet = video::encode(1, &frame(), &[], video::EncodeOptions::default())
        .unwrap()
        .packet;
    let pcm: Vec<i16> = (0..255).map(|i| (i as i16).wrapping_mul(251)).collect();
    let audio_packet = audio::encode(
        &pcm,
        audio::EncodeOptions {
            sample_rate: 48_000,
            channels: 1,
            qstep: 19,
            mid_side: false,
        },
    )
    .unwrap();

    for (packet, is_video) in [(&video_packet, true), (&audio_packet, false)] {
        let step = (packet.len() / 257).max(1);
        for i in (0..packet.len()).step_by(step) {
            for mask in [0x01u8, 0x80, 0xff] {
                let mut mutated = packet.clone();
                mutated[i] ^= mask;
                let result = std::panic::catch_unwind(|| {
                    if is_video {
                        let _ = video::decode(&mutated, &[]);
                    } else {
                        let _ = audio::decode(&mutated);
                    }
                });
                assert!(result.is_ok(), "panic at byte {i}, xor={mask:#x}");
            }
        }
    }
}

#[test]
fn arbitrary_transport_fragmentation_matches_contiguous_packet_count() {
    let mut epoch = Vec::new();
    for i in 0..17u64 {
        container::encode_packet(
            &container::Packet {
                kind: container::PacketKind::Metadata,
                flags: 0,
                stream_id: 1,
                pts: i * 100,
                duration: 100,
                payload: (0..(i as usize * 13 + 1)).map(|x| x as u8).collect(),
            },
            &mut epoch,
        );
    }
    let mut parser =
        container::StreamParser::new(1024 * 1024, 2 * 1024 * 1024, KernelSet::scalar());
    parser.reset_epoch_range();
    let mut count = 0;
    let mut off = 0;
    let mut state = 0x1234_5678u32;
    while off < epoch.len() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let n = (1 + state as usize % 97).min(epoch.len() - off);
        count += parser.push(&epoch[off..off + n]).unwrap().len();
        off += n;
    }
    parser.finish().unwrap();
    assert_eq!(count, 17);
}

#[test]
fn configured_entropy_limits_are_enforced_by_stateful_decoders() {
    let limits = avelune::limits::Limits {
        max_entropy_bytes: 1,
        ..avelune::limits::Limits::default()
    };

    let encoded_video = video::encode(1, &frame(), &[], video::EncodeOptions::default())
        .unwrap()
        .packet;
    let config = avelune::config::Config {
        limits,
        ..avelune::config::Config::default()
    };
    let mut decoder = video::VideoDecoder::with_config(config).unwrap();
    assert!(decoder.decode(&encoded_video).is_err());

    let pcm: Vec<i16> = (0..128).map(|i| (i as i16).wrapping_mul(173)).collect();
    let encoded_audio = audio::encode(
        &pcm,
        audio::EncodeOptions {
            sample_rate: 48_000,
            channels: 1,
            qstep: 1,
            mid_side: false,
        },
    )
    .unwrap();
    let mut decoder = audio::AudioDecoder::with_limits(limits);
    assert!(decoder.decode(&encoded_audio).is_err());
}

#[test]
fn configured_frame_pixel_limits_are_enforced_by_stateful_video() {
    let source = frame();
    let packet = video::encode(1, &source, &[], video::EncodeOptions::default())
        .unwrap()
        .packet;
    let limits = avelune::limits::Limits {
        max_frame_pixels: 64,
        ..avelune::limits::Limits::default()
    };
    let config = avelune::config::Config {
        limits,
        ..avelune::config::Config::default()
    };
    let mut decoder = video::VideoDecoder::with_config(config).unwrap();
    assert_eq!(
        decoder.decode(&packet),
        Err(video::VideoError::OutputTooLarge)
    );
    let mut encoder =
        video::VideoEncoder::with_config(video::EncodeOptions::default(), config).unwrap();
    assert!(matches!(
        encoder.encode(2, &source),
        Err(video::VideoError::OutputTooLarge)
    ));
}

#[test]
fn common_limits_configure_stream_parser_bounds() {
    let limits = avelune::limits::Limits {
        max_packet_bytes: 64,
        max_stream_buffer_bytes: 4096,
        ..avelune::limits::Limits::default()
    };
    let mut parser = container::StreamParser::with_limits(limits, KernelSet::scalar());
    parser.reset_epoch_range();

    let mut packet = Vec::new();
    container::encode_packet(
        &container::Packet {
            kind: container::PacketKind::Metadata,
            flags: 0,
            stream_id: 0,
            pts: 0,
            duration: 0,
            payload: vec![0xaa; 65],
        },
        &mut packet,
    );
    assert!(matches!(
        parser.push(&packet),
        Err(container::ContainerError::PacketTooLarge(65))
    ));
}

#[test]
fn audio_reserved_header_bytes_are_rejected() {
    let pcm = vec![0i16; 64];
    let mut packet = audio::encode(
        &pcm,
        audio::EncodeOptions {
            sample_rate: 48_000,
            channels: 1,
            qstep: 1,
            mid_side: false,
        },
    )
    .unwrap();
    packet[14] = 1;
    assert!(matches!(
        audio::decode(&packet),
        Err(audio::AudioError::BadHeader)
    ));
}

#[test]
fn invalid_v1_front_metadata_is_rejected_even_with_valid_crc() {
    let bad_video = container::Front {
        streams: vec![container::StreamDesc {
            id: 0,
            kind: container::StreamKind::Video,
            codec: 1,
            timescale: container::TIMEBASE,
            param0: 320,
            param1: 180,
            flags: 0,
            meta0: 0x0001_0000,
        }],
        epochs: vec![],
    };
    let encoded = container::encode_front(&bad_video);
    assert!(matches!(
        container::decode_front(&encoded, 1),
        Err(container::ContainerError::BadFront)
    ));

    let bad_audio = container::Front {
        streams: vec![container::StreamDesc {
            id: 1,
            kind: container::StreamKind::Audio,
            codec: 1,
            timescale: container::TIMEBASE,
            param0: 48_000,
            param1: 2,
            flags: 1,
            meta0: 0,
        }],
        epochs: vec![],
    };
    let encoded = container::encode_front(&bad_audio);
    assert!(matches!(
        container::decode_front(&encoded, 1),
        Err(container::ContainerError::BadFront)
    ));
}

#[test]
fn combined_stream_decoder_matches_direct_codec_on_fragmented_input() {
    let source = frame();
    let encoded = video::encode(
        77,
        &source,
        &[],
        video::EncodeOptions {
            qstep: 1,
            motion_radius: 0,
            max_refs: 0,
            preset: video::EncoderPreset::Balanced,
            allow_palette: true,
        },
    )
    .unwrap();
    let mut epoch = Vec::new();
    container::encode_packet(
        &container::Packet {
            kind: container::PacketKind::EpochStart,
            flags: 0,
            stream_id: 0,
            pts: 0,
            duration: 33_333,
            payload: 9u32.to_le_bytes().to_vec(),
        },
        &mut epoch,
    );
    container::encode_packet(
        &container::Packet {
            kind: container::PacketKind::VideoFrame,
            flags: 0,
            stream_id: 1,
            pts: 0,
            duration: 33_333,
            payload: encoded.packet,
        },
        &mut epoch,
    );
    let file = container::build_file_checked(
        vec![container::StreamDesc {
            id: 1,
            kind: container::StreamKind::Video,
            codec: 1,
            timescale: container::TIMEBASE,
            param0: source.width,
            param1: source.height,
            flags: 30u32 << 16 | 1,
            meta0: 0,
        }],
        vec![(9, 0, 33_333, epoch)],
    )
    .unwrap();

    let mut decoder = container::ContainerStreamDecoder::new();
    let mut epochs = 0;
    let mut videos = 0;
    let mut state = 0x9e37_79b9u32;
    let mut off = 0;
    while off < file.len() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let n = (1 + state as usize % 113).min(file.len() - off);
        decoder
            .push_each(&file[off..off + n], |output| {
                match output {
                    container::DecodedOutput::EpochStart { id, .. } => {
                        assert_eq!(id, 9);
                        epochs += 1;
                    }
                    container::DecodedOutput::Video {
                        frame_id,
                        frame: decoded,
                        ..
                    } => {
                        assert_eq!(frame_id, 77);
                        assert_eq!(decoded.as_ref(), &source);
                        videos += 1;
                    }
                    _ => {}
                }
                Ok::<(), std::convert::Infallible>(())
            })
            .unwrap();
        off += n;
    }
    decoder.finish().unwrap();
    assert_eq!((epochs, videos), (1, 1));
}

#[test]
fn stream_parser_rejects_media_packet_routed_to_wrong_stream_kind() {
    let mut epoch = Vec::new();
    container::encode_packet(
        &container::Packet {
            kind: container::PacketKind::EpochStart,
            flags: 0,
            stream_id: 0,
            pts: 0,
            duration: 1,
            payload: 0u32.to_le_bytes().to_vec(),
        },
        &mut epoch,
    );
    container::encode_packet(
        &container::Packet {
            kind: container::PacketKind::VideoFrame,
            flags: 0,
            stream_id: 2,
            pts: 0,
            duration: 1,
            payload: vec![0; 4],
        },
        &mut epoch,
    );
    let file = container::build_file(
        vec![container::StreamDesc {
            id: 2,
            kind: container::StreamKind::Audio,
            codec: 1,
            timescale: container::TIMEBASE,
            param0: 48_000,
            param1: 2,
            flags: 0,
            meta0: 0,
        }],
        vec![(0, 0, 1, epoch)],
    );
    let mut parser = container::StreamParser::default();
    assert!(matches!(
        parser.push(&file),
        Err(container::ContainerError::BadStream)
    ));
}

fn epoch_file_with_packet(
    stream: container::StreamDesc,
    kind: container::PacketKind,
    payload: Vec<u8>,
) -> Vec<u8> {
    let epoch_id = 41u32;
    let mut epoch = Vec::new();
    container::encode_packet(
        &container::Packet {
            kind: container::PacketKind::EpochStart,
            flags: 0,
            stream_id: 0,
            pts: 0,
            duration: 1,
            payload: epoch_id.to_le_bytes().to_vec(),
        },
        &mut epoch,
    );
    container::encode_packet(
        &container::Packet {
            kind,
            flags: 0,
            stream_id: stream.id,
            pts: 0,
            duration: 1,
            payload,
        },
        &mut epoch,
    );
    container::build_file_checked(vec![stream], vec![(epoch_id, 0, 1, epoch)]).unwrap()
}

fn assert_stream_bad_stream(file: &[u8]) {
    let mut decoder = container::ContainerStreamDecoder::new();
    let result = decoder.push_each(file, |_| Ok::<(), std::convert::Infallible>(()));
    assert!(matches!(
        result,
        Err(container::StreamDecodeError::Container(
            container::ContainerError::BadStream
        ))
    ));
}

#[test]
fn combined_stream_decoder_rejects_unsupported_codec_id() {
    let source = frame();
    let payload = video::encode(1, &source, &[], video::EncodeOptions::default())
        .unwrap()
        .packet;
    let file = epoch_file_with_packet(
        container::StreamDesc {
            id: 3,
            kind: container::StreamKind::Video,
            codec: 2,
            timescale: container::TIMEBASE,
            param0: source.width,
            param1: source.height,
            flags: 0,
            meta0: 0,
        },
        container::PacketKind::VideoFrame,
        payload,
    );
    assert_stream_bad_stream(&file);
}

#[test]
fn combined_stream_decoder_rejects_video_descriptor_mismatch() {
    let source = frame();
    let payload = video::encode(1, &source, &[], video::EncodeOptions::default())
        .unwrap()
        .packet;
    let file = epoch_file_with_packet(
        container::StreamDesc {
            id: 4,
            kind: container::StreamKind::Video,
            codec: 1,
            timescale: container::TIMEBASE,
            param0: source.width + 2,
            param1: source.height,
            flags: 0,
            meta0: 0,
        },
        container::PacketKind::VideoFrame,
        payload,
    );
    assert_stream_bad_stream(&file);
}

#[test]
fn combined_stream_decoder_rejects_audio_descriptor_mismatch() {
    let pcm: Vec<i16> = (0..256).map(|i| (i as i16).wrapping_mul(131)).collect();
    let payload = audio::encode(
        &pcm,
        audio::EncodeOptions {
            sample_rate: 48_000,
            channels: 1,
            qstep: 1,
            mid_side: false,
        },
    )
    .unwrap();
    let file = epoch_file_with_packet(
        container::StreamDesc {
            id: 5,
            kind: container::StreamKind::Audio,
            codec: 1,
            timescale: container::TIMEBASE,
            param0: 44_100,
            param1: 2,
            flags: 0,
            meta0: 0,
        },
        container::PacketKind::AudioFrame,
        payload,
    );
    assert_stream_bad_stream(&file);
}
