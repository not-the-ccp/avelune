import {CanvasRenderer, createRenderer} from '../web/player/renderers.js';

function makeYuv(w, h, value = 128) {
  const yLen = w * h;
  const cLen = yLen / 4;
  const yuv = new Uint8Array(yLen + cLen * 2);
  for (let i = 0; i < yLen; i++) yuv[i] = 16 + ((i * 13 + value) % 220);
  yuv.fill(110, yLen, yLen + cLen);
  yuv.fill(150, yLen + cLen);
  return yuv;
}

// Canvas is exercised in real Chromium too, but keep a tiny deterministic conversion smoke here so
// renderer refactors fail before the expensive site test.
{
  let puts = 0;
  const context = {
    createImageData: (w, h) => ({data: new Uint8ClampedArray(w * h * 4)}),
    putImageData: () => { puts++; },
  };
  const canvas = {
    width: 0,
    height: 0,
    getContext: kind => kind === '2d' ? context : null,
  };
  const renderer = new CanvasRenderer(canvas, 0);
  renderer.render({w: 4, h: 4, yuv: makeYuv(4, 4)});
  if (canvas.width !== 4 || canvas.height !== 4 || puts !== 1) throw Error('Canvas renderer smoke failed');
  renderer.destroy();
}

// GitHub's headless Chromium exposes no navigator.gpu on the current Linux runner. Exercise the
// complete WebGPU renderer control/resource path with a strict fake API, while keeping real hardware
// WebGPU validation an explicit environment-dependent check rather than claiming software coverage
// is hardware coverage.
const stats = {
  configured: 0,
  textureCreates: 0,
  textureDestroys: 0,
  writeBuffers: 0,
  writeTextures: 0,
  submits: 0,
  draws: 0,
  deviceDestroys: 0,
};

const fakeContext = {
  configure: options => {
    if (!options.device || options.format !== 'rgba8unorm' || options.alphaMode !== 'opaque') throw Error('bad WebGPU context configuration');
    stats.configured++;
  },
  getCurrentTexture: () => ({createView: () => ({kind: 'swap-view'})}),
};
const fakePipeline = {getBindGroupLayout: index => ({index})};
const fakeDevice = {
  queue: {
    writeBuffer: () => { stats.writeBuffers++; },
    writeTexture: (_target, data, layout, size) => {
      if (!(data instanceof Uint8Array) || !layout.bytesPerRow || !size.width || !size.height) throw Error('invalid WebGPU texture upload');
      stats.writeTextures++;
    },
    submit: commands => {
      if (!Array.isArray(commands) || commands.length !== 1) throw Error('invalid WebGPU submit');
      stats.submits++;
    },
  },
  createShaderModule: ({code}) => {
    if (!code.includes('@fragment') || !code.includes('textureSampleLevel')) throw Error('unexpected WebGPU shader');
    return {code};
  },
  createRenderPipeline: options => {
    if (options.primitive?.topology !== 'triangle-list') throw Error('unexpected WebGPU pipeline');
    return fakePipeline;
  },
  createSampler: () => ({kind: 'sampler'}),
  createBuffer: ({size}) => {
    if (size !== 32) throw Error(`unexpected WebGPU uniform size ${size}`);
    return {kind: 'uniform'};
  },
  createTexture: ({size, format}) => {
    if (format !== 'r8unorm' || !Array.isArray(size) || size.length !== 2) throw Error('unexpected WebGPU texture descriptor');
    stats.textureCreates++;
    let destroyed = false;
    return {
      createView: () => ({kind: 'plane-view'}),
      destroy: () => {
        if (!destroyed) { destroyed = true; stats.textureDestroys++; }
      },
    };
  },
  createBindGroup: ({entries}) => {
    if (entries.length !== 5) throw Error(`unexpected WebGPU bind group size ${entries.length}`);
    return {entries};
  },
  createCommandEncoder: () => ({
    beginRenderPass: ({colorAttachments}) => {
      if (colorAttachments.length !== 1) throw Error('unexpected WebGPU render pass');
      return {
        setPipeline: pipeline => { if (pipeline !== fakePipeline) throw Error('wrong WebGPU pipeline'); },
        setBindGroup: (index, group) => { if (index !== 0 || !group) throw Error('wrong WebGPU bind group'); },
        draw: count => { if (count !== 3) throw Error(`unexpected WebGPU draw count ${count}`); stats.draws++; },
        end: () => {},
      };
    },
    finish: () => ({kind: 'command-buffer'}),
  }),
  destroy: () => { stats.deviceDestroys++; },
};
const fakeAdapter = {requestDevice: async () => fakeDevice};
Object.defineProperty(globalThis, 'navigator', {
  configurable: true,
  value: {
    gpu: {
      requestAdapter: async () => fakeAdapter,
      getPreferredCanvasFormat: () => 'rgba8unorm',
    },
  },
});
Object.defineProperty(globalThis, 'GPUBufferUsage', {configurable: true, value: {UNIFORM: 1, COPY_DST: 2}});
Object.defineProperty(globalThis, 'GPUTextureUsage', {configurable: true, value: {TEXTURE_BINDING: 1, COPY_DST: 2}});

const canvas = {
  width: 0,
  height: 0,
  getContext: kind => kind === 'webgpu' ? fakeContext : null,
};
const selected = await createRenderer(canvas, 0, 'webgpu');
if (selected.name !== 'WebGPU') throw Error(`explicit WebGPU renderer selected ${selected.name}`);
selected.renderer.render({w: 4, h: 4, yuv: makeYuv(4, 4, 1)});
selected.renderer.render({w: 4, h: 4, yuv: makeYuv(4, 4, 2)});
if (stats.textureCreates !== 3) throw Error(`WebGPU textures were not reused (${stats.textureCreates} creates after same-size frames)`);
selected.renderer.render({w: 8, h: 4, yuv: makeYuv(8, 4, 3)});
if (stats.textureCreates !== 6 || stats.textureDestroys !== 3) {
  throw Error(`WebGPU resize resource lifecycle failed creates=${stats.textureCreates} destroys=${stats.textureDestroys}`);
}
selected.renderer.destroy();

if (stats.configured !== 1 || stats.writeBuffers !== 1 || stats.writeTextures !== 9 || stats.submits !== 3 || stats.draws !== 3) {
  throw Error(`WebGPU renderer call counts unexpected: ${JSON.stringify(stats)}`);
}
if (stats.textureDestroys !== 6 || stats.deviceDestroys !== 1) throw Error(`WebGPU renderer cleanup failed: ${JSON.stringify(stats)}`);

console.log(JSON.stringify({rendererSmoke: {canvas: 'passed', webgpuLogic: 'passed', hardwareWebGpu: 'not asserted', stats}}));
