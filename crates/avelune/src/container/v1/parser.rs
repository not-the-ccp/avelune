use super::wire::{
    StreamIndex, build_stream_index, decode_front, decode_header, packet_stream_indexed,
};
use super::*;

/// Borrowed view of one validated packet in a contiguous input range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketView<'a> {
    /// Packet category.
    pub kind: PacketKind,
    /// Packet flags (zero in V1).
    pub flags: u8,
    /// Target stream identifier.
    pub stream_id: u16,
    /// Presentation timestamp.
    pub pts: u64,
    /// Packet duration.
    pub duration: u32,
    /// Borrowed codec/metadata payload.
    pub payload: &'a [u8],
}
impl PacketView<'_> {
    /// Copies this view into an owned packet, used only when ownership crosses a stream-buffer mutation.
    pub fn to_owned(self) -> Packet {
        Packet {
            kind: self.kind,
            flags: self.flags,
            stream_id: self.stream_id,
            pts: self.pts,
            duration: self.duration,
            payload: self.payload.to_vec(),
        }
    }
}

/// Zero-copy parser for one packet already resident in a contiguous byte range.
pub fn decode_packet_view_with(
    b: &[u8],
    max_packet: usize,
    kernels: avelune_kernels::KernelSet,
) -> Result<(PacketView<'_>, usize), ContainerError> {
    if b.len() < PACKET_HEADER_LEN {
        return Err(ContainerError::UnexpectedEof);
    }
    if b[..4] != PACKET_MAGIC {
        return Err(ContainerError::BadMagic);
    }
    if kernels.crc32c(&b[..24]) != u32::from_le_bytes(b[24..28].try_into().unwrap()) {
        return Err(ContainerError::PacketHeaderChecksum);
    }
    if b[5] != 0 {
        return Err(ContainerError::BadHeader);
    }
    let len = u32::from_le_bytes(b[20..24].try_into().unwrap());
    if len as usize > max_packet {
        return Err(ContainerError::PacketTooLarge(len));
    }
    let total = PACKET_HEADER_LEN
        .checked_add(len as usize)
        .and_then(|x| x.checked_add(PACKET_TRAILER_LEN))
        .ok_or(ContainerError::PacketTooLarge(len))?;
    if b.len() < total {
        return Err(ContainerError::UnexpectedEof);
    }
    let payload = &b[PACKET_HEADER_LEN..PACKET_HEADER_LEN + len as usize];
    if kernels.crc32c(payload)
        != u32::from_le_bytes(
            b[PACKET_HEADER_LEN + len as usize..total]
                .try_into()
                .unwrap(),
        )
    {
        return Err(ContainerError::PayloadChecksum);
    }
    Ok((
        PacketView {
            kind: PacketKind::try_from(b[4])?,
            flags: b[5],
            stream_id: u16::from_le_bytes([b[6], b[7]]),
            pts: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            duration: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            payload,
        },
        total,
    ))
}

/// Validates the semantic epoch boundary carried by framed packets.
///
/// The low-level parsers intentionally only frame/checksum packets. Stateful container/codec
/// integrations should feed every packet through this tracker so codec reference state cannot be
/// used before an `EpochStart`, and an indexed range can be tied to its advertised epoch ID.
#[derive(Debug, Clone, Default)]
pub struct EpochTracker {
    active: bool,
    expected: Option<u32>,
    current: Option<u32>,
}
impl EpochTracker {
    /// Starts validation of a complete file/packet sequence.
    pub fn reset_file(&mut self) {
        self.active = false;
        self.expected = None;
        self.current = None;
    }
    /// Starts validation of one independently fetched epoch range.
    pub fn reset_range(&mut self, expected_epoch: Option<u32>) {
        self.active = false;
        self.expected = expected_epoch;
        self.current = None;
    }
    /// Validates one packet. Returns the epoch ID when the packet starts a new epoch.
    pub fn observe(&mut self, packet: PacketView<'_>) -> Result<Option<u32>, ContainerError> {
        match packet.kind {
            PacketKind::EpochStart => {
                let id = u32::from_le_bytes(
                    packet
                        .payload
                        .try_into()
                        .map_err(|_| ContainerError::BadEpoch)?,
                );
                if self.expected.is_some_and(|expected| expected != id) {
                    return Err(ContainerError::BadEpoch);
                }
                self.active = true;
                self.current = Some(id);
                Ok(Some(id))
            }
            _ if !self.active => Err(ContainerError::BadEpoch),
            _ => Ok(None),
        }
    }
    /// Currently active epoch ID after a validated `EpochStart`.
    pub const fn current(&self) -> Option<u32> {
        self.current
    }
}

