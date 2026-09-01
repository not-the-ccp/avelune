import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';

const root = path.resolve(process.cwd());
const chromium = [
  process.env.CHROMIUM,
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
].find(candidate => candidate && fs.existsSync(candidate));

if (!chromium) {
  console.log('SKIP: Chromium is unavailable for browser media-import smoke');
  process.exit(0);
}

const coreDirCandidates = [
  path.join(root, 'web/player/ffmpeg'),
  path.join(root, 'node_modules/@ffmpeg/core/dist/esm'),
];
const coreDir = coreDirCandidates.find(candidate =>
  fs.existsSync(path.join(candidate, 'ffmpeg-core.js')) &&
  fs.existsSync(path.join(candidate, 'ffmpeg-core.wasm')),
);
if (!coreDir) {
  console.log('SKIP: embedded FFmpeg core is unavailable; run npm ci or stage web/player/ffmpeg');
  process.exit(0);
}

const fixture = path.join(root, 'tests/browser/import-smoke.mp4');
for (const required of [
  fixture,
  path.join(root, 'web/player/media-import-worker.js'),
  path.join(root, 'web/player/avelune-loader.js'),
  path.join(root, 'web/player/y4m.js'),
  path.join(root, 'web/player/avelune-scalar.wasm'),
  path.join(root, 'web/player/avelune-simd128.wasm'),
]) {
  if (!fs.existsSync(required)) throw Error(`missing browser media-import smoke dependency: ${required}`);
}

const mime = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.wasm', 'application/wasm'],
  ['.mp4', 'video/mp4'],
]);

const routes = new Map([
  ['/media-import-worker.js', path.join(root, 'web/player/media-import-worker.js')],
  ['/avelune-loader.js', path.join(root, 'web/player/avelune-loader.js')],
  ['/y4m.js', path.join(root, 'web/player/y4m.js')],
  ['/avelune-scalar.wasm', path.join(root, 'web/player/avelune-scalar.wasm')],
  ['/avelune-simd128.wasm', path.join(root, 'web/player/avelune-simd128.wasm')],
  ['/ffmpeg/ffmpeg-core.js', path.join(coreDir, 'ffmpeg-core.js')],
  ['/ffmpeg/ffmpeg-core.wasm', path.join(coreDir, 'ffmpeg-core.wasm')],
  ['/input.mp4', fixture],
]);

const server = http.createServer((req, res) => {
  const pathname = new URL(req.url ?? '/', 'http://localhost').pathname;
  if (pathname === '/smoke.html') {
    const body = '<!doctype html><meta charset="utf-8"><title>Avelune media import smoke</title>';
    res.writeHead(200, {'Content-Type': 'text/html; charset=utf-8', 'Content-Length': Buffer.byteLength(body)});
    res.end(body);
    return;
  }
  const file = routes.get(pathname);
  if (!file) { res.writeHead(404); res.end('not found'); return; }
  const data = fs.readFileSync(file);
  res.writeHead(200, {
    'Content-Type': mime.get(path.extname(file)) ?? 'application/octet-stream',
    'Content-Length': data.length,
    'Cache-Control': 'no-store',
  });
  res.end(data);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'avelune-media-import-chrome-'));
const groupedProcess = process.platform !== 'win32';
const child = spawn(chromium, [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage', '--no-proxy-server',
  '--no-first-run', '--no-default-browser-check', '--disable-extensions',
  '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank',
], {stdio: ['ignore', 'ignore', 'pipe'], detached: groupedProcess});
let stderr = '';
child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-8192); });

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const startupAttempts = 600;
async function devtoolsPort() {
  const portFile = path.join(profile, 'DevToolsActivePort');
  for (let attempt = 0; attempt < startupAttempts; attempt++) {
    if (child.exitCode !== null || child.signalCode !== null) break;
    try {
      const port = Number(fs.readFileSync(portFile, 'utf8').split('\n', 1)[0]);
      if (Number.isInteger(port) && port > 0) return port;
    } catch {}
    await sleep(50);
  }
  throw Error(`Chromium did not publish a DevTools port: ${stderr.trim() || 'no stderr'}`);
}
async function pageTarget() {
  const port = await devtoolsPort();
  for (let attempt = 0; attempt < startupAttempts; attempt++) {
    try {
      const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = pages.find(item => item.type === 'page');
      if (page?.webSocketDebuggerUrl) return page;
    } catch {}
    await sleep(50);
  }
  throw Error(`Chromium DevTools target unavailable: ${stderr.trim() || 'no stderr'}`);
}

