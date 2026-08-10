# Browser streaming contract

The browser API must be designed around incremental delivery before the production WASM ABI is frozen.

Rejected API shape:

```
decode_entire_file(bytes) -> all_frames
```

Required conceptual API:

```
create_decoder(config)
push_container_bytes(fragment)
pop_video_frame()
pop_audio_samples()
seek_reset(epoch_metadata)
```

`push_container_bytes` may receive an arbitrary fragment boundary, including one byte. The demux parser already has a regression test for one-byte fragments.

The first WASM crate therefore exports only tiny build/conformance functions. This is intentional: a fake whole-file buffer ABI would create an implementation convenience that pressures the specification in the wrong direction.

## V1 player buffering rule

The V1 static player fetches one indexed epoch at a time. It schedules that epoch's audio packets, then decodes and renders video frames sequentially at presentation time. It does **not** decode an entire epoch into a frame queue. Thus decoded-video memory is approximately one presented frame plus the codec's bounded four-reference history, rather than `epoch_frames * frame_size`.
