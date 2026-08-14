import {BlobRangeSource, HttpRangeSource, StaleDecodeGenerationError, createAveluneDecoder} from './avelune-loader.js';
import {createRenderer} from './renderers.js';

const $ = id => document.getElementById(id);
const setText = (id, value) => { const node = $(id); if (node) node.textContent = value; };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const seconds = value => Number(value) / 1e6;


function streamFormat(stream) {
  const codec = stream.codec === 1 ? (stream.kind === 1 ? 'ALV1' : 'ALA1') : `codec ${stream.codec}`;
  return stream.kind === 1
    ? `#${stream.id} · ${codec} · ${stream.param0} × ${stream.param1}`
    : `#${stream.id} · ${codec} · ${stream.param0} Hz · ${stream.param1} ch`;
}

class EventLog {
  constructor(node, limit = 160) { this.node = node; this.limit = limit; this.lines = []; }
  clear() { this.lines = []; this.#render(); }
  add(message) {
    const stamp = new Date().toISOString().slice(11, 23);
    this.lines.push(`${stamp}  ${message}`);
    if (this.lines.length > this.limit) this.lines.splice(0, this.lines.length - this.limit);
    this.#render();
  }
  #render() { if (this.node) { this.node.textContent = this.lines.join('\n'); this.node.scrollTop = this.node.scrollHeight; } }
}

class AudioScheduler {
  constructor() { this.context = null; this.sources = new Set(); this.volume = 1; this.gain = null; }
  async ensure() {
    if (!this.context) {
      this.context = new AudioContext();
      this.gain = this.context.createGain();
      this.gain.gain.value = this.volume;
      this.gain.connect(this.context.destination);
    }
    await this.context.resume();
  }
  setVolume(value) { this.volume = value; if (this.gain) this.gain.gain.value = value; }
  stopAll() {
    for (const source of this.sources) { try { source.stop(); } catch {} }
    this.sources.clear();
  }
  async schedule(packet, mediaStart, contextStart, signal) {
    await this.ensure();
    let packetStart = seconds(packet.pts);
    const packetDuration = packet.pcm.length / packet.channels / packet.rate;
    let skipFrames = 0;
    if (packetStart + packetDuration <= mediaStart) return;
    if (packetStart < mediaStart) {
      skipFrames = Math.min(packet.pcm.length / packet.channels, Math.floor((mediaStart - packetStart) * packet.rate));
      packetStart += skipFrames / packet.rate;
    }
    let when = contextStart + Math.max(0, packetStart - mediaStart);
    // Keep queued WebAudio work bounded. This also provides backpressure for audio-only files.
    while (when - this.context.currentTime > 0.75) {
      if (signal.aborted) throw signal.reason;
      await sleep(Math.min(100, (when - this.context.currentTime - 0.6) * 1000));
    }
    const frameCount = packet.pcm.length / packet.channels - skipFrames;
    if (frameCount <= 0) return;
    const buffer = this.context.createBuffer(packet.channels, frameCount, packet.rate);
    for (let channel = 0; channel < packet.channels; channel++) {
      const out = buffer.getChannelData(channel);
      for (let i = 0; i < frameCount; i++) out[i] = packet.pcm[(i + skipFrames) * packet.channels + channel] / 32768;
    }
    const source = this.context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gain);
    source.addEventListener('ended', () => this.sources.delete(source), {once: true});
    this.sources.add(source);
    source.start(Math.max(this.context.currentTime, when));
  }
}

class PlayerController {
  constructor() {
    this.decoder = null;
    this.source = null;
    this.index = null;
    this.renderer = null;
    this.state = 'idle';
    this.duration = 0;
    this.playAbort = null;
    this.playStartMedia = 0;
    this.playStartWall = 0;
    this.playStartAudioContext = 0;
    this.audio = new AudioScheduler();
    this.log = new EventLog($('event-log'));
    this.frameId = null;
    this.videoStreamId = null;
    this.audioStreamId = null;
  }

