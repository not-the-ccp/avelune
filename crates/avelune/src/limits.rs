//! Hostile-input and resource ceilings.

/// Defensive resource limits for parsing and decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum luma-pixel count accepted by stateful video codec instances.
    pub max_frame_pixels: u64,
    /// Maximum packet payload accepted by the container parser.
    pub max_packet_bytes: usize,
    /// Maximum decompressed entropy stream.
    pub max_entropy_bytes: usize,
    /// Maximum bytes retained by an incremental container parser.
    pub max_stream_buffer_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_pixels: 8192 * 8192,
            max_packet_bytes: 128 * 1024 * 1024,
            max_entropy_bytes: 8 * 1024 * 1024,
            max_stream_buffer_bytes: 160 * 1024 * 1024,
        }
    }
}
