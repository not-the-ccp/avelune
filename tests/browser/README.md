# Browser import fixture

`import-smoke.mp4` is a tiny self-generated H.264/AAC file used only to prove that the browser demo accepts ordinary media, preserves audio, encodes through the Avelune WebAssembly ABI, and decodes the resulting `.avl`.

It can be regenerated with:

```sh
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i 'testsrc2=size=96x54:rate=8:duration=0.75' \
  -f lavfi -i 'sine=frequency=440:sample_rate=32000:duration=0.75' \
  -c:v libx264 -preset ultrafast -crf 35 -pix_fmt yuv420p \
  -c:a aac -b:a 32k -shortest tests/browser/import-smoke.mp4
```

The fixture is deliberately tiny; it is not showcase media or a codec-quality sample.
