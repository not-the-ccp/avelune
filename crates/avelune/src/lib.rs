//! Canonical Rust implementation of Avelune Draft Generation 1.
//!
//! Normative semantics remain under `spec/`; this safe, stateful implementation is
//! used by applications and does not itself define the format.
#![forbid(unsafe_code)]

pub mod audio;
pub mod bitstream;
pub mod buffer;
pub mod config;
pub mod container;
pub mod limits;
pub mod scheduler;
pub mod video;

/// Kernel dispatch and validated low-level primitives.
pub use avelune_kernels as kernels;
