# CLI guide

`avelune` is the command-line front end for the current reference/research implementation. It is
intended to make the codec/container POC easy to inspect and experiment with; it is **not** the future
production-performance backend.

Run `avelune <command> --help` for the authoritative option list.

## Ordinary media workflow

Avelune deliberately does not implement H.264/AV1/AAC/etc. `encode`, `decode`, and `play` use FFmpeg
for foreign media I/O.

```sh
avelune encode input.mkv output.avl
avelune verify output.avl
avelune decode output.avl roundtrip.mkv
```

Useful encode options:

```text
--seconds N       encode a prefix of the source
--size WxH        resize before reference encoding
--video-q N       experimental ALV1 quantizer step
--audio-q N       experimental ALA1 quantizer step; 1 is mathematically lossless
--epoch N         maximum epoch length in video frames
--preset NAME     fast | balanced | quality reference search preset
```

`--video-q` and `--audio-q` are codec experiment knobs, **not** CRF values or bitrate targets. Do not
assume a number has comparable meaning to x264/x265/AV1/Opus settings.

### Playback

```sh
avelune play movie.avl
```

This is a convenience/reference path. It may decode/buffer/transcode through FFmpeg before launching
`ffplay`; it is not a low-latency native Avelune player and should not be used to judge the eventual
production architecture.

## Inspection and maintenance

```sh
avelune inspect movie.avl
avelune verify movie.avl
avelune frames movie.avl
avelune benchmark movie.avl
avelune reindex input.avl reindexed.avl
avelune repair damaged.avl recovered.avl
```

- `inspect` parses the file header/front index and prints streams and epochs.
- `verify` additionally decodes media payloads.
- `frames` prints compact video-frame metadata and is pipe-friendly.
- `benchmark` measures scalar reference video decoding only.
- `reindex` rebuilds the front index from intact packets.
- `repair` performs best-effort packet-magic resynchronization; it is not forensic recovery software.

## Raw/reference workflows

Raw subcommands avoid the ordinary-media front end where possible:

```sh
avelune raw encode-y4m input.y4m output.avl --q 96
avelune raw decode-y4m input.avl output.y4m
avelune raw encode-audio input.wav output.avl --q 1
avelune raw decode-audio input.avl output.s16le
```

Draft Generation 1 video accepts 8-bit 4:2:0 Y4M only. Decoded raw audio is interleaved signed
16-bit little-endian PCM.

## Conformance and robustness

```sh
avelune conformance dist/conformance
avelune fuzz-smoke seed.avl 5000
```

`conformance` generates deterministic streams plus expected decoded outputs and cross-checks video
through both the primary and source-separated scalar decoder.

`fuzz-smoke` is deterministic mutation testing. It is useful for regression checks but does not
replace a sustained coverage-guided fuzz campaign.

## Shell completions

Completion scripts are generated from the same command model as `--help`:

```sh
avelune completions bash > ~/.local/share/bash-completion/completions/avelune
avelune completions zsh  > ~/.zfunc/_avelune
avelune completions fish > ~/.config/fish/completions/avelune.fish
avelune completions powershell > avelune.ps1
avelune completions elvish > avelune.elv
```

## Color and non-interactive use

Clap automatically uses terminal color where appropriate. Set the conventional `NO_COLOR`
environment variable to suppress the custom runtime error prefix as well.

Commands return a non-zero exit status on failure. Binary-producing raw commands support `-` where
the underlying helper accepts stdin/stdout; ordinary FFmpeg-facing commands expect filesystem paths.
