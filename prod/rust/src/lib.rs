//! Avelune production backend.
//!
//! Normative decoded semantics remain under `spec/`; this crate is an optimized,
//! stateful implementation and does not define the format.
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
pub use avelune_prod_kernels as kernels;
