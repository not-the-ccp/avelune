//! High-level Rust entry point for Avelune.
//!
//! Avelune is still experimental. The public software API and the Draft Generation 1
//! bitstreams (`ALV1`, `ALA1`, and the Avelune container) may change incompatibly before
//! a stable release.
//!
//! This crate is intentionally a thin facade. The component crates remain useful for
//! spec work and independent decoder development, while applications can depend on one
//! obvious package.
//!
//! # Example
//!
//! ```
//! use avelune::audio::{decode, encode, EncodeOptions};
//!
//! let pcm = vec![0_i16, 1000, -1000, 0];
//! let packet = encode(&pcm, EncodeOptions {
//!     sample_rate: 48_000,
//!     channels: 1,
//!     qstep: 1,
//!     mid_side: false,
//! })?;
//! let (_, channels, decoded) = decode(&packet)?;
//! assert_eq!(channels, 1);
//! assert_eq!(decoded, pcm);
//! # Ok::<(), avelune::audio::AudioError>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Draft Generation 1 audio codec API.
pub mod audio {
    pub use avelune_audio_v1::*;
}

/// Shared bitstream primitives used by the Draft Generation 1 codecs.
pub mod bitstream {
    pub use avelune_bitstream::*;
}

/// Avelune container API.
pub mod container {
    pub use avelune_container_v1::*;
}

/// Draft Generation 1 video codec API.
pub mod video {
    pub use avelune_video_v1::*;
}

/// Source-separated scalar video decoder used for differential conformance checks.
pub mod reference_video {
    pub use avelune_video_ref_v1::*;
}

/// Stateful production backend. This surface owns reusable codec/container state, bounded
/// scheduling, resource limits, and audited CPU-kernel dispatch while retaining Draft
/// Generation 1 reference crates as separate conformance oracles.
pub mod production {
    pub use avelune_prod::*;
}

/// Current software package version.
pub const SOFTWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Human-readable stability marker for the current public tree.
pub const STABILITY: &str = "experimental / draft bitstreams / compatibility not guaranteed";
