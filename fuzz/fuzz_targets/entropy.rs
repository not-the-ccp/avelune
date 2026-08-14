#![no_main]
use avelune::bitstream::v1::{entropy_compress, entropy_decompress};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    let coded = entropy_compress(data);
    let decoded =
        entropy_decompress(&coded, data.len().saturating_mul(16).saturating_add(4096)).unwrap();
    assert_eq!(decoded, data);
    let _ = entropy_decompress(data, 1 << 20);
});
