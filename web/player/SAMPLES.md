# Browser sample media

The demo keeps a small local fixture set so playback does not depend on third-party hosting.

| File | Purpose | Provenance |
| --- | --- | --- |
| `demo.avl` | 320×180 A/V baseline used by longstanding decode/WASM regression tests | Pre-existing project-generated fixture retained from the 0.1.1 tree. It contains 60 ALV1 frames plus 100 ALA1 packets; the original snapshot did not retain its source-generation recipe. |
| `motion.avl` | 160×90, 90-frame motion/texture workload | Fully procedural; regenerate with `scripts/generate-demo-fixtures.py`. |
| `screen.avl` | 160×90, 90-frame UI/text/high-edge workload | Fully procedural; regenerate with `scripts/generate-demo-fixtures.py`. |

`motion.avl` and `screen.avl` are encoded by the canonical CLI at `q=96`, `balanced`. The generator
contains the complete YUV source construction, so no external media or licensing assumption is
involved. `scripts/dev-check.sh` verifies that the committed files still reproduce byte-for-byte.
