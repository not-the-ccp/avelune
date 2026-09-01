// Avelune browser binding. Rust/WASM owns parsing and codec semantics; JavaScript owns byte-range
// transport, cancellation generations, and presentation scheduling.
const DEFAULT_SCALAR = 'avelune-scalar.wasm';
const DEFAULT_SIMD = 'avelune-simd128.wasm';
const DEFAULT_CHUNK = 64 * 1024;

const asBigInt = value => typeof value === 'bigint' ? value : BigInt(value);
const asSafeNumber = (value, label) => {
  const n = Number(value);
  if (!Number.isSafeInteger(n) || n < 0) throw Error(`${label} is outside JavaScript's safe integer range`);
  return n;
};

function abortError() {
  return new DOMException('The operation was aborted', 'AbortError');
}

function throwIfAborted(signal) {
  if (signal?.aborted) throw signal.reason ?? abortError();
}

function combinedSignal(...signals) {
  const live = signals.filter(Boolean);
  if (!live.length) return undefined;
  if (typeof AbortSignal?.any === 'function') return AbortSignal.any(live);
  const controller = new AbortController();
  const abort = signal => controller.abort(signal.reason ?? abortError());
  for (const signal of live) {
    if (signal.aborted) { abort(signal); break; }
    signal.addEventListener('abort', () => abort(signal), {once: true});
  }
  return controller.signal;
}

function parseContentRange(value) {
  const match = /^bytes (\d+)-(\d+)\/(\d+|\*)$/.exec(value ?? '');
  if (!match) throw Error(`invalid Content-Range: ${value ?? '<missing>'}`);
  const first = BigInt(match[1]), last = BigInt(match[2]);
  if (last < first) throw Error(`invalid Content-Range order: ${value}`);
  return {first, last, total: match[3] === '*' ? null : BigInt(match[3])};
}

export class HttpRangeSource {
  constructor(url, {fetchImpl = globalThis.fetch.bind(globalThis)} = {}) {
    this.url = url;
    this.fetchImpl = fetchImpl;
    this.label = url;
    this.size = null;
  }

  async *streamRange(firstValue, lengthValue, {signal, onRange, chunkSize = DEFAULT_CHUNK} = {}) {
    const first = asBigInt(firstValue), length = asBigInt(lengthValue);
    const maxChunk = Math.max(1, Number(chunkSize) || DEFAULT_CHUNK);
    if (length <= 0n) throw Error('range length must be positive');
    const last = first + length - 1n;
    onRange?.({source: 'http', first, last, length, url: this.url});
    throwIfAborted(signal);
    const response = await this.fetchImpl(this.url, {
      headers: {Range: `bytes=${first}-${last}`},
      signal,
      cache: 'no-store',
    });
    if (response.status !== 206) throw Error(`HTTP Range required; got ${response.status} for ${first}-${last}`);
    const range = parseContentRange(response.headers.get('content-range'));
    if (range.first !== first || range.last !== last) {
      throw Error(`Content-Range mismatch: requested ${first}-${last}, received ${range.first}-${range.last}`);
    }
    if (range.total !== null) {
      if (range.total <= last) throw Error(`Content-Range total ${range.total} does not contain byte ${last}`);
      if (this.size !== null && this.size !== range.total) throw Error(`source size changed from ${this.size} to ${range.total}`);
      this.size = range.total;
    }
    const contentLength = response.headers.get('content-length');
    if (contentLength !== null && BigInt(contentLength) !== length) {
      throw Error(`Content-Length mismatch: expected ${length}, received ${contentLength}`);
    }

    let received = 0n;
    const yieldChecked = async function* (chunk) {
      if (!chunk?.length) return;
      const bytes = chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
      for (let offset = 0; offset < bytes.length; offset += maxChunk) {
        const part = bytes.subarray(offset, Math.min(bytes.length, offset + maxChunk));
        const next = received + BigInt(part.length);
        if (next > length) throw Error(`HTTP Range returned more than ${length} bytes`);
        received = next;
        yield part;
      }
    };
    if (response.body?.getReader) {
      const reader = response.body.getReader();
      try {
        while (true) {
          throwIfAborted(signal);
          const {done, value} = await reader.read();
          if (done) break;
          for await (const chunk of yieldChecked(value)) yield chunk;
        }
      } finally {
        reader.releaseLock?.();
      }
    } else {
      const bytes = new Uint8Array(await response.arrayBuffer());
      for await (const chunk of yieldChecked(bytes)) yield chunk;
    }
    if (received !== length) throw Error(`short HTTP Range response: expected ${length} bytes, received ${received}`);
  }

