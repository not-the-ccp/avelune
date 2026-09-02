import fs from 'node:fs';
import http from 'node:http';
import {
  BlobRangeSource,
  createAveluneAvEncoder,
  createAveluneDecoder,
} from '../web/player/avelune-loader.js';

const wasmPath = process.argv[2] ?? 'web/player/avelune-scalar.wasm';
const wasm = fs.readFileSync(wasmPath);
const server = http.createServer((request, response) => {
  if (request.url !== '/scalar.wasm') { response.writeHead(404).end(); return; }
  response.writeHead(200, {'Content-Type': 'application/wasm', 'Content-Length': wasm.length});
  response.end(wasm);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const scalarUrl = `http://127.0.0.1:${server.address().port}/scalar.wasm`;

const width = 32;
const height = 24;
const fpsN = 4;
const fpsD = 1;
const seconds = 9;
const frameCount = fpsN * seconds;
const audioRate = 48_000;
const audioChannels = 2;
const audioFrameCount = audioRate * seconds;
const epochCases = [
  {seconds: 1, frames: 4},
  {seconds: 2, frames: 8},
  {seconds: 4, frames: 16},
  {seconds: 8, frames: 32},
];

function makePcm() {
  const pcm = new Int16Array(audioFrameCount * audioChannels);
  for (let frame = 0; frame < audioFrameCount; frame++) {
    const left = Math.round(Math.sin(frame * 2 * Math.PI * 440 / audioRate) * 12_000);
    const right = Math.round(Math.sin(frame * 2 * Math.PI * 660 / audioRate) * 9_000);
    pcm[frame * 2] = left;
    pcm[frame * 2 + 1] = right;
  }
  return pcm;
}

function makeFrame(encoder, t) {
  const frame = new Uint8Array(encoder.expectedFrameBytes);
  const yLen = width * height;
  const cLen = yLen / 4;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      frame[y * width + x] = 32 + ((x * 5 + y * 3 + t * 11) % 190);
    }
  }
  frame.fill(96 + (t % 20), yLen, yLen + cLen);
  frame.fill(160 - (t % 20), yLen + cLen);
  return frame;
}

async function runCase(epoch) {
  const encoder = await createAveluneAvEncoder({
    width,
    height,
    fpsN,
    fpsD,
    videoQ: 96,
    audioQ: 1,
    preset: 'fast',
    epochFrames: epoch.frames,
    audioRate,
    audioChannels,
  }, {artifact: 'scalar', scalarUrl});

  let encoded;
  try {
    encoder.pushAudio(makePcm());
    for (let t = 0; t < frameCount; t++) encoder.pushFrame(makeFrame(encoder, t));
    encoded = encoder.finish();
  } finally {
    encoder.destroy();
  }

  const decoder = await createAveluneDecoder({artifact: 'scalar', scalarUrl});
  let video = 0;
  let audioPackets = 0;
  let decodedAudioFrames = 0;
  let index;
  try {
    const source = new BlobRangeSource(new Blob([encoded]), `epoch-${epoch.seconds}s.avl`);
    index = await decoder.loadIndex(source);
    for (const item of index.epochs) {
      await decoder.decodeEpoch(source, item, {
        onVideo: () => video++,
        onAudio: packet => {
          audioPackets++;
          decodedAudioFrames += packet.pcm.length / packet.channels;
        },
      });
    }
  } finally {
    decoder.destroy();
  }

  const expectedEpochs = Math.ceil(frameCount / epoch.frames);
  if (index.epochs.length !== expectedEpochs) {
    throw Error(`${epoch.seconds}s epoch produced ${index.epochs.length} index entries, expected ${expectedEpochs}`);
  }
  if (video !== frameCount) throw Error(`${epoch.seconds}s epoch decoded ${video}/${frameCount} video frames`);
  if (!audioPackets) throw Error(`${epoch.seconds}s epoch decoded no audio packets`);
  if (decodedAudioFrames !== audioFrameCount) {
    throw Error(`${epoch.seconds}s epoch decoded ${decodedAudioFrames}/${audioFrameCount} audio frames`);
  }

  // Epoch timestamps must advance monotonically and each non-tail epoch should cover the requested
  // wall-clock interval. This catches regressions where frame counts accidentally become seconds.
  for (let i = 1; i < index.epochs.length; i++) {
    if (Number(index.epochs[i].pts) <= Number(index.epochs[i - 1].pts)) {
      throw Error(`${epoch.seconds}s epoch index timestamps are not strictly increasing`);
    }
  }
  for (let i = 0; i + 1 < index.epochs.length; i++) {
    const duration = Number(index.epochs[i].duration) / 1e6;
    if (Math.abs(duration - epoch.seconds) > 1e-6) {
      throw Error(`${epoch.seconds}s epoch duration ${duration}s does not match requested interval`);
    }
  }

  return {
    seconds: epoch.seconds,
    epochFrames: epoch.frames,
    epochs: index.epochs.length,
    bytes: encoded.length,
    video,
    audioPackets,
    audioFrames: decodedAudioFrames,
  };
}

try {
  const results = [];
  for (const epoch of epochCases) results.push(await runCase(epoch));
  console.log(JSON.stringify({browserEpochMatrix: results}));
} finally {
  await new Promise(resolve => server.close(resolve));
}
