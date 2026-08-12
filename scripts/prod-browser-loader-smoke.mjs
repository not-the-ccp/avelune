import http from 'node:http';
import fs from 'node:fs';
import {createAveluneProdDecoder} from '../web/player/avelune-prod-loader.js';

const files = {
  '/demo.avl': ['application/octet-stream', fs.readFileSync('web/player/demo.avl')],
  '/scalar.wasm': ['application/wasm', fs.readFileSync('web/player/avelune-prod-scalar.wasm')],
  '/simd.wasm': ['application/wasm', fs.readFileSync('web/player/avelune-prod-simd128.wasm')],
};
const requests = [];
const server = http.createServer((req, res) => {
  const found = files[req.url]; if (!found) { res.writeHead(404); return res.end(); }
  const [type, data] = found, match = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range || '');
  if (!match) {
    requests.push({url: req.url, full: true});
    res.writeHead(200, {'Content-Type': type, 'Content-Length': data.length, 'Accept-Ranges': 'bytes'});
    return res.end(data);
  }
  const first = Number(match[1]), last = Math.min(Number(match[2]), data.length - 1);
  requests.push({url: req.url, first, last});
  res.writeHead(206, {'Content-Type': type, 'Content-Length': last - first + 1, 'Content-Range': `bytes ${first}-${last}/${data.length}`, 'Accept-Ranges': 'bytes'});
  res.end(data.subarray(first, last + 1));
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;

async function run(forceScalar) {
  const decoder = await createAveluneProdDecoder({
    simdUrl: forceScalar ? `${base}/missing-simd.wasm` : `${base}/simd.wasm`,
    scalarUrl: `${base}/scalar.wasm`,
  });
  try {
    const index = await decoder.loadIndex(`${base}/demo.avl`);
    let video = 0, audio = 0;
    for (const epoch of index.epochs) {
      await decoder.decodeEpoch(`${base}/demo.avl`, epoch, {onVideo: () => video++, onAudio: () => audio++});
    }
    if (video !== 60 || audio !== 100) throw Error(`decoded ${video}/${audio}`);
    return {backend: decoder.backend, frontBytes: index.frontBytes, streams: index.streams.length, epochs: index.epochs.length, video, audio};
  } finally { decoder.destroy(); }
}

try {
  const simd = await run(false), scalar = await run(true);
  if (simd.backend !== 'simd128' || scalar.backend !== 'scalar') throw Error(`backend selection ${simd.backend}/${scalar.backend}`);
  const mediaRequests = requests.filter(x => x.url === '/demo.avl');
  if (mediaRequests.some(x => x.full)) throw Error('media was fetched without Range');
  console.log(JSON.stringify({simd, scalar, mediaRangeRequests: mediaRequests.length, ranges: mediaRequests}));
} finally {
  await new Promise(resolve => server.close(resolve));
}
