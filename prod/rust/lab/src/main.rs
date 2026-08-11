use avelune_audio_v1 as ref_audio;
use avelune_prod::{
    audio::v1 as audio,
    bitstream::v1 as bitstream,
    config::{Config, CpuBackend, ThreadPolicy},
    container::v1 as container,
    kernels::KernelSet,
    video::v1 as video,
};
use avelune_video_ref_v1 as ref_video_independent;
use avelune_video_v1 as ref_video;
use std::{fmt::Write as _, fs, hint::black_box, path::PathBuf, process::Command, time::Instant};

#[derive(Debug)]
struct Sample {
    name: String,
    iterations: usize,
    median_ns: u128,
    p10_ns: u128,
    p90_ns: u128,
    checksum: u64,
    bytes: usize,
}

fn measure(
    mut f: impl FnMut() -> u64,
    name: &str,
    iterations: usize,
    warmups: usize,
    bytes: usize,
) -> Sample {
    for _ in 0..warmups {
        black_box(f());
    }
    let mut times = Vec::with_capacity(iterations);
    let mut checksum = 0u64;
    for _ in 0..iterations {
        let t = Instant::now();
        checksum ^= black_box(f());
        times.push(t.elapsed().as_nanos());
    }
    times.sort_unstable();
    let percentile = |num: usize, den: usize| times[(times.len().saturating_sub(1) * num) / den];
    Sample {
        name: name.into(),
        iterations,
        median_ns: percentile(1, 2),
        p10_ns: percentile(1, 10),
        p90_ns: percentile(9, 10),
        checksum,
        bytes,
    }
}

#[inline(never)]
fn direct_sad8x8(a: &[u8], stride: usize, b: &[u8]) -> u64 {
    let mut sum = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            sum += u64::from(a[y * stride + x].abs_diff(b[y * stride + x]));
        }
    }
    sum
}

fn xorshift_bytes(n: usize, mut x: u64) -> Vec<u8> {
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            // Mix long runs, ramps and pseudo-random detail so entropy tests are not one degenerate distribution.
            if i % 4096 < 1536 {
                ((i / 64) & 15) as u8
            } else if i % 4096 < 3072 {
                i as u8
            } else {
                x as u8
            }
        })
        .collect()
}

fn prod_frame(w: u32, h: u32) -> video::Frame420 {
    let y = xorshift_bytes((w * h) as usize, 0x6a09_e667_f3bc_c909);
    let u = xorshift_bytes((w * h / 4) as usize, 0xbb67_ae85_84ca_a73b);
    let v = xorshift_bytes((w * h / 4) as usize, 0x3c6e_f372_fe94_f82b);
    video::Frame420::from_planes(w, h, y, u, v).unwrap()
}
fn shift_plane(src: &[u8], w: usize, h: usize, dx: usize) -> Vec<u8> {
    let mut out = vec![0_u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = src[y * w + (x + dx).min(w - 1)];
        }
    }
    out
}
fn shifted_frame(f: &video::Frame420) -> video::Frame420 {
    let w = f.width as usize;
    let h = f.height as usize;
    video::Frame420::from_planes(
        f.width,
        f.height,
        shift_plane(f.y(), w, h, 2),
        shift_plane(f.u(), w / 2, h / 2, 1),
        shift_plane(f.v(), w / 2, h / 2, 1),
    )
    .unwrap()
}

