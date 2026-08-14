import {createAveluneVideoEncoder} from './avelune-loader.js';

function lineEnd(bytes, start) {
  for (let i = start; i < bytes.length; i++) if (bytes[i] === 10) return i;
  return -1;
}

function parseY4m(bytes) {
  if (bytes.length > 256 * 1024 * 1024) throw Error('Y4M encoder lab is limited to 256 MiB inputs');
  const firstNl = lineEnd(bytes, 0);
  if (firstNl < 0) throw Error('Y4M header is truncated');
  const header = new TextDecoder().decode(bytes.subarray(0, firstNl));
  if (!header.startsWith('YUV4MPEG2 ')) throw Error('encoder lab accepts YUV4MPEG2 input');
  let width, height, fpsN = 30, fpsD = 1, chroma = '420', fullRange = false;
  for (const token of header.split(/\s+/).slice(1)) {
    if (token.startsWith('W')) width = Number(token.slice(1));
    else if (token.startsWith('H')) height = Number(token.slice(1));
    else if (token.startsWith('F')) {
      const [n, d = '1'] = token.slice(1).split(':');
      fpsN = Number(n);
      fpsD = Number(d);
    } else if (token.startsWith('C')) chroma = token.slice(1);
    else if (token.toUpperCase() === 'XCOLORRANGE=FULL') fullRange = true;
  }
  for (const [name, value] of Object.entries({width, height, fpsN, fpsD})) {
    if (!Number.isSafeInteger(value) || value <= 0) throw Error(`invalid Y4M ${name}`);
  }
  if (fpsN > 0xffff || fpsD > 0xffff) throw Error('Y4M frame-rate numerator and denominator must fit 16 bits');
  if (!['420', '420jpeg', '420mpeg2'].includes(chroma)) {
    throw Error(`encoder lab requires 8-bit 4:2:0 Y4M, got C${chroma}`);
  }
  if (width % 2 || height % 2) throw Error('Y4M 4:2:0 dimensions must be even');
  const yLen = width * height, frameBytes = yLen + yLen / 2;
  if (!Number.isSafeInteger(frameBytes)) throw Error('Y4M frame size is too large');
  const frames = [];
  let pos = firstNl + 1;
  while (pos < bytes.length) {
    const nl = lineEnd(bytes, pos);
    if (nl < 0) throw Error('truncated Y4M FRAME header');
    const marker = new TextDecoder().decode(bytes.subarray(pos, nl));
    if (marker !== 'FRAME' && !marker.startsWith('FRAME ')) throw Error(`expected Y4M FRAME header at byte ${pos}`);
    pos = nl + 1;
    if (pos + frameBytes > bytes.length) throw Error('truncated Y4M frame payload');
    frames.push(bytes.subarray(pos, pos + frameBytes));
    pos += frameBytes;
  }
  if (!frames.length) throw Error('Y4M input contains no frames');
  const chromaLocation = chroma === '420mpeg2' ? 1 : chroma === '420jpeg' ? 2 : 0;
  return {width, height, fpsN, fpsD, chromaLocation, fullRange, frames};
}

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