/// Borrowed slice parser for complete file prefixes and epoch ranges.
#[derive(Debug, Clone, Copy)]
pub struct SliceParser {
    kernels: avelune_kernels::KernelSet,
    max_packet: usize,
}
impl Default for SliceParser {
    fn default() -> Self {
        Self {
            kernels: avelune_kernels::KernelSet::auto(),
            max_packet: DEFAULT_MAX_PACKET,
        }
    }
}
impl SliceParser {
    /// Creates a slice parser with an explicit dispatch table and packet ceiling.
    pub const fn new(kernels: avelune_kernels::KernelSet, max_packet: usize) -> Self {
        Self {
            kernels,
            max_packet,
        }
    }
    /// Parses one packet without copying its payload.
    pub fn packet<'a>(&self, bytes: &'a [u8]) -> Result<(PacketView<'a>, usize), ContainerError> {
        decode_packet_view_with(bytes, self.max_packet, self.kernels)
    }
}

/// Failure from [`StreamParser::push_each`] or [`StreamParser::commit_reserved`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamPushError<E> {
    /// Container framing or resource-limit failure.
    Container(ContainerError),
    /// Error returned by the packet consumer.
    Consumer(E),
}
impl<E> From<ContainerError> for StreamPushError<E> {
    fn from(value: ContainerError) -> Self {
        Self::Container(value)
    }
}