  setState(state, detail = '') {
    this.state = state;
    setText('state-label', state.toUpperCase());
    setText('state-detail', detail);
    $('play').textContent = state === 'playing' ? 'Pause' : 'Play';
    $('play').disabled = !['ready', 'playing', 'paused', 'ended'].includes(state);
  }

  cancelPlayback() {
    this.playAbort?.abort(new DOMException('playback superseded', 'AbortError'));
    this.playAbort = null;
    this.decoder?.cancel();
    this.audio.stopAll();
  }

  currentTime() {
    if (this.state === 'playing') {
      const elapsed = this.audioStreamId !== null && this.audio.context
        ? this.audio.context.currentTime - this.playStartAudioContext
        : (performance.now() - this.playStartWall) / 1000;
      return Math.min(this.duration, this.playStartMedia + Math.max(0, elapsed));
    }
    return Number($('seek').value) || 0;
  }

  updateTime(value) {
    const clamped = Math.max(0, Math.min(this.duration || 0, value));
    $('seek').value = String(clamped);
    const display = `${clamped.toFixed(2)}s`;
    setText('time', display);
    setText('playback-time', display);
  }

  async load(source) {
    this.cancelPlayback();
    this.decoder?.destroy();
    this.decoder = null;
    this.renderer?.destroy?.();
    this.renderer = null;
    this.index = null;
    this.videoStreamId = null;
    this.audioStreamId = null;
    this.source = source;
    for (const id of ['source-name','source-size','wasm-name','format-renderer','container-streams','container-epochs','container-front','stream-list','format-video','format-audio','format-dimensions','playback-epoch','playback-frame','playback-range']) setText(id, '—');
    this.log.clear();
    this.setState('loading', source.label);
    $('seek').disabled = true;
    try {
      const decoder = await createAveluneDecoder({artifact: $('wasm-artifact').value});
      this.decoder = decoder;
      this.log.add(`WASM ${decoder.artifact} loaded`);
      const index = await decoder.loadIndex(source, {
        onRange: range => this.rangeEvent(range),
      });
      if (decoder !== this.decoder) return;
      this.index = index;
      const videos = index.streams.filter(s => s.kind === 1);
      const audios = index.streams.filter(s => s.kind === 2);
      if (videos.length > 1 || audios.length > 1) {
        throw Error(`browser demo currently supports at most one video and one audio stream; source declares ${videos.length} video / ${audios.length} audio`);
      }
      if (!videos.length && !audios.length) throw Error('source declares no playable audio/video streams');
      const video = videos[0], audio = audios[0];
      this.videoStreamId = video?.id ?? null;
      this.audioStreamId = audio?.id ?? null;
      const renderer = await createRenderer($('c'), video?.meta0 ?? 0, $('renderer-choice').value);
      if (decoder !== this.decoder) { renderer.renderer.destroy?.(); return; }
      this.renderer = renderer.renderer;
      this.duration = Math.max(...index.epochs.map(e => seconds(e.pts) + seconds(e.duration)));
      $('seek').max = String(this.duration);
      $('seek').value = '0';
      $('seek').disabled = false;
      setText('source-name', source.label);
      setText('source-size', source.size === null ? 'HTTP / unknown' : `${source.size} B`);
      setText('wasm-name', decoder.artifact);
      setText('format-renderer', renderer.name);
      setText('container-streams', String(index.streams.length));
      setText('container-epochs', String(index.epochs.length));
      setText('container-front', `${index.frontBytes} B`);
      setText('stream-list', index.streams.map(streamFormat).join('\n') || '—');
      setText('format-video', video ? streamFormat(video) : '—');
      setText('format-audio', audio ? streamFormat(audio) : '—');
      this.updateTime(0);
      this.setState('ready', `${index.epochs.length} indexed epoch${index.epochs.length === 1 ? '' : 's'}`);
      this.log.add(`index streams=${index.streams.length} epochs=${index.epochs.length} front=${index.frontBytes}B`);
    } catch (error) {
      this.log.add(`load error: ${error.message ?? error}`);
      this.setState('error', error.message ?? String(error));
      throw error;
    }
  }