  async readRange(first, length, options = {}) {
    const expected = asSafeNumber(length, 'range length');
    const out = new Uint8Array(expected);
    let offset = 0;
    for await (const chunk of this.streamRange(first, length, options)) {
      out.set(chunk, offset);
      offset += chunk.length;
    }
    return out;
  }
}

export class BlobRangeSource {
  constructor(blob, name = 'local file') {
    if (!(blob instanceof Blob)) throw TypeError('BlobRangeSource requires a Blob or File');
    this.blob = blob;
    this.label = name;
    this.size = BigInt(blob.size);
  }

  async *streamRange(firstValue, lengthValue, {signal, onRange, chunkSize = DEFAULT_CHUNK} = {}) {
    const first = asBigInt(firstValue), length = asBigInt(lengthValue);
    if (length <= 0n) throw Error('range length must be positive');
    const lastExclusive = first + length;
    if (first < 0n || lastExclusive > this.size) throw Error(`local range ${first}-${lastExclusive - 1n} exceeds ${this.size} bytes`);
    const chunk = BigInt(Math.max(1, chunkSize));
    onRange?.({source: 'blob', first, last: lastExclusive - 1n, length, name: this.label});
    for (let cursor = first; cursor < lastExclusive; cursor += chunk) {
      throwIfAborted(signal);
      const end = cursor + chunk < lastExclusive ? cursor + chunk : lastExclusive;
      const part = this.blob.slice(asSafeNumber(cursor, 'blob range start'), asSafeNumber(end, 'blob range end'));
      const bytes = new Uint8Array(await part.arrayBuffer());
      throwIfAborted(signal);
      if (BigInt(bytes.length) !== end - cursor) throw Error('Blob.slice returned an unexpected byte count');
      yield bytes;
    }
  }

  async readRange(first, length, options = {}) {
    const expected = asSafeNumber(length, 'range length');
    const out = new Uint8Array(expected);
    let offset = 0;
    for await (const chunk of this.streamRange(first, length, options)) {
      out.set(chunk, offset);
      offset += chunk.length;
    }
    return out;
  }
}

export class MemoryRangeSource extends BlobRangeSource {
  constructor(bytes, name = 'memory') {
    super(new Blob([bytes]), name);
  }
}

