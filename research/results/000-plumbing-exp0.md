# Experiment 0 bring-up results

Date: 2026-08-10
Status: completed; plumbing experiment retained, coding method rejected as a compression candidate.

## Video

Source generated with FFmpeg testsrc2, 320x180, 30 fps, 2 s, yuv420p.

- source Y4M: 5184418 bytes
- Avelune Experiment 0 container: 5262273 bytes
- ratio: 1.0150
- decoded Y4M compared byte-for-byte equal to the source.
- two independent epochs were used (30 frames each).

Conclusion: the sample-wise varint residual scheme is useful for exact semantic/container testing but is not a viable compression design. It expands this motion-heavy synthetic source. It MUST NOT graduate into the serious candidate merely because it is implemented.

## Audio

Source: 48 kHz, stereo, signed 16-bit PCM, 997 Hz sine, 1 s.

- source PCM: 192000 bytes
- Experiment 0 predictive stream: 181591 bytes
- ratio: 0.9458
- decoded PCM compared byte-for-byte equal to source.

Conclusion: first-order prediction is an adequate lossless plumbing test and obtains modest compression on this highly predictable source. It says nothing about the planned perceptual transform codec.

## Parser / container

- incremental parser passed a regression test with every input fragment exactly one byte long;
- CRC-32C implementation matches the standard `123456789` vector (0xe3069283);
- deliberate payload corruption is detected;
- packet length is bounded before payload allocation.

## Build

- native Rust 1.97.1 workspace tests passed offline;
- release CLI built successfully;
- `avelune-wasm-core` built for `wasm32-unknown-unknown` successfully;
- current WASM smoke module is intentionally tiny because the incremental browser buffer ABI is not yet specified.
