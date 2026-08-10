# Draft Generation 1 reference-implementation benchmark report

Date: 2026-08-10. These measurements characterize the current Draft Generation 1 reference implementation; they are not normative codec requirements.

## Executed real-media video corpus

The release benchmark uses locally available packaged media rather than only generated Mandelbrot/test patterns:

- `world`: two seconds from a packaged 30-second 480x270 H.264/AAC Earth/CGI video, normalized to 320x180 YUV420;
- `example-movie`: two seconds from a packaged 720p timed-text/UI movie, normalized to 320x180 YUV420;
- `lion-pan`: two seconds of pan/zoom motion generated from a real photograph to exercise natural texture statistics;
- `hd1080-6f`: six native 1920x1080 frames from a packaged MP4, used as a decoder-throughput fixture.

`real-video.csv` contains the complete measured curve against FFmpeg's libx264, libx265, libvpx-vp9 and libaom-av1. All PSNR values are recomputed by FFmpeg after decoding to the same YUV420 sample domain.

Representative current points:

| source | Avelune | size / PSNR | mature comparison near quality |
|---|---|---|---|
| world | q128 | 93,541 B / 45.37 dB | AV1 CRF40 7,930 B / 45.72 dB |
| UI movie | q96 | 35,139 B / 50.52 dB | VP9 CRF40 3,610 B / 50.23 dB |
| photo pan | q768 | 57,486 B / 29.28 dB | x264 CRF20 36,159 B / 29.38 dB; VP9 CRF40 22,471 B / 29.26 dB |

Conclusion: ALV1 is a functioning research codec with useful compression, exact screen palettes and bounded streaming dependencies, but is **not competitive with mature codecs**, especially on low-complexity/CGI/UI material. The natural-texture matched-quality gap is materially smaller than early synthetic-only measurements, but still substantial.

The encoder's byte size is not perfectly monotonic in q because mode decisions are distortion-driven and can switch entropy/palette/inter choices. V1 makes no monotonic-rate guarantee.

## Standard external corpus

`scripts/fetch-xiph-corpus.sh` fetches Xiph/Derf `bus_qcif_15fps.y4m` and `foreman_qcif.y4m`; `scripts/benchmark-xiph.py` applies the same Avelune/x264/x265/VP9/AV1 methodology. The release sandbox could verify the Xiph collection metadata but could not download these binary Y4M assets, so **no Xiph result is claimed in this report**.

## Audio

`real-audio.csv` contains four-second excerpts from packaged saxophone, music, and two speech assets compared with libopus at 32/64/96/128 kb/s. `sample_snr_db` is a simple time-domain sample SNR, not a perceptual listening metric.

Representative sax result: ALA1 q128 is 210,552 B at 36.70 dB sample SNR; Opus 128 kb/s is 58,206 B at 35.75 dB by the same metric. ALA1 q1 is mathematically exact. The conclusion is straightforward: Draft Generation 1 audio establishes a custom, lossless-capable codec and streaming implementation; its lossy mode is neither competitive with Opus nor perceptually tuned.

## Decoder throughput fixture

On the local six-frame 1920x1080 fixture, the release build measures approximately 30 fps scalar native ALV1 decode and 23 fps scalar Node/V8 WASM decode on this host. The WASM measurement excludes rendering but includes bitstream decode/reconstruction. Results are hardware/runtime-specific; they replace the earlier aspirational 1080p30-WASM target.
