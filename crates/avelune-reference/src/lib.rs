//! Independent, deliberately simple conformance implementation for Avelune Draft Generation 1.
//!
//! This crate is development/test infrastructure, not an alternate application backend. It does
//! not depend on the canonical `avelune` crate. `video_decoder` is the single ALV1 decoding oracle;
//! `video_encoder` exists only to generate independently encoded packets for reverse differential
//! tests.
#![forbid(unsafe_code)]
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::manual_checked_ops,
    clippy::manual_is_multiple_of,
    clippy::manual_div_ceil,
    clippy::manual_range_contains,
    clippy::needless_lifetimes
)]

pub mod audio;
pub mod bitstream;
pub mod container;
pub mod video_decoder;
pub mod video_encoder;
