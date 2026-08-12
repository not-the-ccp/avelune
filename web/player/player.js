import {createAveluneProdDecoder} from './avelune-prod-loader.js';

const $ = id => document.getElementById(id);
function show(id, value) { const node = $(id); if (node) node.textContent = value; }
function streamFormat(stream) {
  const codec = stream.codec === 1 ? (stream.kind === 1 ? 'ALV1' : 'ALA1') : `codec ${stream.codec}`;
  return stream.kind === 1 ? `${codec} · ${stream.param0} × ${stream.param1}` : `${codec} · ${stream.param0} Hz · ${stream.param1} ch`;
}
let decoder, idx, currentUrl, renderer, ctxAudio, nextAudio = 0, playToken = 0, loadToken = 0;

function colorParams(meta0) {
  const matrix = meta0 & 15, full = !!(meta0 & (1 << 12));
  if (matrix === 1) return full ? [1, 0, 1.402, -.344136, -.714136, 1.772] : [1.164, 16 / 255, 1.596, -.392, -.813, 2.017];
  if (matrix === 3) return full ? [1, 0, 1.4746, -.16455, -.57135, 1.8814] : [1.164, 16 / 255, 1.6787, -.1873, -.6504, 2.1418];
  return full ? [1, 0, 1.5748, -.187324, -.468124, 1.8556] : [1.164, 16 / 255, 1.793, -.213, -.533, 2.112];
}

class CanvasRenderer {
  constructor(canvas, meta0) { this.c = canvas; this.x = canvas.getContext('2d'); this.k = colorParams(meta0); }
  render(f) {
    const {w, h, yuv} = f, [ys, yo, rv, gu, gv, bu] = this.k;
    this.c.width = w; this.c.height = h;
    const im = this.x.createImageData(w, h), Y = yuv, U = yuv.subarray(w * h, w * h + w * h / 4), V = yuv.subarray(w * h + w * h / 4);
    for (let j = 0, k = 0; j < h; j++) for (let i = 0; i < w; i++, k++) {
      const y = Y[k] / 255, u = U[(j >> 1) * (w >> 1) + (i >> 1)] / 255 - .5, v = V[(j >> 1) * (w >> 1) + (i >> 1)] / 255 - .5;
      const yy = ys * (y - yo), q = k * 4;
      im.data[q] = Math.max(0, Math.min(255, (yy + rv * v) * 255));
      im.data[q + 1] = Math.max(0, Math.min(255, (yy + gu * u + gv * v) * 255));
      im.data[q + 2] = Math.max(0, Math.min(255, (yy + bu * u) * 255));
      im.data[q + 3] = 255;
    }
    this.x.putImageData(im, 0, 0);
  }
}

class WebGPURenderer {
  static async make(canvas, meta0) {
    if (!navigator.gpu) return null;
    const adapter = await navigator.gpu.requestAdapter(); if (!adapter) return null;
    const device = await adapter.requestDevice(), context = canvas.getContext('webgpu'), format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({device, format, alphaMode: 'opaque'});
    const code = `@group(0) @binding(0) var ys:texture_2d<f32>;@group(0) @binding(1) var us:texture_2d<f32>;@group(0) @binding(2) var vs:texture_2d<f32>;@group(0) @binding(3) var sm:sampler;struct K{a:vec4f,b:vec4f};@group(0) @binding(4) var<uniform> k:K;struct O{@builtin(position)p:vec4f,@location(0)uv:vec2f};@vertex fn v(@builtin(vertex_index)i:u32)->O{var p=array<vec2f,3>(vec2f(-1.,-1.),vec2f(3.,-1.),vec2f(-1.,3.));var o:O;o.p=vec4f(p[i],0.,1.);o.uv=vec2f((p[i].x+1.)*.5,1.-(p[i].y+1.)*.5);return o}@fragment fn f(i:O)->@location(0) vec4f{let y=k.a.x*(textureSampleLevel(ys,sm,i.uv,0.).r-k.a.y);let u=textureSampleLevel(us,sm,i.uv,0.).r-.5;let vv=textureSampleLevel(vs,sm,i.uv,0.).r-.5;return vec4f(y+k.a.z*vv,y+k.a.w*u+k.b.x*vv,y+k.b.y*u,1.)}`;
    const mod = device.createShaderModule({code}), pipe = device.createRenderPipeline({layout: 'auto', vertex: {module: mod, entryPoint: 'v'}, fragment: {module: mod, entryPoint: 'f', targets: [{format}]}, primitive: {topology: 'triangle-list'}});
    const sampler = device.createSampler({magFilter: 'linear', minFilter: 'linear'}), values = colorParams(meta0), uniform = device.createBuffer({size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST});
    device.queue.writeBuffer(uniform, 0, new Float32Array([values[0], values[1], values[2], values[3], values[4], values[5], 0, 0]));
    return new WebGPURenderer(canvas, device, context, pipe, sampler, uniform);
  }
  constructor(c, d, x, p, s, u) { Object.assign(this, {c, d, x, p, s, u}); }
  tex(w, h, data) { const t = this.d.createTexture({size: [w, h], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST}); this.d.queue.writeTexture({texture: t}, data, {bytesPerRow: w, rowsPerImage: h}, {width: w, height: h}); return t; }
  render(f) {
    const {w, h, yuv} = f; this.c.width = w; this.c.height = h; const n = w * h, c = n / 4;
    const Y = this.tex(w, h, yuv.subarray(0, n)), U = this.tex(w / 2, h / 2, yuv.subarray(n, n + c)), V = this.tex(w / 2, h / 2, yuv.subarray(n + c));
    const bg = this.d.createBindGroup({layout: this.p.getBindGroupLayout(0), entries: [{binding: 0, resource: Y.createView()}, {binding: 1, resource: U.createView()}, {binding: 2, resource: V.createView()}, {binding: 3, resource: this.s}, {binding: 4, resource: {buffer: this.u}}]});
    const enc = this.d.createCommandEncoder(), pass = enc.beginRenderPass({colorAttachments: [{view: this.x.getCurrentTexture().createView(), loadOp: 'clear', storeOp: 'store', clearValue: {r: 0, g: 0, b: 0, a: 1}}]});
    pass.setPipeline(this.p); pass.setBindGroup(0, bg); pass.draw(3); pass.end(); this.d.queue.submit([enc.finish()]);
  }
}

