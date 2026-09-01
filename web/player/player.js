import {BlobRangeSource, HttpRangeSource, StaleDecodeGenerationError, createAveluneDecoder} from './avelune-loader.js';
import {createRenderer} from './renderers.js';

const $ = id => document.getElementById(id);
const setText = (id, value) => { const node = $(id); if (node) node.textContent = value; };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const seconds = value => Number(value) / 1e6;

function formatBytes(value) {
  if (value === null || value === undefined) return 'unknown';
  let n = Number(value);
  if (!Number.isFinite(n)) return `${value} B`;
  const units = ['B', 'KiB', 'MiB', 'GiB'];
  let unit = 0;
  while (n >= 1024 && unit < units.length - 1) { n /= 1024; unit++; }
  const digits = unit === 0 ? 0 : n >= 100 ? 0 : n >= 10 ? 1 : 2;
  return `${n.toFixed(digits)} ${units[unit]}`;
}

function formatTime(value) {
  const total = Math.max(0, Number(value) || 0);
  const minutes = Math.floor(total / 60);
  const secondsPart = total - minutes * 60;
  return minutes ? `${minutes}:${secondsPart.toFixed(2).padStart(5, '0')}` : `${secondsPart.toFixed(2)}s`;
}

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
  #render() {
    if (!this.node) return;
    this.node.textContent = this.lines.join('\n');
    this.node.scrollTop = this.node.scrollHeight;
  }
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
    const when = contextStart + Math.max(0, packetStart - mediaStart);
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
    this.rangeRequests = 0;
    this.rangeBytes = 0n;
    this.framesRendered = 0;
    this.loadGeneration = 0;
  }

  setState(state, detail = '') {
    this.state = state;
    setText('state-label', state.toUpperCase());
    setText('state-detail', detail);
    $('play').textContent = state === 'playing' ? 'Pause' : state === 'ended' ? 'Replay' : 'Play';
    $('play').disabled = !['ready', 'playing', 'paused', 'ended'].includes(state);
    const overlay = $('viewport-state');
    if (overlay) {
      overlay.hidden = !['idle', 'loading', 'error'].includes(state);
      overlay.dataset.state = state;
      overlay.textContent = state === 'loading' ? 'Loading stream index…' : state === 'error' ? detail : 'Choose a sample or open an .avl file.';
    }
  }

  resetMetrics() {
    this.rangeRequests = 0;
    this.rangeBytes = 0n;
    this.framesRendered = 0;
    setText('metric-index', '—');
    setText('metric-ranges', '0');
    setText('metric-bytes', '0 B');
    setText('metric-frames', '0');
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
    const display = formatTime(clamped);
    setText('time', display);
    setText('playback-time', display);
  }

  async load(source) {
    const generation = ++this.loadGeneration;
    this.cancelPlayback();
    this.decoder?.destroy();
    this.decoder = null;
    this.renderer?.destroy?.();
    this.renderer = null;
    this.index = null;
    this.videoStreamId = null;
    this.audioStreamId = null;
    this.source = source;
    this.duration = 0;
    this.resetMetrics();
    for (const id of ['source-name','source-size','wasm-name','format-renderer','container-streams','container-epochs','container-front','stream-list','format-video','format-audio','format-dimensions','playback-epoch','playback-frame','playback-range','playback-duration']) setText(id, '—');
    this.log.clear();
    this.setState('loading', source.label);
    $('seek').disabled = true;
    const started = performance.now();

    try {
      const decoder = await createAveluneDecoder({artifact: $('wasm-artifact').value});
      if (generation !== this.loadGeneration) {
        decoder.destroy();
        return;
      }
      this.decoder = decoder;
      this.log.add(`WASM ${decoder.artifact} loaded`);
      const index = await decoder.loadIndex(source, {
        onRange: range => this.rangeEvent(range),
      });
      if (generation !== this.loadGeneration || decoder !== this.decoder) return;

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
      if (generation !== this.loadGeneration || decoder !== this.decoder) {
        renderer.renderer.destroy?.();
        return;
      }
      this.renderer = renderer.renderer;
      this.duration = Math.max(...index.epochs.map(e => seconds(e.pts) + seconds(e.duration)));
      $('seek').max = String(this.duration);
      $('seek').value = '0';
      $('seek').disabled = false;

      setText('source-name', source.label);
      setText('source-size', source.size === null ? 'HTTP / unknown' : formatBytes(source.size));
      setText('wasm-name', decoder.artifact);
      setText('format-renderer', renderer.name);
      setText('container-streams', String(index.streams.length));
      setText('container-epochs', String(index.epochs.length));
      setText('container-front', formatBytes(index.frontBytes));
      setText('stream-list', index.streams.map(streamFormat).join('\n') || '—');
      setText('format-video', video ? streamFormat(video) : '—');
      setText('format-audio', audio ? streamFormat(audio) : '—');
      setText('format-dimensions', video ? `${video.param0} × ${video.param1}` : '—');
      setText('playback-duration', formatTime(this.duration));
      setText('metric-index', `${(performance.now() - started).toFixed(1)} ms`);
      this.updateTime(0);
      this.setState('ready', `${index.epochs.length} indexed epoch${index.epochs.length === 1 ? '' : 's'} · ${formatTime(this.duration)}`);
      this.log.add(`index streams=${index.streams.length} epochs=${index.epochs.length} front=${index.frontBytes}B`);
    } catch (error) {
      if (generation !== this.loadGeneration || error?.name === 'AbortError' || error instanceof StaleDecodeGenerationError) return;
      this.log.add(`load error: ${error.message ?? error}`);
      this.setState('error', error.message ?? String(error));
      throw error;
    }
  }

  rangeEvent(range) {
    const label = `${range.first}–${range.last}`;
    this.rangeRequests++;
    this.rangeBytes += BigInt(range.length);
    setText('playback-range', label);
    setText('metric-ranges', String(this.rangeRequests));
    setText('metric-bytes', formatBytes(this.rangeBytes));
    this.log.add(`${range.source} range ${label} (${formatBytes(range.length)})`);
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

    // Video-only playback does not need to wake an AudioContext. Apart from
    // avoiding needless work, this keeps video-only samples usable on browsers
    // with strict user-activation rules for audio.
    let contextStart = 0;
    if (this.audioStreamId !== null) {
      await this.audio.ensure();
      if (controller.signal.aborted || this.playAbort !== controller) return;
      contextStart = this.audio.context.currentTime + 0.05;
      this.playStartAudioContext = contextStart;
    }

    this.playStartMedia = start;
    this.playStartWall = performance.now() + (this.audioStreamId !== null ? 50 : 0);
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
            this.framesRendered++;
            setText('metric-frames', String(this.framesRendered));
            setText('playback-frame', `stream ${frame.streamId} · frame ${frame.id}`);
            this.updateTime(frameTime);
          },
        });
        this.updateTime(this.currentTime());
      }

      while (!controller.signal.aborted && this.currentTime() < this.duration) {
        this.updateTime(this.currentTime());
        await sleep(50);
      }
      if (!controller.signal.aborted) {
        this.updateTime(this.duration);
        this.setState('ended', `${this.framesRendered} video frame${this.framesRendered === 1 ? '' : 's'} presented`);
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
    this.setState('paused', `Paused at ${formatTime(at)}`);
  }

  async seek(value) {
    if (!this.index) return;
    const wasPlaying = this.state === 'playing';
    if (wasPlaying) this.pause();
    this.updateTime(value);
    if (wasPlaying) await this.play();
  }

  async seekBy(delta) {
    if (!this.index) return;
    await this.seek(Math.max(0, Math.min(this.duration, this.currentTime() + delta)));
  }

  destroy() {
    this.loadGeneration++;
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
    status.textContent = `${result.frames} frames → ${formatBytes(encoded.length)} · ${result.artifact}`;
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

function selectedSampleSource() {
  const option = $('sample').selectedOptions[0];
  return new HttpRangeSource(option.value);
}

function updateSampleDescription() {
  const option = $('sample').selectedOptions[0];
  setText('sample-description', option?.dataset.description ?? '');
}

function loadUrl() {
  const raw = $('url').value.trim();
  if (!raw) {
    player.setState('error', 'Enter an HTTP(S) .avl URL first.');
    return;
  }
  let url;
  try { url = new URL(raw, location.href); }
  catch { player.setState('error', 'The source URL is not valid.'); return; }
  if (!['http:', 'https:'].includes(url.protocol)) {
    player.setState('error', 'Remote sources must use HTTP or HTTPS.');
    return;
  }
  player.load(new HttpRangeSource(url.href)).catch(reportUiError);
}

$('load-sample').addEventListener('click', () => player.load(selectedSampleSource()).catch(reportUiError));
$('sample').addEventListener('change', updateSampleDescription);
$('load-url').addEventListener('click', loadUrl);
$('url').addEventListener('keydown', event => { if (event.key === 'Enter') loadUrl(); });
$('file').addEventListener('change', event => {
  const file = event.target.files?.[0];
  if (file) player.load(new BlobRangeSource(file, file.name)).catch(reportUiError);
});
$('play').addEventListener('click', () => {
  if (player.state === 'playing') player.pause(); else player.play().catch(reportUiError);
});
$('seek').addEventListener('input', () => {
  if (player.state !== 'playing') player.updateTime(Number($('seek').value));
});
$('seek').addEventListener('change', () => player.seek(Number($('seek').value)).catch(reportUiError));
$('volume').addEventListener('input', () => player.audio.setVolume(Number($('volume').value)));
$('encode-y4m').addEventListener('click', () => encodeLocalY4m().catch(error => {
  $('encode-status').textContent = error.message ?? String(error);
  reportUiError(error);
}));

for (const id of ['wasm-artifact', 'renderer-choice']) {
  $(id).addEventListener('change', () => {
    if (player.source) player.load(player.source).catch(reportUiError);
  });
}

for (const eventName of ['dragenter', 'dragover']) {
  $('drop-target').addEventListener(eventName, event => {
    event.preventDefault();
    $('drop-target').dataset.drop = 'true';
  });
}
for (const eventName of ['dragleave', 'drop']) {
  $('drop-target').addEventListener(eventName, event => {
    event.preventDefault();
    delete $('drop-target').dataset.drop;
  });
}
$('drop-target').addEventListener('drop', event => {
  const file = event.dataTransfer?.files?.[0];
  if (file) player.load(new BlobRangeSource(file, file.name)).catch(reportUiError);
});

window.addEventListener('keydown', event => {
  const target = event.target;
  if (target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'SELECT', 'TEXTAREA', 'BUTTON'].includes(target.tagName))) return;
  if (event.code === 'Space') {
    event.preventDefault();
    if (player.state === 'playing') player.pause();
    else player.play().catch(reportUiError);
  } else if (event.key === 'ArrowLeft') {
    event.preventDefault();
    player.seekBy(-5).catch(reportUiError);
  } else if (event.key === 'ArrowRight') {
    event.preventDefault();
    player.seekBy(5).catch(reportUiError);
  } else if (event.key === 'Home') {
    event.preventDefault();
    player.seek(0).catch(reportUiError);
  } else if (event.key === 'End') {
    event.preventDefault();
    player.seek(player.duration).catch(reportUiError);
  }
});

window.addEventListener('pagehide', () => {
  player.destroy();
  if (encodedObjectUrl) URL.revokeObjectURL(encodedObjectUrl);
});

updateSampleDescription();
// Loading the index does not autoplay media or create an AudioContext, so the
// demo can be useful immediately without violating browser activation rules.
player.load(selectedSampleSource()).catch(reportUiError);
