import {createAveluneVideoEncoder} from './avelune-loader.js';
import {parseY4m} from './y4m.js';


self.addEventListener('message', async event => {
  if (event.data?.type !== 'encode') return;
  const {buffer, qstep, preset, artifact} = event.data;
  let encoder;
  try {
    const y4m = parseY4m(new Uint8Array(buffer));
    const fps = y4m.fpsN / y4m.fpsD;
    const epochFrames = Math.max(1, Math.round(fps * 2));
    encoder = await createAveluneVideoEncoder({
      width: y4m.width,
      height: y4m.height,
      fpsN: y4m.fpsN,
      fpsD: y4m.fpsD,
      qstep,
      preset,
      epochFrames,
      chromaLocation: y4m.chromaLocation,
      fullRange: y4m.fullRange,
    }, {artifact});
    let lastProgressReport = performance.now();
    for (let i = 0; i < y4m.frames.length; i++) {
      encoder.pushFrame(y4m.frames[i]);
      const now = performance.now();
      if (now - lastProgressReport >= 100 || i + 1 === y4m.frames.length) {
        lastProgressReport = now;
        self.postMessage({type: 'progress', done: i + 1, total: y4m.frames.length});
      }
    }
    const encoded = encoder.finish();
    self.postMessage(
      {type: 'done', encoded: encoded.buffer, frames: y4m.frames.length, artifact: encoder.artifact},
      [encoded.buffer],
    );
  } catch (error) {
    self.postMessage({type: 'error', message: error?.message ?? String(error)});
  } finally {
    encoder?.destroy();
  }
});
