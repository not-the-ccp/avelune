// Production browser binding. Container/header/packet parsing lives in Rust; JavaScript owns
// HTTP Range scheduling, presentation, and audio timing.
const DEFAULT_SCALAR = 'avelune-prod-scalar.wasm';
const DEFAULT_SIMD = 'avelune-prod-simd128.wasm';

async function fetchBytes(url, init) {
  const response = await fetch(url, {...init, cache: 'no-store'});
  if (!response.ok) throw Error(`${url}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}

async function instantiate(bytes) {
  if (!WebAssembly.validate(bytes)) throw Error('WASM validation failed');
  return (await WebAssembly.instantiate(bytes, {})).instance;
}

function decodeError(ex, handle) {
  const p = ex.decoder_last_error_ptr(handle), n = ex.decoder_last_error_len(handle);
  return n ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, p, n)) : 'unknown decoder error';
}

export class AveluneProdDecoder {
  constructor(instance, backend) {
    this.instance = instance;
    this.ex = instance.exports;
    this.backend = backend;
    this.handle = this.ex.decoder_create();
    if (!this.handle) throw Error('decoder_create failed');
  }

  destroy() {
    if (this.handle) {
      this.ex.decoder_destroy(this.handle);
      this.handle = 0;
    }
  }

  push(bytes) {
    if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
    const p = this.ex.input_reserve(this.handle, bytes.length);
    if (!p && bytes.length) throw Error(decodeError(this.ex, this.handle));
    new Uint8Array(this.ex.memory.buffer, p, bytes.length).set(bytes);
    if (this.ex.decoder_push(this.handle, bytes.length) !== 0) {
      throw Error(decodeError(this.ex, this.handle));
    }
  }

  resetForEpochRange(epochId) {
    const rc = this.ex.decoder_seek_reset_epoch
      ? this.ex.decoder_seek_reset_epoch(this.handle, epochId)
      : this.ex.decoder_seek_reset(this.handle);
    if (rc !== 0) throw Error('decoder epoch reset failed');
  }

  index() {
    const streams = [];
    for (let i = 0, n = this.ex.container_stream_count(this.handle); i < n; i++) {
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
    for (let i = 0, n = this.ex.container_epoch_count(this.handle); i < n; i++) {
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

  async loadIndex(url) {
    const fixed = this.ex.container_fixed_header_len();
    const header = await this.#range(url, 0, fixed - 1);
    this.push(header);
    const frontLen = this.ex.container_front_len(this.handle);
    if (!frontLen) throw Error('validated header did not expose a front index length');
    this.push(await this.#range(url, fixed, fixed + frontLen - 1));
    const index = this.index();
    if (!index.epochs.length) throw Error('validated front index contains no epochs');
    return {frontBytes: fixed + frontLen, ...index};
  }

  async decodeEpoch(url, epoch, {onVideo, onAudio, onRange} = {}) {
    this.resetForEpochRange(epoch.id);
    const end = epoch.offset + epoch.len - 1n;
    if (onRange) onRange(epoch.offset, end);
    const response = await fetch(url, {
      headers: {Range: `bytes=${epoch.offset}-${end}`},
      cache: 'no-store',
    });
    if (response.status !== 206) throw Error(`server must support HTTP Range; got ${response.status}`);
    if (!response.body) {
      this.push(new Uint8Array(await response.arrayBuffer()));
      this.drain({onVideo, onAudio});
      return;
    }
    const reader = response.body.getReader();
    while (true) {
      const {done, value} = await reader.read();
      if (done) break;
      this.push(value);
      this.drain({onVideo, onAudio});
    }
    this.drain({onVideo, onAudio});
  }

  drain({onVideo, onAudio} = {}) {
    while (this.ex.decoder_pop_audio(this.handle) > 0) {
      if (!onAudio) continue;
      const n = this.ex.audio_len_samples(this.handle), p = this.ex.audio_ptr(this.handle);
      // Copy because the Rust getter storage is replaced by the next pop.
      onAudio({
        pts: this.ex.audio_pts(this.handle),
        duration: this.ex.audio_duration(this.handle),
        rate: this.ex.audio_rate(this.handle),
        channels: this.ex.audio_channels(this.handle),
        pcm: new Int16Array(this.ex.memory.buffer, p, n).slice(),
      });
    }
    while (this.ex.decoder_pop_video(this.handle) > 0) {
      if (!onVideo) continue;
      const w = this.ex.video_width(this.handle), h = this.ex.video_height(this.handle);
      const yn = this.ex.video_y_len(this.handle), cn = this.ex.video_uv_len(this.handle);
      const out = new Uint8Array(yn + 2 * cn);
      out.set(new Uint8Array(this.ex.memory.buffer, this.ex.video_y_ptr(this.handle), yn), 0);
      out.set(new Uint8Array(this.ex.memory.buffer, this.ex.video_u_ptr(this.handle), cn), yn);
      out.set(new Uint8Array(this.ex.memory.buffer, this.ex.video_v_ptr(this.handle), cn), yn + cn);
      onVideo({pts: this.ex.video_pts(this.handle), id: this.ex.video_frame_id(this.handle), w, h, yuv: out});
    }
  }

  async #range(url, first, last) {
    const response = await fetch(url, {headers: {Range: `bytes=${first}-${last}`}, cache: 'no-store'});
    if (response.status !== 206) throw Error(`server must support HTTP Range; got ${response.status} for ${first}-${last}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length !== last - first + 1) throw Error('short HTTP Range response');
    return bytes;
  }
}

export async function createAveluneProdDecoder({simdUrl = DEFAULT_SIMD, scalarUrl = DEFAULT_SCALAR} = {}) {
  let simdFailure;
  try {
    const bytes = await fetchBytes(simdUrl);
    const instance = await instantiate(bytes);
    return new AveluneProdDecoder(instance, 'simd128');
  } catch (error) {
    simdFailure = error;
  }
  try {
    const instance = await instantiate(await fetchBytes(scalarUrl));
    return new AveluneProdDecoder(instance, 'scalar');
  } catch (scalarFailure) {
    throw new AggregateError([simdFailure, scalarFailure], 'neither production WASM backend could be loaded');
  }
}