  rangeEvent(range) {
    const label = `${range.first}–${range.last}`;
    setText('playback-range', label);
    this.log.add(`${range.source} range ${label} (${range.length} B)`);
  }

  epochEvent(event) {
    if (event.type === 'epoch-start') this.log.add(`epoch ${event.id} decode start generation=${event.generation}`);
    else if (event.type === 'epoch-finish') this.log.add(`epoch ${event.id} complete`);
    else if (event.type === 'epoch-error') this.log.add(`epoch ${event.id} error: ${event.message}`);
  }

  async play() {
    if (!this.decoder || !this.index || !this.renderer || !['ready', 'paused', 'ended'].includes(this.state)) return;
    const start = this.state === 'ended' ? 0 : Number($('seek').value) || 0;
    this.cancelPlayback();
    const controller = new AbortController();
    this.playAbort = controller;
    await this.audio.ensure();
    if (controller.signal.aborted || this.playAbort !== controller) return;
    const contextStart = this.audio.context.currentTime + 0.05;
    this.playStartMedia = start;
    this.playStartWall = performance.now() + 50;
    this.playStartAudioContext = contextStart;
    this.setState('playing');
    const firstEpoch = Math.max(0, this.index.epochs.findLastIndex(epoch => seconds(epoch.pts) <= start));
    try {
      for (let i = firstEpoch; i < this.index.epochs.length; i++) {
        if (controller.signal.aborted) throw controller.signal.reason;
        const epoch = this.index.epochs[i];
        setText('playback-epoch', `${i + 1} / ${this.index.epochs.length} · id ${epoch.id}`);
        await this.decoder.decodeEpoch(this.source, epoch, {
          signal: controller.signal,
          onRange: range => this.rangeEvent(range),
          onEvent: event => this.epochEvent(event),
          onAudio: packet => packet.streamId === this.audioStreamId ? this.audio.schedule(packet, start, contextStart, controller.signal) : undefined,
          onVideo: async frame => {
            if (frame.streamId !== this.videoStreamId) return;
            const frameTime = seconds(frame.pts);
            if (frameTime < start) return;
            if (this.audioStreamId !== null) {
              const target = contextStart + (frameTime - start);
              while (this.audio.context.currentTime + 0.001 < target) {
                if (controller.signal.aborted) throw controller.signal.reason;
                await sleep(Math.min(40, (target - this.audio.context.currentTime) * 1000));
              }
            } else {
              const target = this.playStartWall + (frameTime - start) * 1000;
              while (performance.now() + 1 < target) {
                if (controller.signal.aborted) throw controller.signal.reason;
                await sleep(Math.min(40, target - performance.now()));
              }
            }
            this.renderer.render(frame);
            this.frameId = frame.id;
            setText('playback-frame', `stream ${frame.streamId} · frame ${frame.id}`);
            setText('format-dimensions', `${frame.w} × ${frame.h}`);
            this.updateTime(frameTime);
          },
        });
        this.updateTime(this.currentTime());
      }
      // Audio-only media can finish decoding before the final scheduled sample has played.
      while (!controller.signal.aborted && this.currentTime() < this.duration) {
        this.updateTime(this.currentTime());
        await sleep(50);
      }
      if (!controller.signal.aborted) {
        this.updateTime(this.duration);
        this.setState('ended');
      }
    } catch (error) {
      if (error?.name === 'AbortError' || error instanceof StaleDecodeGenerationError) return;
      this.log.add(`playback error: ${error.message ?? error}`);
      this.setState('error', error.message ?? String(error));
    } finally {
      if (this.playAbort === controller) this.playAbort = null;
    }
  }

  pause() {
    if (this.state !== 'playing') return;
    const at = this.currentTime();
    this.cancelPlayback();
    this.updateTime(at);
    this.setState('paused');
  }

  async seek(value) {
    const wasPlaying = this.state === 'playing';
    if (wasPlaying) this.pause();
    this.updateTime(value);
    if (wasPlaying) await this.play();
  }

