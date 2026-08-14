function colorParams(meta0) {
  const matrix = meta0 & 15, full = !!(meta0 & (1 << 12));
  if (matrix === 1) return full ? [1, 0, 1.402, -.344136, -.714136, 1.772] : [1.164, 16 / 255, 1.596, -.392, -.813, 2.017];
  if (matrix === 3) return full ? [1, 0, 1.4746, -.16455, -.57135, 1.8814] : [1.164, 16 / 255, 1.6787, -.1873, -.6504, 2.1418];
  return full ? [1, 0, 1.5748, -.187324, -.468124, 1.8556] : [1.164, 16 / 255, 1.793, -.213, -.533, 2.112];
}

export class CanvasRenderer {
  constructor(canvas, meta0) {
    this.canvas = canvas;
    this.context = canvas.getContext('2d', {alpha: false});
    if (!this.context) throw Error('Canvas2D context unavailable');
    this.coefficients = colorParams(meta0);
    this.image = null;
    this.width = 0;
    this.height = 0;
  }
  render(frame) {
    const {w, h, yuv} = frame, [ys, yo, rv, gu, gv, bu] = this.coefficients;
    if (this.width !== w || this.height !== h || !this.image) {
      this.width = w; this.height = h;
      this.canvas.width = w; this.canvas.height = h;
      this.image = this.context.createImageData(w, h);
    }
    const image = this.image;
    const yLen = w * h, cLen = yLen / 4;
    const yPlane = yuv.subarray(0, yLen), uPlane = yuv.subarray(yLen, yLen + cLen), vPlane = yuv.subarray(yLen + cLen);
    for (let row = 0, pixel = 0; row < h; row++) {
      for (let col = 0; col < w; col++, pixel++) {
        const y = yPlane[pixel] / 255;
        const u = uPlane[(row >> 1) * (w >> 1) + (col >> 1)] / 255 - .5;
        const v = vPlane[(row >> 1) * (w >> 1) + (col >> 1)] / 255 - .5;
        const luma = ys * (y - yo), out = pixel * 4;
        image.data[out] = Math.max(0, Math.min(255, (luma + rv * v) * 255));
        image.data[out + 1] = Math.max(0, Math.min(255, (luma + gu * u + gv * v) * 255));
        image.data[out + 2] = Math.max(0, Math.min(255, (luma + bu * u) * 255));
        image.data[out + 3] = 255;
      }
    }
    this.context.putImageData(image, 0, 0);
  }
  destroy() {}
}

export class WebGpuRenderer {
  static async create(canvas, meta0) {
    if (!navigator.gpu) return null;
    const adapter = await navigator.gpu.requestAdapter();
    if (!adapter) return null;
    const device = await adapter.requestDevice();
    const context = canvas.getContext('webgpu');
    if (!context) return null;
    const format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({device, format, alphaMode: 'opaque'});
    const module = device.createShaderModule({code: `
      @group(0) @binding(0) var y_tex: texture_2d<f32>;
      @group(0) @binding(1) var u_tex: texture_2d<f32>;
      @group(0) @binding(2) var v_tex: texture_2d<f32>;
      @group(0) @binding(3) var sample_linear: sampler;
      struct Coefficients { a: vec4f, b: vec4f };
      @group(0) @binding(4) var<uniform> k: Coefficients;
      struct Out { @builtin(position) position: vec4f, @location(0) uv: vec2f };
      @vertex fn vertex(@builtin(vertex_index) i: u32) -> Out {
        var p = array<vec2f, 3>(vec2f(-1., -1.), vec2f(3., -1.), vec2f(-1., 3.));
        var out: Out;
        out.position = vec4f(p[i], 0., 1.);
        out.uv = vec2f((p[i].x + 1.) * .5, 1. - (p[i].y + 1.) * .5);
        return out;
      }
      @fragment fn fragment(input: Out) -> @location(0) vec4f {
        let y = k.a.x * (textureSampleLevel(y_tex, sample_linear, input.uv, 0.).r - k.a.y);
        let u = textureSampleLevel(u_tex, sample_linear, input.uv, 0.).r - .5;
        let v = textureSampleLevel(v_tex, sample_linear, input.uv, 0.).r - .5;
        return vec4f(y + k.a.z * v, y + k.a.w * u + k.b.x * v, y + k.b.y * u, 1.);
      }
    `});
    const pipeline = device.createRenderPipeline({
      layout: 'auto',
      vertex: {module, entryPoint: 'vertex'},
      fragment: {module, entryPoint: 'fragment', targets: [{format}]},
      primitive: {topology: 'triangle-list'},
    });
    const sampler = device.createSampler({magFilter: 'nearest', minFilter: 'nearest'});
    const uniform = device.createBuffer({size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST});
    const values = colorParams(meta0);
    device.queue.writeBuffer(uniform, 0, new Float32Array([values[0], values[1], values[2], values[3], values[4], values[5], 0, 0]));
    return new WebGpuRenderer(canvas, device, context, pipeline, sampler, uniform);
  }

