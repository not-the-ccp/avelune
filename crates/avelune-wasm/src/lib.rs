//! Browser-facing WebAssembly adapters for the canonical Avelune implementation.
//!
//! Decoder and encoder handles use integer IDs and caller-visible linear-memory buffers. Codec and
//! container semantics remain in `avelune`; this crate only exposes a compact browser ABI.
#![deny(unsafe_op_in_unsafe_fn)]

mod decoder;
mod encoder;