/// Incremental parser for arbitrary fragment boundaries without per-packet front drains.
#[derive(Debug)]
pub struct StreamParser {
    buf: Vec<u8>,
    cursor: usize,
    prefix_done: bool,
    max_packet: usize,
    max_buffer: usize,
    kernels: avelune_kernels::KernelSet,
    reserved_start: Option<usize>,
    header: Option<FileHeader>,
    front: Option<Front>,
    stream_index: StreamIndex,
}
impl Default for StreamParser {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_PACKET,
            160 * 1024 * 1024,
            avelune_kernels::KernelSet::auto(),
        )
    }
}
impl StreamParser {
    /// Creates a parser from the default defensive limits and an explicit kernel set.
    pub fn with_limits(limits: crate::limits::Limits, kernels: avelune_kernels::KernelSet) -> Self {
        Self::new(
            limits.max_packet_bytes,
            limits.max_stream_buffer_bytes,
            kernels,
        )
    }
    /// Creates a bounded incremental parser.
    pub fn new(max_packet: usize, max_buffer: usize, kernels: avelune_kernels::KernelSet) -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
            prefix_done: false,
            max_packet,
            max_buffer: max_buffer.max(FIXED_HEADER_LEN),
            kernels,
            reserved_start: None,
            header: None,
            front: None,
            stream_index: StreamIndex::new(),
        }
    }
    fn unread_len(&self) -> usize {
        self.buf.len().saturating_sub(self.cursor)
    }
    fn compact(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let unread = self.unread_len();
        self.buf.copy_within(self.cursor.., 0);
        self.buf.truncate(unread);
        self.cursor = 0;
    }
    fn make_room(&mut self, incoming: usize) -> Result<(), ContainerError> {
        if self.cursor > 64 * 1024
            && (self.cursor >= self.buf.len() / 2
                || self.buf.len().saturating_add(incoming) > self.max_buffer)
        {
            self.compact();
        }
        if self
            .unread_len()
            .checked_add(incoming)
            .ok_or(ContainerError::ResourceLimit)?
            > self.max_buffer
        {
            return Err(ContainerError::ResourceLimit);
        }
        Ok(())
    }
    /// Reserves writable tail storage for a transport fragment without an intermediate copy.
    /// The caller must follow with [`Self::commit_reserved`] before reserving again.
    pub fn reserve_fragment(&mut self, len: usize) -> Result<&mut [u8], ContainerError> {
        if self.reserved_start.is_some() {
            return Err(ContainerError::ParserState);
        }
        self.make_room(len)?;
        let start = self.buf.len();
        let end = start
            .checked_add(len)
            .ok_or(ContainerError::ResourceLimit)?;
        self.buf.resize(end, 0);
        self.reserved_start = Some(start);
        Ok(&mut self.buf[start..end])
    }

    fn parse_available_each_context<E>(
        &mut self,
        mut consume: impl FnMut(PacketView<'_>, Option<&StreamDesc>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        if !self.prefix_done {
            if self.unread_len() < FIXED_HEADER_LEN {
                return Ok(());
            }
            let start = self.cursor;
            let h = decode_header(&self.buf[start..start + FIXED_HEADER_LEN])?;
            self.header = Some(h.clone());
            let n = FIXED_HEADER_LEN
                .checked_add(h.front_len as usize)
                .ok_or(ContainerError::BadHeader)?;
            if n > self.max_buffer {
                return Err(ContainerError::ResourceLimit.into());
            }
            if self.unread_len() < n {
                return Ok(());
            }
            let front = decode_front(
                &self.buf[start + FIXED_HEADER_LEN..start + n],
                h.stream_count,
            )?;
            self.stream_index = build_stream_index(&front.streams)?;
            self.front = Some(front);
            self.cursor += n;
            self.prefix_done = true;
        }
        loop {
            if self.unread_len() == 0 {
                break;
            }
            let start = self.cursor;
            match decode_packet_view_with(&self.buf[start..], self.max_packet, self.kernels) {
                Ok((view, n)) => {
                    let stream = if let Some(front) = &self.front {
                        packet_stream_indexed(front, &self.stream_index, view)?
                    } else {
                        None
                    };
                    consume(view, stream).map_err(StreamPushError::Consumer)?;
                    self.cursor += n;
                }
                Err(ContainerError::UnexpectedEof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        if self.cursor > 1024 * 1024 && self.cursor >= self.buf.len() / 2 {
            self.compact();
        }
        Ok(())
    }

    /// Commits at most `written` bytes from the most recent reservation and streams packet views
    /// synchronously to `consume`. Packet payloads are borrowed directly from parser storage and
    /// are valid only for the duration of the callback.
    pub fn commit_reserved<E>(
        &mut self,
        written: usize,
        mut consume: impl FnMut(PacketView<'_>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        self.commit_reserved_context(written, |packet, _stream| consume(packet))
    }

    pub(super) fn commit_reserved_context<E>(
        &mut self,
        written: usize,
        consume: impl FnMut(PacketView<'_>, Option<&StreamDesc>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        let start = self
            .reserved_start
            .take()
            .ok_or(ContainerError::ParserState)?;
        let reserved = self.buf.len().saturating_sub(start);
        if written > reserved {
            self.buf.truncate(start);
            return Err(ContainerError::ParserState.into());
        }
        self.buf.truncate(start + written);
        self.parse_available_each_context(consume)
    }

    /// Copies an arbitrary fragment into parser storage and synchronously exposes validated packet
    /// views. Integrations that can write into caller-provided storage should prefer
    /// [`Self::reserve_fragment`] plus [`Self::commit_reserved`] to avoid this copy.
    pub fn push_each<E>(
        &mut self,
        fragment: &[u8],
        consume: impl FnMut(PacketView<'_>) -> Result<(), E>,
    ) -> Result<(), StreamPushError<E>> {
        self.reserve_fragment(fragment.len())?
            .copy_from_slice(fragment);
        self.commit_reserved(fragment.len(), consume)
    }

    /// Appends an arbitrary fragment and returns owned packets for convenience.
    pub fn push(&mut self, fragment: &[u8]) -> Result<Vec<Packet>, ContainerError> {
        let mut out = Vec::new();
        match self.push_each(fragment, |view| {
            out.push(view.to_owned());
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(()) => Ok(out),
            Err(StreamPushError::Container(e)) => Err(e),
            Err(StreamPushError::Consumer(never)) => match never {},
        }
    }
    /// Returns the validated fixed header once at least the complete fixed header has arrived.
    pub fn file_header(&self) -> Option<&FileHeader> {
        self.header.as_ref()
    }
    /// Returns the validated front index once the complete file prefix has arrived.
    pub fn front_index(&self) -> Option<&Front> {
        self.front.as_ref()
    }

    /// Resets parser state for a new complete file while preserving limits and kernel dispatch.
    pub fn reset_file(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.prefix_done = false;
        self.reserved_start = None;
        self.header = None;
        self.front = None;
        self.stream_index.clear();
    }

    /// Resets the parser to accept a standalone epoch range rather than a full file prefix.
    pub fn reset_epoch_range(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.prefix_done = true;
        self.reserved_start = None;
    }
    /// Validates that the current input transaction ended on a complete packet boundary.
    ///
    /// Unlike [`Self::finish`], this does not consume the parser and is suitable for independently
    /// fetched epoch ranges where the same decoder will be reset and reused for another range.
    pub fn finish_input(&self) -> Result<(), ContainerError> {
        if self.reserved_start.is_some() {
            return Err(ContainerError::ParserState);
        }
        if self.unread_len() == 0 {
            Ok(())
        } else {
            Err(ContainerError::TrailingData)
        }
    }

    /// Finishes the stream, rejecting any incomplete trailing bytes.
    pub fn finish(self) -> Result<(), ContainerError> {
        self.finish_input()
    }
    /// Number of unread bytes currently buffered.
    pub fn buffered_bytes(&self) -> usize {
        self.unread_len()
    }
}