async function chooseRenderer(meta0) {
  try { const r = await WebGPURenderer.make($('c'), meta0); if (r) { $('status').textContent += '\nrenderer: WebGPU (presentation only)'; show('format-renderer', 'WebGPU'); return r; } } catch (e) { console.warn(e); }
  $('status').textContent += '\nrenderer: Canvas2D fallback'; show('format-renderer', 'Canvas2D'); return new CanvasRenderer($('c'), meta0);
}

function scheduleAudio(a, when) {
  const frames = a.pcm.length / a.channels, buffer = ctxAudio.createBuffer(a.channels, frames, a.rate);
  for (let c = 0; c < a.channels; c++) { const out = buffer.getChannelData(c); for (let i = 0; i < frames; i++) out[i] = a.pcm[i * a.channels + c] / 32768; }
  const source = ctxAudio.createBufferSource(); source.buffer = buffer; source.connect(ctxAudio.destination); source.start(when); return frames / a.rate;
}

async function epoch(index, token, activeDecoder) {
  const e = idx.epochs[index], videos = [], audios = [];
  show('playback-epoch', `${index + 1} / ${idx.epochs.length}`);
  await activeDecoder.decodeEpoch(currentUrl, e, {
    onVideo: f => { show('format-dimensions', `${f.w} × ${f.h}`); videos.push(f); },
    onAudio: a => audios.push(a),
    onRange: (first, last) => show('playback-range', `${first}–${last}`),
  });
  if (token !== playToken) return;
  const audioBase = Math.max(nextAudio, ctxAudio.currentTime + .05);
  for (const a of audios) {
    const when = audioBase + Math.max(0, Number(a.pts - e.pts) / 1e6);
    nextAudio = Math.max(nextAudio, when + scheduleAudio(a, when));
  }
  const base = performance.now();
  for (const f of videos) {
    const wait = Math.max(0, Number(f.pts - e.pts) / 1000 - (performance.now() - base));
    if (wait) await new Promise(r => setTimeout(r, wait));
    if (token !== playToken) return;
    renderer.render(f); const current = (Number(f.pts) / 1e6).toFixed(2) + 's'; $('time').textContent = current; show('playback-time', current); $('seek').value = Number(f.pts) / 1e6;
  }
}

async function playFrom(sec) {
  const token = ++playToken;
  const activeDecoder = decoder;
  if (!activeDecoder || !idx) return;
  if (!ctxAudio) ctxAudio = new AudioContext(); await ctxAudio.resume(); nextAudio = ctxAudio.currentTime + .05;
  const ei = Math.max(0, idx.epochs.findLastIndex(e => Number(e.pts) / 1e6 <= sec));
  for (let i = ei; i < idx.epochs.length && token === playToken && activeDecoder === decoder; i++) {
    await epoch(i, token, activeDecoder);
  }
}

$('load').onclick = async () => {
  const token = ++loadToken;
  ++playToken;
  const previous = decoder;
  decoder = undefined;
  previous?.destroy();
  let candidate;
  try {
    show('state-label', 'LOADING');
    candidate = await createAveluneProdDecoder();
    if (token !== loadToken) { candidate.destroy(); return; }
    decoder = candidate; currentUrl = $('url').value; idx = await candidate.loadIndex(currentUrl);
    if (token !== loadToken) { candidate.destroy(); if (decoder === candidate) decoder = undefined; return; }
    $('status').textContent = `backend=${candidate.backend}\nstreams=${idx.streams.length} epochs=${idx.epochs.length}\nfront bytes=${idx.frontBytes}`;
    show('container-streams', idx.streams.length); show('container-epochs', idx.epochs.length); show('container-front', `${idx.frontBytes} B`);
    const video = idx.streams.find(s => s.kind === 1), audio = idx.streams.find(s => s.kind === 2);
    show('format-video', video ? streamFormat(video) : '—'); show('format-audio', audio ? streamFormat(audio) : '—');
    renderer = await chooseRenderer(video?.meta0 || 0);
    if (token !== loadToken) { candidate.destroy(); if (decoder === candidate) decoder = undefined; return; }
    $('status').textContent += `\nvideo meta0=0x${(video?.meta0 || 0).toString(16)}`;
    const end = Math.max(...idx.epochs.map(e => Number(e.pts) / 1e6 + Number(e.duration) / 1e6)); $('seek').max = end; $('seek').disabled = false; $('play').disabled = false; show('state-label', 'READY'); await playFrom(0);
  } catch (e) {
    candidate?.destroy();
    if (decoder === candidate) decoder = undefined;
    if (token === loadToken) { show('state-label', 'ERROR'); $('status').textContent = 'ERROR ' + (e.stack || e); }
  }
};
$('play').onclick = () => playFrom(+$('seek').value);
$('seek').onchange = () => playFrom(+$('seek').value);
window.addEventListener('pagehide', () => { ++loadToken; ++playToken; decoder?.destroy(); decoder = undefined; });
