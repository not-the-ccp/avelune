import fs from 'node:fs';
import http from 'node:http';
import {
  AveluneDecoder,
  BlobRangeSource,
  HttpRangeSource,
  MemoryRangeSource,
  StaleDecodeGenerationError,
  createAveluneDecoder,
  createAveluneVideoEncoder,
} from '../web/player/avelune-loader.js';

const media = new Uint8Array(fs.readFileSync(process.argv[2] ?? 'web/player/demo.avl'));
const scalar = fs.readFileSync(process.argv[3] ?? 'web/player/avelune-scalar.wasm');
const simd = fs.readFileSync(process.argv[4] ?? 'web/player/avelune-simd128.wasm');
const requests = [];
const files = new Map([
  ['/demo.avl', ['application/octet-stream', Buffer.from(media)]],
  ['/scalar.wasm', ['application/wasm', scalar]],
  ['/simd.wasm', ['application/wasm', simd]],
]);
const server = http.createServer((req, res) => {
  const entry = files.get(req.url);
  if (!entry) { res.writeHead(404); res.end(); return; }
  const [type, data] = entry;
  const match = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range ?? '');
  if (!match) {
    requests.push({url: req.url, full: true});
    res.writeHead(200, {'Content-Type': type, 'Content-Length': data.length, 'Accept-Ranges': 'bytes'});
    res.end(data); return;
  }
  const first = Number(match[1]), last = Number(match[2]);
  if (first < 0 || last < first || last >= data.length) { res.writeHead(416); res.end(); return; }
  requests.push({url: req.url, first, last});
  res.writeHead(206, {
    'Content-Type': type,
    'Content-Length': last - first + 1,
    'Content-Range': `bytes ${first}-${last}/${data.length}`,
    'Accept-Ranges': 'bytes',
  });
  res.end(data.subarray(first, last + 1));
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;

async function decodeAll(decoder, source) {
  const index = await decoder.loadIndex(source);
  let video = 0, audio = 0;
  for (const epoch of index.epochs) {
    await decoder.decodeEpoch(source, epoch, {onVideo: () => video++, onAudio: () => audio++});
  }
  return {index, video, audio};
}

async function expectReject(promise, pattern) {
  try { await promise; } catch (error) {
    if (!pattern.test(String(error?.message ?? error))) throw error;
    return error;
  }
  throw Error(`expected rejection matching ${pattern}`);
}

try {
  const simdDecoder = await createAveluneDecoder({artifact: 'simd128', simdUrl: `${base}/simd.wasm`});
  const scalarDecoder = await createAveluneDecoder({artifact: 'scalar', scalarUrl: `${base}/scalar.wasm`});
  let httpResult, blobResult;
  try {
    httpResult = await decodeAll(simdDecoder, new HttpRangeSource(`${base}/demo.avl`));
    blobResult = await decodeAll(scalarDecoder, new BlobRangeSource(new Blob([media]), 'demo.avl'));
  } finally { simdDecoder.destroy(); scalarDecoder.destroy(); }
  if (httpResult.video !== blobResult.video || httpResult.audio !== blobResult.audio || !httpResult.video) {
    throw Error(`HTTP/Blob mismatch ${httpResult.video}/${httpResult.audio} vs ${blobResult.video}/${blobResult.audio}`);
  }
  if (requests.some(r => r.url === '/demo.avl' && r.full)) throw Error('media fetched without HTTP Range');

  await expectReject(
    createAveluneVideoEncoder({
      width: 31, height: 24, fpsN: 30, fpsD: 1, qstep: 96, preset: 'balanced', epochFrames: 3, meta0: 2 << 13,
    }, {artifact: 'scalar', scalarUrl: `${base}/scalar.wasm`}),
    /dimensions/,
  );

  // Exercise the browser-facing encoder wrapper, not only the raw WASM exports.
  const browserEncoder = await createAveluneVideoEncoder({
    width: 32, height: 24, fpsN: 30, fpsD: 1, qstep: 96, preset: 'balanced', epochFrames: 3, meta0: 2 << 13,
  }, {artifact: 'scalar', scalarUrl: `${base}/scalar.wasm`});
  let browserEncoded;
  try {
    for (let t = 0; t < 5; t++) {
      const frame = new Uint8Array(browserEncoder.expectedFrameBytes), yLen = 32 * 24, cLen = yLen / 4;
      for (let y = 0; y < 24; y++) for (let x = 0; x < 32; x++) frame[y * 32 + x] = (x * 7 + y * 5 + t * 13) & 255;
      frame.fill(100 + t, yLen, yLen + cLen); frame.fill(150 - t, yLen + cLen);
      browserEncoder.pushFrame(frame);
    }
    browserEncoded = browserEncoder.finish();
  } finally { browserEncoder.destroy(); }
  const encoderRoundtripDecoder = await createAveluneDecoder({artifact: 'scalar', scalarUrl: `${base}/scalar.wasm`});
  let encoderRoundtrip;
  try { encoderRoundtrip = await decodeAll(encoderRoundtripDecoder, new BlobRangeSource(new Blob([browserEncoded]), 'encoded.avl')); }
  finally { encoderRoundtripDecoder.destroy(); }
  if (encoderRoundtrip.video !== 5 || encoderRoundtrip.audio !== 0) throw Error('browser encoder wrapper roundtrip failed');

  // Arbitrarily fragmented complete ranges must decode identically to contiguous reads.
  class FragmentedSource {
    async *streamRange(first, length) {
      const start = Number(first), total = Number(length);
      const steps = [7, 1, 63, 4093, 2, 61, 8191, 1];
      let cursor = 0, ci = 0;
      while (cursor < total) {
        const n = Math.min(steps[ci % steps.length], total - cursor);
        yield media.slice(start + cursor, start + cursor + n);
        cursor += n; ci += 1;
      }
    }
    async readRange(first, length, options = {}) {
      const out = new Uint8Array(Number(length));
      let offset = 0;
      for await (const chunk of this.streamRange(first, length, options)) {
        out.set(chunk, offset);
        offset += chunk.length;
      }
      return out;
    }
  }
  const fragmentedDecoder = await createAveluneDecoder({artifact: 'simd128', simdUrl: `${base}/simd.wasm`});
  let fragmented;
  try { fragmented = await decodeAll(fragmentedDecoder, new FragmentedSource()); }
  finally { fragmentedDecoder.destroy(); }
  if (fragmented.video !== httpResult.video || fragmented.audio !== httpResult.audio) {
    throw Error(`fragmented range mismatch ${fragmented.video}/${fragmented.audio} vs ${httpResult.video}/${httpResult.audio}`);
  }

  // HTTP contract checks are independent of codec parsing.
  const wrongRange = new HttpRangeSource('memory://bad', {fetchImpl: async () => new Response(new Uint8Array(8), {
    status: 206,
    headers: {'Content-Range': 'bytes 1-8/100', 'Content-Length': '8'},
  })});
  await expectReject(wrongRange.readRange(0n, 8n), /Content-Range mismatch/);
  const shortRange = new HttpRangeSource('memory://short', {fetchImpl: async () => new Response(new Uint8Array(7), {
    status: 206,
    headers: {'Content-Range': 'bytes 0-7/100'},
  })});
  await expectReject(shortRange.readRange(0n, 8n), /short HTTP Range response/);
  const longRange = new HttpRangeSource('memory://long', {fetchImpl: async () => new Response(new Uint8Array(9), {
    status: 206,
    headers: {'Content-Range': 'bytes 0-7/100'},
  })});
  await expectReject(longRange.readRange(0n, 8n), /more than 8 bytes/);

  // Exact transport length is not enough: Rust must reject an indexed range truncated inside framing.
  // The adversarial seek-generation cases run against both WASM artifacts as the issue requires.
  const adversarialArtifacts = [];
  for (const artifact of ['scalar', 'simd128']) {
    const decoder = await createAveluneDecoder({artifact, scalarUrl: `${base}/scalar.wasm`, simdUrl: `${base}/simd.wasm`});
    adversarialArtifacts.push(artifact);
    try {
      const source = new MemoryRangeSource(media, 'memory');
      const index = await decoder.loadIndex(source);
      const epoch = index.epochs[0];
      class TruncatedSource {
        async *streamRange(first, length) {
          const start = Number(first), n = Number(length) - 1;
          yield media.slice(start, start + n);
        }
      }
      await expectReject(decoder.decodeEpoch(new TruncatedSource(), epoch), /TrailingData|UnexpectedEof|trailing|eof/i);
      await expectReject(decoder.decodeEpoch(source, {...epoch, id: Number(epoch.id) + 1000}), /BadEpoch/);

      // A stale generation is rejected at the JS/WASM boundary even if its source ignores cancellation.
      const a = index.epochs[0], b = index.epochs[Math.min(1, index.epochs.length - 1)];
      let markStalled, release;
      const stalled = new Promise(resolve => { markStalled = resolve; });
      const released = new Promise(resolve => { release = resolve; });
      class StubbornSource {
        async *streamRange(first, length) {
          const start = Number(first), n = Number(length), cut = Math.min(64, n);
          yield media.slice(start, start + cut);
          markStalled();
          await released;
          if (cut < n) yield media.slice(start + cut, start + n);
        }
      }
      const oldDecode = decoder.decodeEpoch(new StubbornSource(), a);
      await stalled;
      let newVideo = 0;
      const newDecode = decoder.decodeEpoch(source, b, {onVideo: () => newVideo++});
      release();
      const stale = await expectReject(oldDecode, /stale decode generation|aborted/i);
      await newDecode;
      if (!(stale instanceof StaleDecodeGenerationError) && stale?.name !== 'AbortError') throw stale;
      if (!newVideo && httpResult.video) throw Error('new generation produced no video');
    } finally { decoder.destroy(); }
  }

  console.log(JSON.stringify({
    http: {video: httpResult.video, audio: httpResult.audio, epochs: httpResult.index.epochs.length},
    blob: {video: blobResult.video, audio: blobResult.audio},
    encoder: {bytes: browserEncoded.length, video: encoderRoundtrip.video},
    fragmented: {video: fragmented.video, audio: fragmented.audio, steps: ['7','1','63','4093','2','61','8191','1']},
    mediaRangeRequests: requests.filter(r => r.url === '/demo.avl').length,
    adversarial: ['content-range', 'short-range', 'long-range', 'truncated-epoch', 'wrong-epoch', 'stale-generation'],
    adversarialArtifacts,
  }));
} finally {
  await new Promise(resolve => server.close(resolve));
}
