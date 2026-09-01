# Browser sample media

The browser demo keeps a small local fixture set so playback and CI do not depend on third-party hosting. These are regression fixtures, not a representative visual-quality corpus.

| File | Dimensions | Purpose | Provenance |
| --- | ---: | --- | --- |
| `demo.avl` | 320×180 | Primary browser A/V baseline; 60 ALV1 frames plus 100 ALA1 packets | Pre-existing project-generated fixture retained from the 0.1.1 tree. The original snapshot did not retain its source-generation recipe. |
| `motion.avl` | 160×90 | Small motion/texture stress vector; 90 frames | Fully procedural; regenerate with `scripts/generate-demo-fixtures.py`. |
| `screen.avl` | 160×90 | Small UI/text/high-edge stress vector; 90 frames | Fully procedural; regenerate with `scripts/generate-demo-fixtures.py`. |

`motion.avl` and `screen.avl` are intentionally tiny source-controlled test vectors. They are useful for deterministic browser, seek, Range, renderer, and codec regression checks, but their resolution is too low for judging normal playback quality. The demo labels them accordingly and accepts arbitrary local `.avl` files or HTTP Range sources for larger media.

The procedural vectors are encoded by the canonical CLI at `q=96`, `balanced`. The generator contains the complete YUV source construction, so no external media or licensing assumption is involved. `scripts/dev-check.sh` verifies that the committed files still reproduce byte-for-byte.
