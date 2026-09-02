import fs from 'node:fs';
import http from 'node:http';
import {
  BlobRangeSource,
  createAveluneAvEncoder,
  createAveluneDecoder,
} from '../web/player/avelune-loader.js';
import {PcmTimeline} from '../web/player/audio-timeline.js';

const scalarPath = process.argv[2] ?? 'web/player/avelune-scalar.wasm';
const simdPath = process.argv[3] ?? 'web/player/avelune-simd128.wasm';
const wasmFiles = new Map([
  ['/scalar.wasm', fs.readFileSync(scalarPath)],
  ['/simd128.wasm', fs.readFileSync(simdPath)],
]);
const server = http.createServer((request, response) => {
  const wasm = wasmFiles.get(request.url);
  if (!wasm) { response.writeHead(404).end(); return; }
  response.writeHead(200, {'Content-Type': 'application/wasm', 'Content-Length': wasm.length});
  response.end(wasm);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const scalarUrl = `${base}/scalar.wasm`;
const simdUrl = `${base}/simd128.wasm`;

const width = 32;
const height = 24;
const fpsN = 4;
const seconds = 3;
const frameCount = fpsN * seconds;
const audioRate = 48_000;
const audioChannels = 2;
const audioFrames = audioRate * seconds;

const clamp16 = value => Math.max(-32768, Math.min(32767, Math.round(value)));

function makeFixture(kind) {
  const pcm = new Int16Array(audioFrames * audioChannels);
  for (let frame = 0; frame < audioFrames; frame++) {
    const t = frame / audioRate;
    let left;
    let right;
    if (kind === 'tone') {
      left = Math.sin(2 * Math.PI * 440 * t) * 13_000;
      right = Math.sin(2 * Math.PI * 660 * t + 0.2) * 11_000;
    } else if (kind === 'speech') {
      const syllable = 0.35 + 0.65 * Math.max(0, Math.sin(2 * Math.PI * 3.2 * t));
      const pitch = 125 + 18 * Math.sin(2 * Math.PI * 0.7 * t);
      const voiced = Math.sin(2 * Math.PI * pitch * t);
      const formants = 0.42 * Math.sin(2 * Math.PI * 730 * t) + 0.23 * Math.sin(2 * Math.PI * 1180 * t);
      left = (0.58 * voiced + formants) * syllable * 12_000;
      right = (0.56 * voiced + 0.38 * Math.sin(2 * Math.PI * 690 * t) + 0.2 * Math.sin(2 * Math.PI * 1240 * t)) * syllable * 12_000;
    } else if (kind === 'music') {
      const beatPhase = (t * 2) % 1;
      const kick = Math.exp(-beatPhase * 16) * Math.sin(2 * Math.PI * (72 - 20 * beatPhase) * t);
      const chordL = Math.sin(2 * Math.PI * 261.626 * t) + 0.7 * Math.sin(2 * Math.PI * 329.628 * t) + 0.55 * Math.sin(2 * Math.PI * 391.995 * t);
      const chordR = Math.sin(2 * Math.PI * 293.665 * t) + 0.7 * Math.sin(2 * Math.PI * 369.994 * t) + 0.55 * Math.sin(2 * Math.PI * 440 * t);
      left = (0.24 * chordL + 0.42 * kick) * 13_000;
      right = (0.24 * chordR + 0.38 * kick) * 13_000;
    } else {
      throw Error(`unknown fixture ${kind}`);
    }
    pcm[frame * 2] = clamp16(left);
    pcm[frame * 2 + 1] = clamp16(right);
  }
  return pcm;
}

function makeVideoFrame(encoder, t) {
  const frame = new Uint8Array(encoder.expectedFrameBytes);
  const yLen = width * height;
  const cLen = yLen / 4;
  for (let i = 0; i < yLen; i++) frame[i] = 40 + ((i + t * 13) % 170);
  frame.fill(118 + (t % 5), yLen, yLen + cLen);
  frame.fill(138 - (t % 5), yLen + cLen);
  return frame;
}

async function encodeFixture(kind, pcm) {
  const encoder = await createAveluneAvEncoder({
    width,
    height,
    fpsN,
    fpsD: 1,
    videoQ: 96,
    audioQ: 1,
    preset: 'fast',
    epochFrames: fpsN,
    audioRate,
    audioChannels,
  }, {artifact: 'scalar', scalarUrl});
  try {
    encoder.pushAudio(pcm);
    for (let t = 0; t < frameCount; t++) encoder.pushFrame(makeVideoFrame(encoder, t));
    return encoder.finish();
  } finally {
    encoder.destroy();
  }
}

function assertTimeline(packets, expected, label) {
  const timeline = new PcmTimeline(audioChannels);
  timeline.clear(0);
  for (const packet of packets) {
    const startFrame = Math.round(Number(packet.pts) * audioRate / 1_000_000);
    const floats = new Float32Array(packet.pcm.length);
    for (let i = 0; i < packet.pcm.length; i++) floats[i] = packet.pcm[i] / 32768;
    const pushed = timeline.push(startFrame, floats);
    if (pushed.lateFrames) throw Error(`${label}: ${pushed.lateFrames} worklet frames arrived late`);
  }

  const rendered = [new Float32Array(128), new Float32Array(128)];
  let offset = 0;
  while (offset < audioFrames) {
    const count = Math.min(128, audioFrames - offset);
    const result = timeline.render(rendered, offset, count);
    if (result.silentFrames) throw Error(`${label}: timeline inserted ${result.silentFrames} silent frames at ${offset}`);
    for (let frame = 0; frame < count; frame++) {
      for (let channel = 0; channel < audioChannels; channel++) {
        const actual = rendered[channel][frame];
        const expectedFloat = Math.fround(expected[(offset + frame) * audioChannels + channel] / 32768);
        if (actual !== expectedFloat) {
          throw Error(`${label}: timeline sample mismatch frame=${offset + frame} channel=${channel}: ${actual} != ${expectedFloat}`);
        }
      }
    }
    offset += count;
  }
  if (timeline.queuedFrames() !== 0) throw Error(`${label}: timeline retained ${timeline.queuedFrames()} frames after playback`);
}

async function decodeAndCheck(encoded, expected, artifact, kind) {
  const decoder = await createAveluneDecoder({artifact, scalarUrl, simdUrl});
  const packets = [];
  let sampleOffset = 0;
  try {
    const source = new BlobRangeSource(new Blob([encoded]), `${kind}-${artifact}.avl`);
    const index = await decoder.loadIndex(source);
    if (index.epochs.length !== seconds) throw Error(`${kind}/${artifact}: expected ${seconds} 1s epochs, got ${index.epochs.length}`);
    for (const epoch of index.epochs) {
      await decoder.decodeEpoch(source, epoch, {
        onAudio: packet => {
          if (packet.rate !== audioRate || packet.channels !== audioChannels) throw Error(`${kind}/${artifact}: decoded audio format changed`);
          for (let i = 0; i < packet.pcm.length; i++) {
            const expectedSample = expected[sampleOffset + i];
            if (packet.pcm[i] !== expectedSample) {
              throw Error(`${kind}/${artifact}: q1 sample mismatch at ${sampleOffset + i}: ${packet.pcm[i]} != ${expectedSample}`);
            }
          }
          sampleOffset += packet.pcm.length;
          packets.push({pts: packet.pts, rate: packet.rate, channels: packet.channels, pcm: packet.pcm.slice()});
        },
      });
    }
  } finally {
    decoder.destroy();
  }
  if (sampleOffset !== expected.length) throw Error(`${kind}/${artifact}: decoded ${sampleOffset}/${expected.length} samples`);
  assertTimeline(packets, expected, `${kind}/${artifact}`);
  return {artifact, packets: packets.length, frames: sampleOffset / audioChannels};
}

try {
  const results = [];
  for (const kind of ['tone', 'speech', 'music']) {
    const expected = makeFixture(kind);
    const encoded = await encodeFixture(kind, expected);
    const decoders = [];
    for (const artifact of ['scalar', 'simd128']) decoders.push(await decodeAndCheck(encoded, expected, artifact, kind));
    results.push({kind, bytes: encoded.length, decoders});
  }
  console.log(JSON.stringify({browserQ1Continuity: results}));
} finally {
  await new Promise(resolve => server.close(resolve));
}
