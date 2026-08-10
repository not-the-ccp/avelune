# V1 experiment: explicit inter skip modes — rejected

Candidate syntax added a one-byte implicit `ref0, mv=(0,0), residual=0` mode and an
explicit-reference/motion zero-residual mode. The existing token stream already lets static
inter blocks collapse very efficiently under frame-local static rANS. On the real-media
corpus the new modes were neutral/slightly larger while reconstruction was identical:

- `world`, q96: 137,514 B -> 137,700 B
- `world`, q128: 103,083 B -> 103,237 B
- `example-movie`, q96: 37,394 B -> 37,689 B
- `lion-pan`, q768: 88,705 B -> 88,782 B

The syntax was removed before V1. The result is useful evidence that adding symbolic modes
without accounting for entropy-model interaction can make the format worse even when the
raw token sequence looks shorter.
