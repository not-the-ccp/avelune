use std::io::{self, Write};

use crate::{
    codec,
    error::{CliError, Result},
};
use avelune::container::v1::{self as container, PacketKind, StreamKind};

pub fn inspect(bytes: &[u8]) -> Result<()> {
    let (header, front, prefix) = container::parse_file_prefix(bytes)?;
    println!(
        "Avelune Draft Gen 1 bytes={} prefix={} streams={} epochs={}",
        bytes.len(),
        prefix,
        header.stream_count,
        front.epochs.len()
    );
    for stream in &front.streams {
        println!(
            "stream {} {:?} codec={} timebase={} p0={} p1={} flags={:#x} meta={:#x}",
            stream.id,
            stream.kind,
            stream.codec,
            stream.timescale,
            stream.param0,
            stream.param1,
            stream.flags,
            stream.meta0
        );
    }
    for epoch in &front.epochs {
        println!(
            "epoch {} pts={} duration={} offset={} len={}",
            epoch.id, epoch.pts, epoch.duration, epoch.offset, epoch.len
        );
        let end = epoch
            .offset
            .checked_add(epoch.len)
            .ok_or_else(|| CliError::message("epoch byte range overflow"))?;
        if end > bytes.len() as u64 {
            return Err(CliError::message(format!(
                "epoch {} points outside the file",
                epoch.id
            )));
        }
    }
    Ok(())
}

pub fn verify(bytes: &[u8]) -> Result<()> {
    inspect(bytes)?;
    let summary = codec::verify(bytes)?;
    println!("verified epochs={}", summary.epochs);
    for (id, count) in summary.video_frames {
        println!("verified video stream={id} frames={count}");
    }
    for (id, count) in summary.audio_packets {
        println!("verified audio stream={id} packets={count}");
    }
    Ok(())
}

pub fn frames(bytes: &[u8]) -> Result<()> {
    let (_, _, mut pos) = container::parse_file_prefix(bytes)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    while pos < bytes.len() {
        let (packet, used) =
            container::decode_packet(&bytes[pos..], container::DEFAULT_MAX_PACKET)?;
        if packet.kind == PacketKind::VideoFrame
            && packet.payload.len() >= 20
            && packet.payload[..4] == avelune::video::v1::CODEC_MAGIC
        {
            let id = u64::from_le_bytes(packet.payload[4..12].try_into().expect("length checked"));
            let q = u16::from_le_bytes(packet.payload[16..18].try_into().expect("length checked"));
            let refs = packet.payload[18];
            if let Err(e) = writeln!(
                out,
                "stream={} frame={} pts={} q={} refs={} packet={}",
                packet.stream_id,
                id,
                packet.pts,
                q,
                refs,
                packet.payload.len()
            ) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(CliError::io("write frame listing", e));
            }
        }
        pos += used;
    }
    Ok(())
}

pub fn reindex(bytes: &[u8], repair: bool) -> Result<Vec<u8>> {
    let (_, front, prefix) = container::parse_file_prefix(bytes)?;
    let mut groups = Vec::<(u32, u64, u32, Vec<u8>)>::new();
    let mut pos = prefix;
    let mut current = None::<(u32, u64, u32, Vec<u8>)>;
    while pos < bytes.len() {
        match container::decode_packet(&bytes[pos..], container::DEFAULT_MAX_PACKET) {
            Ok((packet, used)) => {
                if packet.kind == PacketKind::EpochStart {
                    if let Some(group) = current.take() {
                        groups.push(group);
                    }
                    if packet.payload.len() != 4 {
                        return Err(CliError::message(
                            "recoverable EpochStart has invalid payload",
                        ));
                    }
                    let id =
                        u32::from_le_bytes(packet.payload[..4].try_into().expect("length checked"));
                    current = Some((id, packet.pts, packet.duration, Vec::new()));
                }
                let Some(group) = current.as_mut() else {
                    if repair {
                        pos += used;
                        continue;
                    }
                    return Err(CliError::message(
                        "media packet appears before the first EpochStart",
                    ));
                };
                group.3.extend_from_slice(&bytes[pos..pos + used]);
                pos += used;
            }
            Err(error) if repair => {
                let bad = pos;
                pos += 1;
                while pos + container::PACKET_MAGIC.len() <= bytes.len()
                    && bytes[pos..pos + 4] != container::PACKET_MAGIC
                {
                    pos += 1;
                }
                eprintln!("repair: skipped bytes starting at {bad} after {error}");
            }
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(group) = current {
        groups.push(group);
    }
    if groups.is_empty() {
        return Err(CliError::message("no recoverable epochs"));
    }
    // Keep only stream descriptors actually understood by the canonical parser; checked building
    // revalidates every recovered packet and epoch instead of trusting the old front index.
    if front
        .streams
        .iter()
        .any(|s| !matches!(s.kind, StreamKind::Video | StreamKind::Audio))
    {
        return Err(CliError::message("unsupported stream kind in front index"));
    }
    Ok(container::build_file_checked(front.streams, groups)?)
}