  destroy() {
    this.cancelPlayback();
    this.decoder?.destroy();
    this.renderer?.destroy?.();
  }
}


let encodedObjectUrl = null;

async function encodeLocalY4m() {
  const file = $('encode-file').files?.[0];
  if (!file) throw Error('choose a local .y4m file first');
  const status = $('encode-status');
  const button = $('encode-y4m');
  button.disabled = true;
  status.textContent = 'Reading Y4M…';
  const worker = new Worker(new URL('./encoder-worker.js', import.meta.url), {type: 'module'});
  try {
    const input = await file.arrayBuffer();
    const result = await new Promise((resolve, reject) => {
      worker.addEventListener('message', event => {
        if (event.data?.type === 'progress') {
          status.textContent = `Encoding ${event.data.done} / ${event.data.total} frames…`;
        } else if (event.data?.type === 'done') {
          resolve(event.data);
        } else if (event.data?.type === 'error') {
          reject(Error(event.data.message));
        }
      });
      worker.addEventListener('error', event => reject(Error(event.message || 'encoder worker failed')), {once: true});
      worker.postMessage({
        type: 'encode',
        buffer: input,
        qstep: Number($('encode-q').value),
        preset: $('encode-preset').value,
        artifact: $('wasm-artifact').value,
      }, [input]);
    });
    const encoded = new Uint8Array(result.encoded);
    const blob = new Blob([encoded], {type: 'application/octet-stream'});
    const stem = file.name.replace(/\.y4m$/i, '') || 'encoded';
    if (encodedObjectUrl) URL.revokeObjectURL(encodedObjectUrl);
    encodedObjectUrl = URL.createObjectURL(blob);
    const download = $('encode-download');
    download.href = encodedObjectUrl;
    download.download = `${stem}.avl`;
    download.hidden = false;
    download.textContent = `Save ${stem}.avl`;
    status.textContent = `${result.frames} frames → ${encoded.length.toLocaleString()} B · ${result.artifact}`;
    await player.load(new BlobRangeSource(blob, `${stem}.avl`));
  } finally {
    worker.terminate();
    button.disabled = false;
  }
}

const player = new PlayerController();

function reportUiError(error) {
  if (error?.name !== 'AbortError' && !(error instanceof StaleDecodeGenerationError)) console.error(error);
}

$('load-sample').addEventListener('click', () => {
  const option = $('sample').selectedOptions[0];
  player.load(new HttpRangeSource(option.value)).catch(reportUiError);
});
$('load-url').addEventListener('click', () => player.load(new HttpRangeSource($('url').value.trim())).catch(reportUiError));
$('file').addEventListener('change', event => {
  const file = event.target.files?.[0];
  if (file) player.load(new BlobRangeSource(file, file.name)).catch(reportUiError);
});
$('play').addEventListener('click', () => {
  if (player.state === 'playing') player.pause(); else player.play().catch(reportUiError);
});
$('seek').addEventListener('change', () => player.seek(Number($('seek').value)).catch(reportUiError));
$('volume').addEventListener('input', () => player.audio.setVolume(Number($('volume').value)));
$('encode-y4m').addEventListener('click', () => encodeLocalY4m().catch(error => { $('encode-status').textContent = error.message ?? String(error); reportUiError(error); }));

for (const eventName of ['dragenter', 'dragover']) {
  $('drop-target').addEventListener(eventName, event => { event.preventDefault(); $('drop-target').dataset.drop = 'true'; });
}
for (const eventName of ['dragleave', 'drop']) {
  $('drop-target').addEventListener(eventName, event => { event.preventDefault(); delete $('drop-target').dataset.drop; });
}
$('drop-target').addEventListener('drop', event => {
  const file = event.dataTransfer?.files?.[0];
  if (file) player.load(new BlobRangeSource(file, file.name)).catch(reportUiError);
});
window.addEventListener('pagehide', () => { player.destroy(); if (encodedObjectUrl) URL.revokeObjectURL(encodedObjectUrl); });