fn ref_frame(f: &video::Frame420) -> ref_video::Frame420 {
    ref_video::Frame420 {
        width: f.width,
        height: f.height,
        y: f.y().to_vec(),
        u: f.u().to_vec(),
        v: f.v().to_vec(),
    }
}
fn ref_independent_frame(f: &video::Frame420) -> ref_video_independent::Frame420 {
    ref_video_independent::Frame420 {
        width: f.width,
        height: f.height,
        y: f.y().to_vec(),
        u: f.u().to_vec(),
        v: f.v().to_vec(),
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out
}
fn command_output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|x| x.status.success())
        .map(|x| String::from_utf8_lossy(&x.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut json_path = PathBuf::from("results/prod-bench.json");
    let mut csv_path = PathBuf::from("results/prod-bench.csv");
    let mut scale = 1usize;
    let mut media_path = PathBuf::from("web/player/demo.avl");
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                i += 1;
                json_path = args.get(i).ok_or("missing --json value")?.into();
            }
            "--csv" => {
                i += 1;
                csv_path = args.get(i).ok_or("missing --csv value")?.into();
            }
            "--scale" => {
                i += 1;
                scale = args.get(i).ok_or("missing --scale value")?.parse()?;
            }
            "--media" => {
                i += 1;
                media_path = args.get(i).ok_or("missing --media value")?.into();
            }
            x => return Err(format!("unknown argument: {x}").into()),
        }
        i += 1;
    }
    if scale == 0 {
        return Err("--scale must be >=1".into());
    }

    let scalar = KernelSet::scalar();
    let auto = KernelSet::auto();
    let bulk_a = xorshift_bytes(8 * 1024 * 1024, 0x1234_5678_9abc_def0);
    let mut bulk_b = bulk_a.clone();
    for i in (0..bulk_b.len()).step_by(31) {
        bulk_b[i] ^= (i as u8).wrapping_mul(17).wrapping_add(1);
    }
    let mut samples = Vec::new();
    let fast_iters = (20 * scale).max(5);
    samples.push(measure(
        || u64::from(scalar.crc32c(black_box(&bulk_a))),
        "crc32c.scalar.8MiB",
        fast_iters,
        3,
        bulk_a.len(),
    ));
    samples.push(measure(
        || u64::from(auto.crc32c(black_box(&bulk_a))),
        "crc32c.auto.8MiB",
        fast_iters,
        3,
        bulk_a.len(),
    ));
    samples.push(measure(
        || scalar.sad(black_box(&bulk_a), black_box(&bulk_b)),
        "sad.scalar.8MiB",
        fast_iters,
        3,
        bulk_a.len(),
    ));
    samples.push(measure(
        || auto.sad(black_box(&bulk_a), black_box(&bulk_b)),
        "sad.auto.8MiB",
        fast_iters,
        3,
        bulk_a.len(),
    ));

    let stride = 640usize;
    let block_a = xorshift_bytes(stride * 8, 0x1319_8a2e_0370_7344);
    let mut block_b = block_a.clone();
    for y in 0..8 {
        for x in 0..8 {
            block_b[y * stride + x] ^= ((x + y * 11) as u8).wrapping_add(1);
        }
    }
    samples.push(measure(
        || {
            scalar.sad_block(
                black_box(&block_a),
                stride,
                black_box(&block_b),
                stride,
                8,
                8,
            )
        },
        "sad8x8.scalar.stride640",
        200_000 * scale,
        100,
        64,
    ));
    samples.push(measure(
        || {
            auto.sad_block(
                black_box(&block_a),
                stride,
                black_box(&block_b),
                stride,
                8,
                8,
            )
        },
        "sad8x8.auto.stride640",
        200_000 * scale,
        100,
        64,
    ));
    samples.push(measure(
        || direct_sad8x8(black_box(&block_a), stride, black_box(&block_b)),
        "sad8x8.direct_rust.stride640",
        200_000 * scale,
        100,
        64,
    ));

    let mut transform_input = [0_i32; 64];
    for (i, v) in transform_input.iter_mut().enumerate() {
        *v = ((i as i32 * 65_537) % 2_000_001) - 1_000_000;
    }
    samples.push(measure(
        || scalar.inverse_wht8x8(black_box(transform_input))[0] as u64,
        "inverse_wht8x8.scalar",
        20_000 * scale,
        100,
        std::mem::size_of_val(&transform_input),
    ));
    samples.push(measure(
        || auto.inverse_wht8x8(black_box(transform_input))[0] as u64,
        "inverse_wht8x8.auto",
        20_000 * scale,
        100,
        std::mem::size_of_val(&transform_input),
    ));

    let entropy_src = xorshift_bytes(1024 * 1024, 0xdead_beef_fade_cafe);
    let entropy_coded = bitstream::entropy_compress(&entropy_src);
    samples.push(measure(
        || bitstream::entropy_compress(black_box(&entropy_src)).len() as u64,
        "entropy.encode.1MiB",
        5 * scale,
        1,
        entropy_src.len(),
    ));
    samples.push(measure(
        || {
            let v = bitstream::entropy_decompress(black_box(&entropy_coded), entropy_src.len())
                .unwrap();
            u64::from(v[0]) ^ (v.len() as u64)
        },
        "entropy.decode.1MiB",
        10 * scale,
        2,
        entropy_src.len(),
    ));

    // Proxy for restartable/interleaved entropy overhead: two independently restartable byte lanes.
    let mut even = Vec::with_capacity(entropy_src.len().div_ceil(2));
    let mut odd = Vec::with_capacity(entropy_src.len() / 2);
    for (i, b) in entropy_src.iter().copied().enumerate() {
        if i.is_multiple_of(2) {
            even.push(b)
        } else {
            odd.push(b)
        }
    }
    let whole = bitstream::entropy_compress(&entropy_src);
    let split_even = bitstream::entropy_compress(&even);
    let split_odd = bitstream::entropy_compress(&odd);
    let split_size = split_even.len() + split_odd.len() + 8;
    let rebuild_split = |parallel: bool| -> Vec<u8> {
        let (a, b) = if parallel {
            std::thread::scope(|scope| {
                let ea =
                    scope.spawn(|| bitstream::entropy_decompress(&split_even, even.len()).unwrap());
                let eb =
                    scope.spawn(|| bitstream::entropy_decompress(&split_odd, odd.len()).unwrap());
                (ea.join().unwrap(), eb.join().unwrap())
            })
        } else {
            (
                bitstream::entropy_decompress(&split_even, even.len()).unwrap(),
                bitstream::entropy_decompress(&split_odd, odd.len()).unwrap(),
            )
        };
        let mut out = vec![0; entropy_src.len()];
        for (i, v) in a.into_iter().enumerate() {
            out[i * 2] = v;
        }
        for (i, v) in b.into_iter().enumerate() {
            out[i * 2 + 1] = v;
        }
        out
    };
    assert_eq!(rebuild_split(false), entropy_src);
    samples.push(measure(
        || {
            let v = rebuild_split(false);
            v.len() as u64 ^ u64::from(v[0])
        },
        "entropy.decode.two_lane.serial.1MiB",
        10 * scale,
        2,
        entropy_src.len(),
    ));
    samples.push(measure(
        || {
            let v = rebuild_split(true);
            v.len() as u64 ^ u64::from(v[0])
        },
        "entropy.decode.two_lane.parallel.1MiB",
        10 * scale,
        2,
        entropy_src.len(),
    ));

    let f = prod_frame(640, 360);
    let rf = ref_frame(&f);
    let opts = video::EncodeOptions {
        qstep: 73,
        motion_radius: 0,
        max_refs: 0,
        preset: video::EncoderPreset::Balanced,
        allow_palette: true,
    };
    let ropts = ref_video::EncodeOptions {
        qstep: 73,
        motion_radius: 0,
        max_refs: 0,
        allow_palette: true,
    };
    let prod_encoded = video::encode(1, &f, &[], opts)?;
    let ref_encoded = ref_video::encode(1, &rf, &[], ropts)?;
    if prod_encoded.packet != ref_encoded.packet {
        return Err("prod/reference baseline encoder packet mismatch".into());
    }
    let (_, ind, _) = ref_video_independent::decode(&prod_encoded.packet, &[])?;
    let expected = ref_independent_frame(&prod_encoded.reconstructed);
    if ind != expected {
        return Err("independent decoder mismatch before benchmark".into());
    }
    let video_iters = scale.max(1);
    let single_scalar = Config {
        cpu: CpuBackend::Scalar,
        threads: ThreadPolicy::Single,
        ..Config::default()
    };
    let single_auto = Config {
        cpu: CpuBackend::Auto,
        threads: ThreadPolicy::Single,
        ..Config::default()
    };
    let threaded_auto = Config {
        cpu: CpuBackend::Auto,
        threads: ThreadPolicy::Auto,
        ..Config::default()
    };
    let mut scalar_encoder = video::VideoEncoder::with_config(opts, single_scalar)?;
    samples.push(measure(
        || {
            scalar_encoder.reset_epoch();
            let e = scalar_encoder.encode(1, black_box(&f)).unwrap();
            e.packet.len() as u64 ^ u64::from(e.reconstructed.y()[0])
        },
        "video.encode.prod.scalar.single.640x360.q73",
        video_iters,
        1,
        f.storage_len(),
    ));
    let mut auto_single_encoder = video::VideoEncoder::with_config(opts, single_auto)?;
    samples.push(measure(
        || {
            auto_single_encoder.reset_epoch();
            let e = auto_single_encoder.encode(1, black_box(&f)).unwrap();
            e.packet.len() as u64 ^ u64::from(e.reconstructed.y()[0])
        },
        "video.encode.prod.auto.single.640x360.q73",
        video_iters,
        1,
        f.storage_len(),
    ));
    let mut auto_threaded_encoder = video::VideoEncoder::with_config(opts, threaded_auto)?;
    samples.push(measure(
        || {
            auto_threaded_encoder.reset_epoch();
            let e = auto_threaded_encoder.encode(1, black_box(&f)).unwrap();
            e.packet.len() as u64 ^ u64::from(e.reconstructed.y()[0])
        },
        "video.encode.prod.auto.private_pool.640x360.q73",
        video_iters,
        1,
        f.storage_len(),
    ));
    samples.push(measure(
        || {
            let e = video::encode(1, black_box(&f), &[], opts).unwrap();
            e.packet.len() as u64 ^ u64::from(e.reconstructed.y()[0])
        },
        "video.encode.prod.scoped_threads.640x360.q73",
        video_iters,
        1,
        f.storage_len(),
    ));
    samples.push(measure(
        || {
            let e = ref_video::encode(1, black_box(&rf), &[], ropts).unwrap();
            e.packet.len() as u64 ^ u64::from(e.reconstructed.y[0])
        },
        "video.encode.reference.640x360.q73",
        video_iters,
        1,
        f.storage_len(),
    ));
    let f2 = shifted_frame(&f);
    let rf2 = ref_frame(&f2);
    let inter_opts = video::EncodeOptions {
        qstep: 73,
        motion_radius: 4,
        max_refs: 1,
        preset: video::EncoderPreset::Balanced,
        allow_palette: true,
    };
    let inter_ropts = ref_video::EncodeOptions {
        qstep: 73,
        motion_radius: 4,
        max_refs: 1,
        allow_palette: true,
    };
    for (name, cfg) in [
        (
            "video.encode.inter.prod.scalar.single.640x360",
            single_scalar,
        ),
        ("video.encode.inter.prod.auto.single.640x360", single_auto),
    ] {
        let mut times = Vec::with_capacity((2 * scale).max(2));
        let mut checksum = 0_u64;
        for _ in 0..(2 * scale).max(2) {
            let mut enc = video::VideoEncoder::with_config(inter_opts, cfg)?;
            let _ = enc.encode(1, &f)?;
            let t = Instant::now();
            let e = enc.encode(2, black_box(&f2))?;
            times.push(t.elapsed().as_nanos());
            checksum ^= e.packet.len() as u64 ^ u64::from(e.reconstructed.y()[0]);
        }
        times.sort_unstable();
        let percentile =
            |num: usize, den: usize| times[(times.len().saturating_sub(1) * num) / den];
        samples.push(Sample {
            name: name.into(),
            iterations: times.len(),
            median_ns: percentile(1, 2),
            p10_ns: percentile(1, 10),
            p90_ns: percentile(9, 10),
            checksum,
            bytes: f2.storage_len(),
        });
    }
    samples.push(measure(
        || {
            let refs = [(1_u64, &rf)];
            let e = ref_video::encode(2, black_box(&rf2), &refs, inter_ropts).unwrap();
            e.packet.len() as u64 ^ u64::from(e.reconstructed.y[0])
        },
        "video.encode.inter.reference.640x360",
        (2 * scale).max(2),
        1,
        f2.storage_len(),
    ));

    let mut scalar_decoder = video::VideoDecoder::with_config(single_scalar)?;
    samples.push(measure(
        || {
            scalar_decoder.reset_epoch();
            let (_, d, _) = scalar_decoder
                .decode(black_box(&prod_encoded.packet))
                .unwrap();
            d.y().len() as u64 ^ u64::from(d.y()[0])
        },
        "video.decode.prod.scalar.single.640x360.q73",
        4 * scale,
        1,
        prod_encoded.packet.len(),
    ));
    let mut auto_single_decoder = video::VideoDecoder::with_config(single_auto)?;
    samples.push(measure(
        || {
            auto_single_decoder.reset_epoch();
            let (_, d, _) = auto_single_decoder
                .decode(black_box(&prod_encoded.packet))
                .unwrap();
            d.y().len() as u64 ^ u64::from(d.y()[0])
        },
        "video.decode.prod.auto.single.640x360.q73",
        4 * scale,
        1,
        prod_encoded.packet.len(),
    ));
    let mut auto_threaded_decoder = video::VideoDecoder::with_config(threaded_auto)?;
    samples.push(measure(
        || {
            auto_threaded_decoder.reset_epoch();
            let (_, d, _) = auto_threaded_decoder
                .decode(black_box(&prod_encoded.packet))
                .unwrap();
            d.y().len() as u64 ^ u64::from(d.y()[0])
        },
        "video.decode.prod.auto.private_pool.640x360.q73",
        4 * scale,
        1,
        prod_encoded.packet.len(),
    ));
    samples.push(measure(
        || {
            let (_, d, _) = video::decode(black_box(&prod_encoded.packet), &[]).unwrap();
            d.y().len() as u64 ^ u64::from(d.y()[0])
        },
        "video.decode.prod.scoped_threads.640x360.q73",
        4 * scale,
        1,
        prod_encoded.packet.len(),
    ));
    samples.push(measure(
        || {
            let (_, d, _) = ref_video::decode(black_box(&prod_encoded.packet), &[]).unwrap();
            d.y.len() as u64 ^ u64::from(d.y[0])
        },
        "video.decode.reference.640x360.q73",
        4 * scale,
        1,
        prod_encoded.packet.len(),
    ));
    samples.push(measure(
        || {
            let (_, d, _) =
                ref_video_independent::decode(black_box(&prod_encoded.packet), &[]).unwrap();
            d.y.len() as u64 ^ u64::from(d.y[0])
        },
        "video.decode.independent_ref.640x360.q73",
        4 * scale,
        1,
        prod_encoded.packet.len(),
    ));

    let pcm: Vec<i16> = (0..4096 * 2)
        .map(|i| (((i as i64 * 811) % 60001) - 30000) as i16)
        .collect();
    let aopts = audio::EncodeOptions {
        sample_rate: 48_000,
        channels: 2,
        qstep: 37,
        mid_side: true,
    };
    let raopts = ref_audio::EncodeOptions {
        sample_rate: 48_000,
        channels: 2,
        qstep: 37,
        mid_side: true,
    };
    let audio_packet = audio::encode(&pcm, aopts)?;
    samples.push(measure(
        || audio::encode(black_box(&pcm), aopts).unwrap().len() as u64,
        "audio.encode.prod.4096x2.q37",
        10 * scale,
        2,
        pcm.len() * 2,
    ));
    samples.push(measure(
        || ref_audio::encode(black_box(&pcm), raopts).unwrap().len() as u64,
        "audio.encode.reference.4096x2.q37",
        10 * scale,
        2,
        pcm.len() * 2,
    ));
    samples.push(measure(
        || audio::decode(black_box(&audio_packet)).unwrap().2.len() as u64,
        "audio.decode.prod.4096x2.q37",
        15 * scale,
        2,
        audio_packet.len(),
    ));
    let mut reusable_audio_decoder = audio::AudioDecoder::new();
    let mut reusable_pcm = Vec::new();
    samples.push(measure(
        || {
            let _ = reusable_audio_decoder
                .decode_into(black_box(&audio_packet), &mut reusable_pcm)
                .unwrap();
            reusable_pcm.len() as u64 ^ reusable_pcm.first().copied().unwrap_or_default() as u64
        },
        "audio.decode_into.prod.reuse.4096x2.q37",
        15 * scale,
        2,
        audio_packet.len(),
    ));
    samples.push(measure(
        || ref_audio::decode(black_box(&audio_packet)).unwrap().2.len() as u64,
        "audio.decode.reference.4096x2.q37",
        15 * scale,
        2,
        audio_packet.len(),
    ));

    let media = fs::read(&media_path)?;
    let (file_header, file_front, prefix_len) = container::parse_file_prefix(&media)?;
    let slice_parser = container::SliceParser::default();
    samples.push(measure(
        || {
            let mut packets = 0u64;
            // Parsing the immutable prefix once is representative of a mapped/contiguous file;
            // packet payloads remain borrowed from the input for the entire measurement.
            black_box(file_header.stream_count);
            for epoch in &file_front.epochs {
                let start = usize::try_from(epoch.offset).unwrap();
                let end = usize::try_from(epoch.offset + epoch.len).unwrap();
                let mut range = &media[start..end];
                while !range.is_empty() {
                    let (_, used) = slice_parser.packet(range).unwrap();
                    packets += 1;
                    range = &range[used..];
                }
            }
            packets ^ prefix_len as u64
        },
        "container.slice.demo.borrowed",
        20 * scale,
        3,
        media.len(),
    ));
    samples.push(measure(
        || {
            let mut parser = container::StreamParser::default();
            let mut packets = 0u64;
            for chunk in media.chunks(64 * 1024) {
                packets += parser.push(chunk).unwrap().len() as u64;
            }
            parser.finish().unwrap();
            packets
        },
        "container.stream.demo.64KiB",
        20 * scale,
        3,
        media.len(),
    ));

    let rustc = command_output("rustc", &["--version"]);
    let commit = command_output("git", &["rev-parse", "HEAD"]);
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|x| x.starts_with("model name"))
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "unavailable".into());
    let features = if cfg!(target_arch = "x86_64") {
        format!(
            "sse4.2={} avx2={} avx512f={}",
            std::arch::is_x86_feature_detected!("sse4.2"),
            std::arch::is_x86_feature_detected!("avx2"),
            std::arch::is_x86_feature_detected!("avx512f")
        )
    } else {
        "non-x86_64".into()
    };

    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = csv_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut csv =
        String::from("name,iterations,median_ns,p10_ns,p90_ns,bytes,checksum,throughput_MiB_s\n");
    for x in &samples {
        let mib_s = if x.median_ns == 0 {
            0.0
        } else {
            (x.bytes as f64 / 1048576.0) / (x.median_ns as f64 / 1e9)
        };
        writeln!(
            csv,
            "{},{},{},{},{},{},{},{:.3}",
            x.name, x.iterations, x.median_ns, x.p10_ns, x.p90_ns, x.bytes, x.checksum, mib_s
        )?;
    }
    fs::write(&csv_path, csv)?;
    let mut json = format!(
        "{{\n  \"commit\": \"{}\",\n  \"rustc\": \"{}\",\n  \"cpu\": \"{}\",\n  \"features\": \"{}\",\n  \"kernel_backend\": \"{:?}\",\n  \"entropy_restart_proxy\": {{\"source_bytes\":{},\"single_bytes\":{},\"two_lane_bytes_with_headers\":{},\"delta_bytes\":{}}},\n  \"samples\": [\n",
        json_escape(&commit),
        json_escape(&rustc),
        json_escape(&cpu),
        json_escape(&features),
        auto.backend(),
        entropy_src.len(),
        whole.len(),
        split_size,
        split_size as i64 - whole.len() as i64
    );
    for (i, x) in samples.iter().enumerate() {
        let comma = if i + 1 == samples.len() { "" } else { "," };
        writeln!(
            json,
            "    {{\"name\":\"{}\",\"iterations\":{},\"median_ns\":{},\"p10_ns\":{},\"p90_ns\":{},\"bytes\":{},\"checksum\":{}}}{}",
            json_escape(&x.name),
            x.iterations,
            x.median_ns,
            x.p10_ns,
            x.p90_ns,
            x.bytes,
            x.checksum,
            comma
        )?;
    }
    json.push_str("  ]\n}\n");
    fs::write(&json_path, json)?;
    println!("wrote {} and {}", json_path.display(), csv_path.display());
    println!(
        "kernel={:?}; restart-proxy single={} split={} delta={:+}",
        auto.backend(),
        whole.len(),
        split_size,
        split_size as i64 - whole.len() as i64
    );
    for x in &samples {
        println!(
            "{:<42} median {:>10} ns p10 {:>10} p90 {:>10}",
            x.name, x.median_ns, x.p10_ns, x.p90_ns
        );
    }
    Ok(())
}