  constructor(canvas, device, context, pipeline, sampler, uniform) {
    Object.assign(this, {canvas, device, context, pipeline, sampler, uniform});
    this.width = 0;
    this.height = 0;
    this.textures = null;
    this.bindGroup = null;
  }

  #destroyTextures() {
    if (this.textures) for (const texture of this.textures) texture.destroy();
    this.textures = null;
    this.bindGroup = null;
  }

  #ensureTextures(width, height) {
    if (this.width === width && this.height === height && this.textures) return;
    this.#destroyTextures();
    this.width = width;
    this.height = height;
    this.canvas.width = width;
    this.canvas.height = height;
    const usage = GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST;
    const make = (w, h) => this.device.createTexture({size: [w, h], format: 'r8unorm', usage});
    this.textures = [make(width, height), make(width / 2, height / 2), make(width / 2, height / 2)];
    this.bindGroup = this.device.createBindGroup({
      layout: this.pipeline.getBindGroupLayout(0),
      entries: [
        {binding: 0, resource: this.textures[0].createView()},
        {binding: 1, resource: this.textures[1].createView()},
        {binding: 2, resource: this.textures[2].createView()},
        {binding: 3, resource: this.sampler},
        {binding: 4, resource: {buffer: this.uniform}},
      ],
    });
  }

  render(frame) {
    const {w, h, yuv} = frame;
    this.#ensureTextures(w, h);
    const yLen = w * h, uvLen = yLen / 4;
    this.device.queue.writeTexture({texture: this.textures[0]}, yuv.subarray(0, yLen), {bytesPerRow: w, rowsPerImage: h}, {width: w, height: h});
    this.device.queue.writeTexture({texture: this.textures[1]}, yuv.subarray(yLen, yLen + uvLen), {bytesPerRow: w / 2, rowsPerImage: h / 2}, {width: w / 2, height: h / 2});
    this.device.queue.writeTexture({texture: this.textures[2]}, yuv.subarray(yLen + uvLen), {bytesPerRow: w / 2, rowsPerImage: h / 2}, {width: w / 2, height: h / 2});
    const encoder = this.device.createCommandEncoder();
    const pass = encoder.beginRenderPass({colorAttachments: [{view: this.context.getCurrentTexture().createView(), loadOp: 'clear', storeOp: 'store', clearValue: {r: 0, g: 0, b: 0, a: 1}}]});
    pass.setPipeline(this.pipeline);
    pass.setBindGroup(0, this.bindGroup);
    pass.draw(3);
    pass.end();
    this.device.queue.submit([encoder.finish()]);
  }

  destroy() { this.#destroyTextures(); this.device.destroy?.(); }
}

export async function createRenderer(canvas, meta0, preference = 'auto') {
  if (preference !== 'canvas') {
    try {
      const gpu = await WebGpuRenderer.create(canvas, meta0);
      if (gpu) return {renderer: gpu, name: 'WebGPU'};
      if (preference === 'webgpu') throw Error('WebGPU is unavailable');
    } catch (error) {
      if (preference === 'webgpu') throw error;
      console.warn('WebGPU presentation unavailable; using Canvas2D', error);
    }
  }
  return {renderer: new CanvasRenderer(canvas, meta0), name: 'Canvas2D'};
}
