# Production backend direction

Production implementations are intentionally separate from the reference implementation.

The current target architecture is hybrid rather than “everything on the GPU”:

- CPU/WASM: parsing, control flow, entropy coding/decoding, dependency management;
- native SIMD / WASM SIMD: prediction, transform/reconstruction, color conversion, bulk pixel work;
- optional WebGPU: large regular batches such as motion-cost evaluation, transforms/reconstruction,
  color conversion, scaling/compositing, and editor-oriented GPU-resident pipelines.

`prod/rust/` is a compiling placeholder only. No optimized backend is currently claimed.