async function fetchBytes(url, signal) {
  const response = await fetch(url, {signal, cache: 'no-store'});
  if (!response.ok) throw Error(`${url}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}

const wasmModuleCache = new Map();

async function compileArtifact(url, signal) {
  const key = String(url);
  let pending = wasmModuleCache.get(key);
  if (!pending) {
    pending = (async () => {
      const bytes = await fetchBytes(url, signal);
      if (!WebAssembly.validate(bytes)) throw Error(`${url}: WASM validation failed`);
      return WebAssembly.compile(bytes);
    })();
    wasmModuleCache.set(key, pending);
    try { await pending; }
    catch (error) { if (wasmModuleCache.get(key) === pending) wasmModuleCache.delete(key); throw error; }
  }
  throwIfAborted(signal);
  return pending;
}

async function instantiateArtifact(url, signal) {
  const module = await compileArtifact(url, signal);
  throwIfAborted(signal);
  const instance = await WebAssembly.instantiate(module, {});
  const abi = instance.exports.avelune_abi_version?.();
  if (abi !== 0x0002_0000) throw Error(`unsupported Avelune WASM ABI: ${abi ?? '<missing>'}`);
  return instance;
}

function decoderError(ex, handle) {
  const ptr = ex.decoder_last_error_ptr(handle), len = ex.decoder_last_error_len(handle);
  return len ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len)) : 'unknown decoder error';
}


function encoderError(ex, handle) {
  const ptr = ex.video_encoder_last_error_ptr(handle), len = ex.video_encoder_last_error_len(handle);
  return len ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len)) : 'unknown encoder error';
}

function encoderCreateError(ex) {
  const len = ex.video_encoder_create_error_len();
  if (!len) return 'video encoder configuration was rejected';
  const ptr = ex.video_encoder_create_error_ptr();
  return new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len));
}

function avEncoderError(ex, handle) {
  const ptr = ex.av_encoder_last_error_ptr(handle), len = ex.av_encoder_last_error_len(handle);
  return len ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len)) : 'unknown A/V encoder error';
}

function avEncoderCreateError(ex) {
  const len = ex.av_encoder_create_error_len();
  if (!len) return 'A/V encoder configuration was rejected';
  const ptr = ex.av_encoder_create_error_ptr();
  return new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len));
}

function streamAvEncoderError(ex, handle) {
  const ptr = ex.av_stream_encoder_last_error_ptr(handle), len = ex.av_stream_encoder_last_error_len(handle);
  return len ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len)) : 'unknown streaming A/V encoder error';
}

function streamAvEncoderCreateError(ex) {
  const len = ex.av_stream_encoder_create_error_len();
  if (!len) return 'streaming A/V encoder configuration was rejected';
  const ptr = ex.av_stream_encoder_create_error_ptr();
  return new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len));
}

export class StaleDecodeGenerationError extends Error {
  constructor() { super('stale decode generation'); this.name = 'StaleDecodeGenerationError'; }
}

export class AveluneDecoder {
  constructor(instance, artifact) {
    this.instance = instance;
    this.ex = instance.exports;
    this.artifact = artifact;
    this.handle = this.ex.decoder_create();
    this.generation = 0;
    this.activeAbort = null;
    if (!this.handle) throw Error('decoder_create failed');
  }

  destroy() {
    this.cancel();
    if (this.handle) {
      this.ex.decoder_destroy(this.handle);
      this.handle = 0;
    }
  }

  cancel() {
    this.generation++;
    this.activeAbort?.abort(abortError());
    this.activeAbort = null;
  }

  #startGeneration(externalSignal) {
    this.cancel();
    const generation = this.generation;
    const controller = new AbortController();
    this.activeAbort = controller;
    return {generation, signal: combinedSignal(controller.signal, externalSignal)};
  }

  #checkGeneration(generation, signal) {
    throwIfAborted(signal);
    if (!this.handle || generation !== this.generation) throw new StaleDecodeGenerationError();
  }

  push(bytes, generation = this.generation, signal) {
    this.#checkGeneration(generation, signal);
    if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
    const ptr = this.ex.input_reserve(this.handle, bytes.length);
    if (!ptr && bytes.length) throw Error(decoderError(this.ex, this.handle));
    new Uint8Array(this.ex.memory.buffer, ptr, bytes.length).set(bytes);
    this.#checkGeneration(generation, signal);
    if (this.ex.decoder_push(this.handle, bytes.length) !== 0) throw Error(decoderError(this.ex, this.handle));
  }

  finishInput(generation = this.generation, signal) {
    this.#checkGeneration(generation, signal);
    if (this.ex.decoder_finish_input(this.handle) !== 0) throw Error(decoderError(this.ex, this.handle));
  }

  resetForEpochRange(epochId) {
    if (this.ex.decoder_seek_reset_epoch(this.handle, Number(epochId)) !== 0) throw Error(decoderError(this.ex, this.handle));
  }

  index() {
    const streams = [];
    for (let i = 0, count = this.ex.container_stream_count(this.handle); i < count; i++) {
      streams.push({
        id: this.ex.container_stream_id(this.handle, i),
        kind: this.ex.container_stream_kind(this.handle, i),
        codec: this.ex.container_stream_codec(this.handle, i),
        timescale: this.ex.container_stream_timescale(this.handle, i),
        param0: this.ex.container_stream_param0(this.handle, i),
        param1: this.ex.container_stream_param1(this.handle, i),
        flags: this.ex.container_stream_flags(this.handle, i),
        meta0: this.ex.container_stream_meta0(this.handle, i),
      });
    }
    const epochs = [];
    for (let i = 0, count = this.ex.container_epoch_count(this.handle); i < count; i++) {
      epochs.push({
        id: this.ex.container_epoch_id(this.handle, i),
        pts: this.ex.container_epoch_pts(this.handle, i),
        duration: this.ex.container_epoch_duration(this.handle, i),
        offset: this.ex.container_epoch_offset(this.handle, i),
        len: this.ex.container_epoch_len(this.handle, i),
      });
    }
    return {streams, epochs};
  }

  async loadIndex(source, {signal, onRange} = {}) {
    const tx = this.#startGeneration(signal);
    const fixed = BigInt(this.ex.container_fixed_header_len());
    this.push(await source.readRange(0n, fixed, {signal: tx.signal, onRange}), tx.generation, tx.signal);
    const frontLen = BigInt(this.ex.container_front_len(this.handle));
    if (!frontLen) throw Error('validated header did not expose a front-index length');
    this.push(await source.readRange(fixed, frontLen, {signal: tx.signal, onRange}), tx.generation, tx.signal);
    this.finishInput(tx.generation, tx.signal);
    const index = this.index();
    if (!index.epochs.length) throw Error('validated front index contains no epochs');
    return {frontBytes: fixed + frontLen, ...index};
  }

  async decodeEpoch(source, epoch, {signal, onVideo, onAudio, onRange, onEvent} = {}) {
    const tx = this.#startGeneration(signal);
    this.resetForEpochRange(epoch.id);
    onEvent?.({type: 'epoch-start', id: epoch.id, offset: epoch.offset, len: epoch.len, generation: tx.generation});
    try {
      for await (const chunk of source.streamRange(epoch.offset, epoch.len, {signal: tx.signal, onRange})) {
        this.#checkGeneration(tx.generation, tx.signal);
        this.push(chunk, tx.generation, tx.signal);
        await this.drain({generation: tx.generation, signal: tx.signal, onVideo, onAudio});
      }
      this.finishInput(tx.generation, tx.signal);
      await this.drain({generation: tx.generation, signal: tx.signal, onVideo, onAudio});
      onEvent?.({type: 'epoch-finish', id: epoch.id, generation: tx.generation});
    } catch (error) {
      if (error instanceof StaleDecodeGenerationError || error?.name === 'AbortError') {
        onEvent?.({type: 'epoch-cancel', id: epoch.id, generation: tx.generation});
      } else {
        onEvent?.({type: 'epoch-error', id: epoch.id, generation: tx.generation, message: String(error?.message ?? error)});
      }
      throw error;
    }
  }

  async drain({generation = this.generation, signal, onVideo, onAudio} = {}) {
    while (true) {
      this.#checkGeneration(generation, signal);
      const popped = this.ex.decoder_pop_audio(this.handle);
      if (popped <= 0) break;
      if (!onAudio) continue;
      const count = this.ex.audio_len_samples(this.handle), ptr = this.ex.audio_ptr(this.handle);
      const audio = {
        streamId: this.ex.audio_stream_id(this.handle),
        pts: this.ex.audio_pts(this.handle),
        duration: this.ex.audio_duration(this.handle),
        rate: this.ex.audio_rate(this.handle),
        channels: this.ex.audio_channels(this.handle),
        pcm: new Int16Array(this.ex.memory.buffer, ptr, count).slice(),
      };
      await onAudio(audio);
    }
    while (true) {
      this.#checkGeneration(generation, signal);
      const popped = this.ex.decoder_pop_video(this.handle);
      if (popped <= 0) break;
      if (!onVideo) continue;
      const w = this.ex.video_width(this.handle), h = this.ex.video_height(this.handle);
      const yLen = this.ex.video_y_len(this.handle), uvLen = this.ex.video_uv_len(this.handle);
      const yuv = new Uint8Array(yLen + 2 * uvLen);
      yuv.set(new Uint8Array(this.ex.memory.buffer, this.ex.video_y_ptr(this.handle), yLen), 0);
      yuv.set(new Uint8Array(this.ex.memory.buffer, this.ex.video_u_ptr(this.handle), uvLen), yLen);
      yuv.set(new Uint8Array(this.ex.memory.buffer, this.ex.video_v_ptr(this.handle), uvLen), yLen + uvLen);
      await onVideo({streamId: this.ex.video_stream_id(this.handle), pts: this.ex.video_pts(this.handle), id: this.ex.video_frame_id(this.handle), w, h, yuv});
    }
  }
}

async function loadArtifact(url, artifact, signal) {
  return new AveluneDecoder(await instantiateArtifact(url, signal), artifact);
}

export async function createAveluneDecoder({artifact = 'auto', simdUrl = DEFAULT_SIMD, scalarUrl = DEFAULT_SCALAR, signal} = {}) {
  if (artifact === 'scalar') return loadArtifact(scalarUrl, 'scalar', signal);
  if (artifact === 'simd128') return loadArtifact(simdUrl, 'simd128', signal);
  if (artifact !== 'auto') throw Error(`unknown WASM artifact selection: ${artifact}`);
  let simdFailure;
  try { return await loadArtifact(simdUrl, 'simd128', signal); } catch (error) { simdFailure = error; }
  try { return await loadArtifact(scalarUrl, 'scalar', signal); }
  catch (scalarFailure) { throw new AggregateError([simdFailure, scalarFailure], 'neither Avelune WASM artifact could be loaded'); }
}


export class AveluneVideoEncoder {
  constructor(instance, artifact, {width, height, fpsN, fpsD = 1, qstep = 96, preset = 'balanced', epochFrames = 60, chromaLocation = 0, fullRange = false}) {
    this.instance = instance;
    this.ex = instance.exports;
    this.artifact = artifact;
    const presetId = ({fast: 0, balanced: 1, quality: 2})[preset];
    if (presetId === undefined) throw Error(`unknown encoder preset: ${preset}`);
    for (const [label, value] of Object.entries({width, height, fpsN, fpsD, qstep, epochFrames, chromaLocation})) {
      if (!Number.isSafeInteger(value) || value < 0 || value > 0xffff_ffff) throw Error(`${label} must fit an unsigned 32-bit integer`);
    }
    if (!fpsN || !fpsD || fpsN > 0xffff || fpsD > 0xffff) throw Error('frame-rate numerator and denominator must be in 1..=65535');
    const fpsFlags = ((fpsN << 16) | fpsD) >>> 0;
    const meta0 = this.ex.video_encoder_pack_meta0(chromaLocation, fullRange ? 1 : 0);
    if (meta0 === 0xffff_ffff) throw Error(`unknown chroma location: ${chromaLocation}`);
    this.handle = this.ex.video_encoder_create(width, height, fpsFlags, qstep, presetId, epochFrames, meta0);
    if (!this.handle) throw Error(encoderCreateError(this.ex));
    this.expectedFrameBytes = this.ex.video_encoder_frame_len(this.handle);
  }

  destroy() {
    if (this.handle) {
      this.ex.video_encoder_destroy(this.handle);
      this.handle = 0;
    }
  }

  pushFrame(bytes) {
    if (!this.handle) throw Error('video encoder is destroyed');
    if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
    if (bytes.length !== this.expectedFrameBytes) {
      throw Error(`raw YUV frame has ${bytes.length} bytes; expected ${this.expectedFrameBytes}`);
    }
    const ptr = this.ex.video_encoder_frame_ptr(this.handle);
    if (!ptr && bytes.length) throw Error(encoderError(this.ex, this.handle));
    new Uint8Array(this.ex.memory.buffer, ptr, bytes.length).set(bytes);
    if (this.ex.video_encoder_push_frame(this.handle) !== 0) throw Error(encoderError(this.ex, this.handle));
  }

  finish() {
    if (!this.handle) throw Error('video encoder is destroyed');
    if (this.ex.video_encoder_finish(this.handle) !== 0) throw Error(encoderError(this.ex, this.handle));
    const ptr = this.ex.video_encoder_output_ptr(this.handle), len = this.ex.video_encoder_output_len(this.handle);
    if (!len) throw Error(encoderError(this.ex, this.handle));
    return new Uint8Array(this.ex.memory.buffer, ptr, len).slice();
  }
}

async function loadVideoEncoderArtifact(url, artifact, options, signal) {
  return new AveluneVideoEncoder(await instantiateArtifact(url, signal), artifact, options);
}

export async function createAveluneVideoEncoder(options, {artifact = 'auto', simdUrl = DEFAULT_SIMD, scalarUrl = DEFAULT_SCALAR, signal} = {}) {
  if (artifact === 'scalar') return loadVideoEncoderArtifact(scalarUrl, 'scalar', options, signal);
  if (artifact === 'simd128') return loadVideoEncoderArtifact(simdUrl, 'simd128', options, signal);
  if (artifact !== 'auto') throw Error(`unknown WASM artifact selection: ${artifact}`);
  let simdFailure;
  try { return await loadVideoEncoderArtifact(simdUrl, 'simd128', options, signal); }
  catch (error) { simdFailure = error; }
  try { return await loadVideoEncoderArtifact(scalarUrl, 'scalar', options, signal); }
  catch (scalarFailure) { throw new AggregateError([simdFailure, scalarFailure], 'neither Avelune WASM encoder artifact could be loaded'); }
}


export class AveluneAvEncoder {
  constructor(instance, artifact, {
    width, height, fpsN, fpsD = 1, videoQ = 96, audioQ = 96, preset = 'balanced',
    epochFrames = 60, chromaLocation = 0, fullRange = false, audioRate = 0, audioChannels = 0,
  }) {
    this.instance = instance;
    this.ex = instance.exports;
    this.artifact = artifact;
    const presetId = ({fast: 0, balanced: 1, quality: 2})[preset];
    if (presetId === undefined) throw Error(`unknown encoder preset: ${preset}`);
    for (const [label, value] of Object.entries({width, height, fpsN, fpsD, videoQ, audioQ, epochFrames, chromaLocation, audioRate, audioChannels})) {
      if (!Number.isSafeInteger(value) || value < 0 || value > 0xffff_ffff) throw Error(`${label} must fit an unsigned 32-bit integer`);
    }
    if (!fpsN || !fpsD || fpsN > 0xffff || fpsD > 0xffff) throw Error('frame-rate numerator and denominator must be in 1..=65535');
    const fpsFlags = ((fpsN << 16) | fpsD) >>> 0;
    const meta0 = this.ex.video_encoder_pack_meta0(chromaLocation, fullRange ? 1 : 0);
    if (meta0 === 0xffff_ffff) throw Error(`unknown chroma location: ${chromaLocation}`);
    this.handle = this.ex.av_encoder_create(
      width, height, fpsFlags, videoQ, presetId, epochFrames, meta0, audioRate, audioChannels, audioQ,
    );
    if (!this.handle) throw Error(avEncoderCreateError(this.ex));
    this.expectedFrameBytes = this.ex.av_encoder_video_frame_len(this.handle);
    this.audioChannels = audioChannels;
  }

  destroy() {
    if (this.handle) {
      this.ex.av_encoder_destroy(this.handle);
      this.handle = 0;
    }
  }

  pushFrame(bytes) {
    if (!this.handle) throw Error('A/V encoder is destroyed');
    if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
    if (bytes.length !== this.expectedFrameBytes) {
      throw Error(`raw YUV frame has ${bytes.length} bytes; expected ${this.expectedFrameBytes}`);
    }
    const ptr = this.ex.av_encoder_video_frame_ptr(this.handle);
    if (!ptr && bytes.length) throw Error(avEncoderError(this.ex, this.handle));
    new Uint8Array(this.ex.memory.buffer, ptr, bytes.length).set(bytes);
    if (this.ex.av_encoder_push_video_frame(this.handle) !== 0) throw Error(avEncoderError(this.ex, this.handle));
  }

  pushAudio(samples) {
    if (!this.handle) throw Error('A/V encoder is destroyed');
    if (!this.audioChannels) {
      if (samples?.length) throw Error('audio is disabled for this encoder');
      return;
    }
    if (!(samples instanceof Int16Array)) samples = new Int16Array(samples);
    if (!samples.length) return;
    if (samples.length % this.audioChannels) throw Error('PCM samples are not aligned to the configured channel count');
    const ptr = this.ex.av_encoder_audio_reserve(this.handle, samples.length);
    if (!ptr) throw Error(avEncoderError(this.ex, this.handle));
    new Int16Array(this.ex.memory.buffer, ptr, samples.length).set(samples);
    if (this.ex.av_encoder_push_audio(this.handle, samples.length) !== 0) throw Error(avEncoderError(this.ex, this.handle));
  }

  finish() {
    if (!this.handle) throw Error('A/V encoder is destroyed');
    if (this.ex.av_encoder_finish(this.handle) !== 0) throw Error(avEncoderError(this.ex, this.handle));
    const ptr = this.ex.av_encoder_output_ptr(this.handle), len = this.ex.av_encoder_output_len(this.handle);
    if (!len) throw Error(avEncoderError(this.ex, this.handle));
    return new Uint8Array(this.ex.memory.buffer, ptr, len).slice();
  }
}

async function loadAvEncoderArtifact(url, artifact, options, signal) {
  return new AveluneAvEncoder(await instantiateArtifact(url, signal), artifact, options);
}

export async function createAveluneAvEncoder(options, {artifact = 'auto', simdUrl = DEFAULT_SIMD, scalarUrl = DEFAULT_SCALAR, signal} = {}) {
  if (artifact === 'scalar') return loadAvEncoderArtifact(scalarUrl, 'scalar', options, signal);
  if (artifact === 'simd128') return loadAvEncoderArtifact(simdUrl, 'simd128', options, signal);
  if (artifact !== 'auto') throw Error(`unknown WASM artifact selection: ${artifact}`);
  let simdFailure;
  try { return await loadAvEncoderArtifact(simdUrl, 'simd128', options, signal); }
  catch (error) { simdFailure = error; }
  try { return await loadAvEncoderArtifact(scalarUrl, 'scalar', options, signal); }
  catch (scalarFailure) { throw new AggregateError([simdFailure, scalarFailure], 'neither Avelune WASM A/V encoder artifact could be loaded'); }
}

export class AveluneStreamingAvEncoder {
  constructor(instance, artifact, {
    width, height, fpsN, fpsD = 1, videoQ = 96, audioQ = 96, preset = 'balanced',
    epochFrames = 60, chromaLocation = 0, fullRange = false, audioRate = 0, audioChannels = 0,
  }) {
    this.instance = instance;
    this.ex = instance.exports;
    this.artifact = artifact;
    const presetId = ({fast: 0, balanced: 1, quality: 2})[preset];
    if (presetId === undefined) throw Error(`unknown encoder preset: ${preset}`);
    for (const [label, value] of Object.entries({width, height, fpsN, fpsD, videoQ, audioQ, epochFrames, chromaLocation, audioRate, audioChannels})) {
      if (!Number.isSafeInteger(value) || value < 0 || value > 0xffff_ffff) throw Error(`${label} must fit an unsigned 32-bit integer`);
    }
    if (!fpsN || !fpsD || fpsN > 0xffff || fpsD > 0xffff) throw Error('frame-rate numerator and denominator must be in 1..=65535');
    const fpsFlags = ((fpsN << 16) | fpsD) >>> 0;
    const meta0 = this.ex.video_encoder_pack_meta0(chromaLocation, fullRange ? 1 : 0);
    if (meta0 === 0xffff_ffff) throw Error(`unknown chroma location: ${chromaLocation}`);
    this.handle = this.ex.av_stream_encoder_create(
      width, height, fpsFlags, videoQ, presetId, epochFrames, meta0, audioRate, audioChannels, audioQ,
    );
    if (!this.handle) throw Error(streamAvEncoderCreateError(this.ex));
    this.expectedFrameBytes = this.ex.av_stream_encoder_video_frame_len(this.handle);
    this.audioChannels = audioChannels;
  }

  destroy() {
    if (this.handle) {
      this.ex.av_stream_encoder_destroy(this.handle);
      this.handle = 0;
    }
  }

  #drainReady() {
    const chunks = [];
    while (this.ex.av_stream_encoder_ready_epochs(this.handle) > 0) {
      const status = this.ex.av_stream_encoder_take_epoch(this.handle);
      if (status < 0) throw Error(streamAvEncoderError(this.ex, this.handle));
      if (status === 0) break;
      const ptr = this.ex.av_stream_encoder_epoch_ptr(this.handle);
      const len = this.ex.av_stream_encoder_epoch_len(this.handle);
      if (!len) throw Error('streaming A/V encoder exposed an empty ready epoch');
      chunks.push(new Uint8Array(this.ex.memory.buffer, ptr, len).slice());
    }
    return chunks;
  }

  pushFrame(bytes) {
    if (!this.handle) throw Error('streaming A/V encoder is destroyed');
    if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
    if (bytes.length !== this.expectedFrameBytes) {
      throw Error(`raw YUV frame has ${bytes.length} bytes; expected ${this.expectedFrameBytes}`);
    }
    const ptr = this.ex.av_stream_encoder_video_frame_ptr(this.handle);
    if (!ptr && bytes.length) throw Error(streamAvEncoderError(this.ex, this.handle));
    new Uint8Array(this.ex.memory.buffer, ptr, bytes.length).set(bytes);
    if (this.ex.av_stream_encoder_push_video_frame(this.handle) !== 0) throw Error(streamAvEncoderError(this.ex, this.handle));
    return this.#drainReady();
  }

  pushAudio(samples) {
    if (!this.handle) throw Error('streaming A/V encoder is destroyed');
    if (!this.audioChannels) {
      if (samples?.length) throw Error('audio is disabled for this encoder');
      return [];
    }
    if (!(samples instanceof Int16Array)) samples = new Int16Array(samples);
    if (!samples.length) return [];
    if (samples.length % this.audioChannels) throw Error('PCM samples are not aligned to the configured channel count');
    const ptr = this.ex.av_stream_encoder_audio_reserve(this.handle, samples.length);
    if (!ptr) throw Error(streamAvEncoderError(this.ex, this.handle));
    new Int16Array(this.ex.memory.buffer, ptr, samples.length).set(samples);
    if (this.ex.av_stream_encoder_push_audio(this.handle, samples.length) !== 0) throw Error(streamAvEncoderError(this.ex, this.handle));
    return this.#drainReady();
  }

  finish() {
    if (!this.handle) throw Error('streaming A/V encoder is destroyed');
    if (this.ex.av_stream_encoder_finish(this.handle) !== 0) throw Error(streamAvEncoderError(this.ex, this.handle));
    const epochs = this.#drainReady();
    const ptr = this.ex.av_stream_encoder_prefix_ptr(this.handle), len = this.ex.av_stream_encoder_prefix_len(this.handle);
    if (!len) throw Error(streamAvEncoderError(this.ex, this.handle));
    return {prefix: new Uint8Array(this.ex.memory.buffer, ptr, len).slice(), epochs};
  }
}

async function loadStreamingAvEncoderArtifact(url, artifact, options, signal) {
  return new AveluneStreamingAvEncoder(await instantiateArtifact(url, signal), artifact, options);
}

export async function createAveluneStreamingAvEncoder(options, {artifact = 'auto', simdUrl = DEFAULT_SIMD, scalarUrl = DEFAULT_SCALAR, signal} = {}) {
  if (artifact === 'scalar') return loadStreamingAvEncoderArtifact(scalarUrl, 'scalar', options, signal);
  if (artifact === 'simd128') return loadStreamingAvEncoderArtifact(simdUrl, 'simd128', options, signal);
  if (artifact !== 'auto') throw Error(`unknown WASM artifact selection: ${artifact}`);
  let simdFailure;
  try { return await loadStreamingAvEncoderArtifact(simdUrl, 'simd128', options, signal); }
  catch (error) { simdFailure = error; }
  try { return await loadStreamingAvEncoderArtifact(scalarUrl, 'scalar', options, signal); }
  catch (scalarFailure) { throw new AggregateError([simdFailure, scalarFailure], 'neither Avelune WASM streaming A/V encoder artifact could be loaded'); }
}