let ws;
const pending = new Map();
let sequence = 0;
async function connect() {
  ws = new WebSocket((await pageTarget()).webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, {once: true});
    ws.addEventListener('error', reject, {once: true});
  });
  ws.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    const resolver = pending.get(message.id);
    if (resolver) { pending.delete(message.id); resolver(message); }
  });
}
const call = (method, params = {}) => new Promise(resolve => {
  const id = ++sequence;
  pending.set(id, resolve);
  ws.send(JSON.stringify({id, method, params}));
});
async function evaluate(expression) {
  const reply = await call('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true});
  if (reply.result?.exceptionDetails) {
    throw Error(reply.result.exceptionDetails.exception?.description ?? reply.result.exceptionDetails.text);
  }
  return reply.result?.result?.value;
}
async function navigate(url) {
  await call('Page.navigate', {url});
  for (let attempt = 0; attempt < 300; attempt++) {
    if (await evaluate('document.readyState') === 'complete') return;
    await sleep(50);
  }
  throw Error(`navigation timeout: ${url}`);
}

const browserProgram = `(async()=>{
  const inputBlob = await (await fetch('/input.mp4')).blob();
  const input = new File([inputBlob], 'import-smoke.mp4', {type: 'video/mp4'});
  const worker = new Worker('/media-import-worker.js', {type: 'module'});
  const result = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(Error('media import worker timeout')), 90_000);
    worker.addEventListener('message', event => {
      const data = event.data || {};
      if (data.requestId !== 1) return;
      if (data.type === 'done') { clearTimeout(timer); resolve(data); }
      if (data.type === 'error') { clearTimeout(timer); reject(Error(data.message)); }
    });
    worker.addEventListener('error', event => { clearTimeout(timer); reject(Error(event.message || 'media import worker failed')); });
    worker.postMessage({
      type: 'convert', requestId: 1, file: input, name: 'import-smoke.mp4',
      options: {
        videoQ: 112, audioQ: 64, audioChannels: 2, audioRate: 32_000,
        epochSeconds: 1, preset: 'fast', resolution: '96x54', fps: '8', artifact: 'simd128',
      },
    });
  });
  worker.terminate();

  const {createAveluneDecoder, BlobRangeSource} = await import('/avelune-loader.js');
  if (!(result.file instanceof Blob)) throw Error('streaming importer did not return a file/blob');
  if (result.spool !== 'opfs') throw Error('Chromium streaming smoke did not use OPFS output spooling');
  const encoded = new Uint8Array(await result.file.arrayBuffer());
  const decoder = await createAveluneDecoder({artifact: 'simd128'});
  let video = 0, audio = 0;
  try {
    const source = new BlobRangeSource(new Blob([encoded]), 'converted.avl');
    const index = await decoder.loadIndex(source);
    for (const epoch of index.epochs) {
      await decoder.decodeEpoch(source, epoch, {onVideo: () => video++, onAudio: () => audio++});
    }
    if (video !== result.frames || video < 4) throw Error('converted video frame count mismatch');
    if (audio <= 0 || result.audioSamples <= 0) throw Error('converted audio was lost');
    const expectedRawVideoBytes = result.frames * result.width * result.height * 3 / 2;
    if (result.rawVideoBytes !== expectedRawVideoBytes) throw Error('raw video streaming byte count mismatch: ' + result.rawVideoBytes + ' != ' + expectedRawVideoBytes);
    if (result.rawAudioBytes !== result.audioSamples * 2) throw Error('raw PCM streaming byte count mismatch');
    if (result.artifact !== 'simd128') throw Error('requested SIMD128 encoder was not used');
    return {
      artifact: result.artifact,
      encodedBytes: encoded.length,
      width: result.width,
      height: result.height,
      frames: result.frames,
      decodedVideo: video,
      decodedAudioPackets: audio,
      audioSamples: result.audioSamples,
      streams: index.streams.map(stream => stream.kind),
      spool: result.spool,
      sourceBytes: result.sourceBytes,
      rawVideoBytes: result.rawVideoBytes,
      rawAudioBytes: result.rawAudioBytes,
    };
  } finally {
    decoder.destroy();
  }
})()`;

try {
  await connect();
  await call('Page.enable');
  await call('Runtime.enable');
  await navigate(`${base}/smoke.html`);
  const result = await evaluate(browserProgram);
  console.log(JSON.stringify({chromium: true, ffmpegCore: path.relative(root, coreDir), result}));
} finally {
  ws?.close();
  const exited = new Promise(resolve => child.once('exit', resolve));
  try {
    if (groupedProcess) process.kill(-child.pid, 'SIGTERM');
    else child.kill('SIGTERM');
  } catch {}
  await Promise.race([exited, sleep(2000)]);
  await new Promise(resolve => server.close(resolve));
  fs.rmSync(profile, {recursive: true, force: true, maxRetries: 2, retryDelay: 100});
}
