import {BlobRangeSource, HttpRangeSource, StaleDecodeGenerationError, createAveluneDecoder} from './avelune-loader.js';
import {AudioSink, AudioUnderrunError} from './audio-sink.js';
import {PlaybackBuffer} from './playback-buffer.js';
import {createRenderer} from './renderers.js';

const $ = id => document.getElementById(id);
const setText = (id, value) => { const node = $(id); if (node) node.textContent = value; };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const seconds = value => Number(value) / 1e6;
const PLAYBACK_START_DELAY = 0.12;
const AUDIO_SCHEDULE_AHEAD = 0.75;
const PLAYBACK_POLL_MS = 12;

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

function ensurePlaybackInspectorMetrics() {
  const list = $('playback-time')?.closest('dl');
  if (!list) return;
  const rows = [
    ['Audio output', 'playback-audio-output', '—'],
    ['Audio queued', 'playback-audio-buffer', '—'],
    ['Audio decode-ahead', 'playback-audio-decode', '—'],
    ['Video queued', 'playback-video-buffer', '—'],
    ['Late audio', 'playback-audio-late', '0'],
    ['Underruns', 'playback-audio-underruns', '0'],
  ];
  for (const [label, id, initial] of rows) {
    if ($(id)) continue;
    const row = document.createElement('div');
    const term = document.createElement('dt');
    const value = document.createElement('dd');
    term.textContent = label;
    value.id = id;
    value.textContent = initial;
    row.append(term, value);
    list.append(row);
  }
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

class PlayerController {
  constructor() {
    ensurePlaybackInspectorMetrics();
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
    this.audio = new AudioSink();
    this.playbackBuffer = null;
    this.log = new EventLog($('event-log'));
    this.frameId = null;
    this.videoStreamId = null;
    this.audioStreamId = null;
    this.audioRate = null;
    this.audioChannels = null;
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
    this.playbackBuffer = null;
    setText('metric-index', '—');
    setText('metric-ranges', '0');
    setText('metric-bytes', '0 B');
    setText('metric-frames', '0');
    setText('playback-audio-output', '—');
    setText('playback-audio-buffer', '—');
    setText('playback-audio-decode', '—');
    setText('playback-video-buffer', '—');
    setText('playback-audio-late', '0');
    setText('playback-audio-underruns', '0');
  }

  updatePlaybackMetrics(queue, at) {
    if (!queue) return;
    const metrics = queue.metrics(at);
    const audio = this.audio.metrics();
    setText('playback-audio-output', this.audioStreamId === null ? '—' : audio.mode);
    setText('playback-audio-buffer', this.audioStreamId === null ? '—' : `${metrics.audioQueuedSeconds.toFixed(2)} s · ${metrics.audioPackets} packets`);
    setText('playback-audio-decode', this.audioStreamId === null ? '—' : `${metrics.audioDecodedAheadSeconds.toFixed(2)} s`);
    setText('playback-video-buffer', this.videoStreamId === null ? '—' : `${metrics.videoFrames} frames · ${metrics.videoQueuedSeconds.toFixed(2)} s`);
    setText('playback-audio-late', this.audioStreamId === null ? '—' : `${metrics.lateAudioPackets} packets · ${audio.workletLateFrames} worklet frames`);
    setText('playback-audio-underruns', this.audioStreamId === null ? '—' : String(metrics.audioUnderruns));
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
    this.audioRate = null;
    this.audioChannels = null;
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
      setText('engine-wasm-active', decoder.artifact === 'simd128' ? 'SIMD128' : 'scalar');
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
      this.audioRate = audio?.param0 ?? null;
      this.audioChannels = audio?.param1 ?? null;
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
      setText('engine-wasm-active', 'unavailable');
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
    const hasAudio = this.audioStreamId !== null;
    const hasVideo = this.videoStreamId !== null;
    // Each playback generation owns its buffer. An aborted producer may finish later, but it can
    // only mark its own queue complete and therefore cannot poison a new seek/replay generation.
    const queue = new PlaybackBuffer({prebufferSeconds: 0.5, maxAudioSeconds: 4, maxVideoSeconds: 4});
    queue.reset(start);
    this.playbackBuffer = queue;

    // Wake WebAudio synchronously from the user gesture before any decode/prebuffer waits.
    if (hasAudio) {
      await this.audio.ensure(this.audioRate, this.audioChannels);
      if (controller.signal.aborted || this.playAbort !== controller) return;
    }

    let clockStarted = false;
    let contextStart = 0;
    let wallStart = 0;
    const mediaNow = () => {
      if (!clockStarted) return start;
      const elapsed = hasAudio
        ? this.audio.context.currentTime - contextStart
        : (performance.now() - wallStart) / 1000;
      return Math.min(this.duration, start + Math.max(0, elapsed));
    };
    const waitForQueueSpace = async kind => {
      // Before the clock starts, allow the first independently-decodable epoch to establish a
      // usable A/V prebuffer. Some decoder drains expose all audio packets before video; applying
      // the steady-state audio high-water mark there could prevent the first video frame from ever
      // reaching the queue. Once playback starts, backpressure is per stream so one full queue does
      // not prevent the decoder from draining the other stream.
      if (!clockStarted) return;
      const capacity = kind === 'audio'
        ? {hasAudio: true, hasVideo: false, at: mediaNow()}
        : {hasAudio: false, hasVideo: true, at: mediaNow()};
      while (queue.atCapacity(capacity)) {
        if (controller.signal.aborted) throw controller.signal.reason;
        capacity.at = mediaNow();
        await sleep(PLAYBACK_POLL_MS);
      }
    };

    let producerError = null;
    const firstEpoch = Math.max(0, this.index.epochs.findLastIndex(epoch => seconds(epoch.pts) <= start));
    const producer = (async () => {
      try {
        for (let i = firstEpoch; i < this.index.epochs.length; i++) {
          if (controller.signal.aborted) throw controller.signal.reason;
          const epoch = this.index.epochs[i];
          setText('playback-epoch', `${i + 1} / ${this.index.epochs.length} · id ${epoch.id}`);
          await this.decoder.decodeEpoch(this.source, epoch, {
            signal: controller.signal,
            onRange: range => this.rangeEvent(range),
            onEvent: event => this.epochEvent(event),
            onAudio: async packet => {
              if (packet.streamId !== this.audioStreamId) return;
              await waitForQueueSpace('audio');
              queue.pushAudio(packet);
            },
            onVideo: async frame => {
              if (frame.streamId !== this.videoStreamId) return;
              await waitForQueueSpace('video');
              queue.pushVideo(frame);
            },
          });
        }
      } catch (error) {
        producerError = error;
      } finally {
        queue.markDecodeFinished();
      }
    })();

    try {
      this.setState('buffering', 'Buffering decoded media…');
      while (!queue.readyToStart({hasAudio, hasVideo, at: start})) {
        if (controller.signal.aborted) throw controller.signal.reason;
        if (producerError) throw producerError;
        this.updatePlaybackMetrics(queue, start);
        await sleep(PLAYBACK_POLL_MS);
      }
      if (producerError) throw producerError;

      let outputMode = 'video clock';
      if (hasAudio) {
        contextStart = this.audio.context.currentTime + PLAYBACK_START_DELAY;
        this.playStartAudioContext = contextStart;
        outputMode = this.audio.begin(this.audioChannels);
      } else {
        wallStart = performance.now() + PLAYBACK_START_DELAY * 1000;
        this.playStartWall = wallStart;
      }
      this.playStartMedia = start;
      clockStarted = true;
      this.updatePlaybackMetrics(queue, start);
      this.setState('playing', `Buffered ${queue.bufferedAudioSeconds(start).toFixed(2)}s audio · ${queue.video.length} video frames`);
      this.log.add(`playback start prebuffer audio=${queue.bufferedAudioSeconds(start).toFixed(3)}s video=${queue.video.length} output=${outputMode}`);

      while (!controller.signal.aborted) {
        if (producerError) throw producerError;
        const audioFault = hasAudio ? this.audio.takeFault() : null;
        if (audioFault) throw audioFault;
        const now = mediaNow();

        if (hasAudio) {
          const scheduleUntil = Math.min(this.duration, now + AUDIO_SCHEDULE_AHEAD);
          for (const item of queue.takeAudioThrough(scheduleUntil)) {
            this.audio.schedule(item.packet, start, contextStart);
          }
          if (queue.noteAudioPlaybackPosition(now)) {
            const metrics = queue.metrics(now);
            throw new AudioUnderrunError(`decoded audio ran dry at ${formatTime(now)} (frontier ${formatTime(metrics.audioFrontier)})`);
          }
        }

        if (hasVideo) {
          const due = queue.takeVideoForTime(now + 0.001);
          if (due) {
            this.renderer.render(due.frame);
            this.frameId = due.frame.id;
            this.framesRendered++;
            setText('metric-frames', String(this.framesRendered));
            setText('playback-frame', `stream ${due.frame.streamId} · frame ${due.frame.id}`);
          }
        }

        this.updateTime(now);
        this.updatePlaybackMetrics(queue, now);
        if (now >= this.duration) break;
        await sleep(PLAYBACK_POLL_MS);
      }

      if (!controller.signal.aborted) {
        await producer;
        if (producerError) throw producerError;
        const audioFault = hasAudio ? this.audio.takeFault() : null;
        if (audioFault) throw audioFault;
        this.updateTime(this.duration);
        this.updatePlaybackMetrics(queue, this.duration);
        const metrics = queue.metrics(this.duration);
        const audioMetrics = this.audio.metrics();
        this.audio.stopAll();
        this.log.add(`playback complete underruns=${metrics.audioUnderruns} late=${metrics.lateAudioPackets} workletLateFrames=${audioMetrics.workletLateFrames}`);
        this.setState('ended', `${this.framesRendered} video frame${this.framesRendered === 1 ? '' : 's'} presented · ${metrics.audioUnderruns} audio underruns`);
      }
    } catch (error) {
      if (error?.name === 'AbortError' || error instanceof StaleDecodeGenerationError) return;
      controller.abort(error);
      this.audio.stopAll();
      const metrics = queue.metrics(mediaNow());
      this.updatePlaybackMetrics(queue, mediaNow());
      this.log.add(`playback error: ${error.message ?? error} · underruns=${metrics.audioUnderruns} late=${metrics.lateAudioPackets}`);
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
    this.updatePlaybackMetrics(this.playbackBuffer, at);
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

let mediaObjectUrl = null;
let pendingMediaFile = null;
let mediaImportWorker = null;
let mediaImportSequence = 0;
let activeMediaImport = null;

function queueMediaFile(file) {
  if (file && /\.avl$/i.test(file.name)) {
    pendingMediaFile = null;
    $('media-convert').disabled = true;
    setText('media-file-name', `${file.name} · ${formatBytes(file.size)}`);
    setText('media-status', 'Avelune file detected — loading directly; no conversion needed.');
    $('media-download').hidden = true;
    player.load(new BlobRangeSource(file, file.name)).catch(error => {
      setText('media-status', error.message ?? String(error));
      reportUiError(error);
    });
    return;
  }
  pendingMediaFile = file ?? null;
  const button = $('media-convert');
  button.disabled = !pendingMediaFile;
  setText('media-file-name', pendingMediaFile ? `${pendingMediaFile.name} · ${formatBytes(pendingMediaFile.size)}` : 'No file selected.');
  setText('media-status', pendingMediaFile ? 'Ready to convert. Adjust compression/import settings or start.' : 'Choose or drop a media file or .avl.');
  const download = $('media-download');
  download.hidden = true;
}

function mediaOptions() {
  const videoQ = Number($('media-video-q').value);
  const audioQ = Number($('media-audio-q').value);
  const audioChannels = Number($('media-audio-channels').value);
  const audioRate = Number($('media-audio-rate').value);
  const epochSeconds = Number($('media-epoch').value);
  const startSeconds = Number($('media-start').value);
  const durationSeconds = Number($('media-duration').value);
  if (!Number.isInteger(videoQ) || videoQ < 1 || videoQ > 65535) throw Error('video Q step must be in 1..65535');
  if (!Number.isInteger(audioQ) || audioQ < 1 || audioQ > 65535) throw Error('audio Q step must be in 1..65535');
  if (![0, 1, 2].includes(audioChannels)) throw Error('audio channels must be disabled, mono, or stereo');
  if (![32000, 44100, 48000].includes(audioRate)) throw Error('unsupported browser audio sample rate');
  if (!Number.isFinite(epochSeconds) || epochSeconds <= 0 || epochSeconds > 60) throw Error('epoch length must be in (0, 60] seconds');
  if (!Number.isFinite(startSeconds) || startSeconds < 0) throw Error('start time must be non-negative');
  if (!Number.isFinite(durationSeconds) || durationSeconds < 0) throw Error('duration must be non-negative');
  const preset = $('media-preset').value;
  if (!['fast', 'balanced', 'quality'].includes(preset)) throw Error(`unknown encoder preset: ${preset}`);
  return {
    videoQ,
    audioQ,
    audioChannels,
    audioRate,
    epochSeconds,
    startSeconds,
    durationSeconds,
    preset,
    resolution: $('media-resolution').value,
    fps: $('media-fps').value,
    artifact: $('wasm-artifact').value,
  };
}

function getMediaImportWorker() {
  if (!mediaImportWorker) {
    mediaImportWorker = new Worker(new URL('./media-import-worker.js', import.meta.url), {type: 'module'});
  }
  return mediaImportWorker;
}

function cancelMediaConversion() {
  if (!activeMediaImport) return;
  const {reject} = activeMediaImport;
  activeMediaImport = null;
  mediaImportSequence++;
  mediaImportWorker?.terminate();
  mediaImportWorker = null;
  reject(new DOMException('Conversion cancelled', 'AbortError'));
}

async function convertMediaFile() {
  const file = pendingMediaFile;
  if (!file) throw Error('choose or drop a media file first');
  const button = $('media-convert');
  const status = $('media-status');
  const progress = $('media-progress');
  const logNode = $('media-log');
  const logLines = [];
  if (activeMediaImport) throw Error('a media conversion is already running');
  button.disabled = true;
  $('media-cancel').disabled = false;
  progress.hidden = false;
  progress.removeAttribute('value');
  status.textContent = 'Preparing browser media importer…';
  logNode.textContent = '';
  const worker = getMediaImportWorker();
  const requestId = ++mediaImportSequence;
  try {
    const options = mediaOptions();
    const result = await new Promise((resolve, reject) => {
      const cleanup = () => {
        worker.removeEventListener('message', onMessage);
        worker.removeEventListener('error', onError);
        if (activeMediaImport?.requestId === requestId) activeMediaImport = null;
      };
      const settle = (fn, value) => { cleanup(); fn(value); };
      const onMessage = event => {
        const data = event.data ?? {};
        if (data.requestId !== requestId) return;
        if (data.type === 'progress') {
          if (data.detail) status.textContent = data.detail;
          if (Number.isFinite(data.done) && Number.isFinite(data.total) && data.total > 0) {
            progress.max = data.total;
            progress.value = data.done;
          } else if (Number.isFinite(data.progress) && data.progress >= 0) {
            progress.max = 1;
            progress.value = Math.min(1, data.progress);
          } else {
            progress.removeAttribute('value');
          }
        } else if (data.type === 'log') {
          logLines.push(data.message);
          if (logLines.length > 160) logLines.splice(0, logLines.length - 160);
          logNode.textContent = logLines.join('\n');
          logNode.scrollTop = logNode.scrollHeight;
        } else if (data.type === 'done') settle(resolve, data);
        else if (data.type === 'error') settle(reject, Error(data.message));
      };
      const onError = event => settle(reject, Error(event.message || 'media import worker failed'));
      activeMediaImport = {requestId, reject: error => settle(reject, error)};
      worker.addEventListener('message', onMessage);
      worker.addEventListener('error', onError);
      // File/Blob is structured-cloned by reference. The worker mounts it through Emscripten
      // WORKERFS, so a multi-gigabyte source is not copied into an ArrayBuffer up front.
      worker.postMessage({type: 'convert', requestId, file, name: file.name, options});
    });
    const blob = result.file;
    if (!(blob instanceof Blob)) throw Error('media importer did not return an Avelune file');
    const stem = file.name.replace(/\.[^.]+$/, '') || 'converted';
    if (mediaObjectUrl) URL.revokeObjectURL(mediaObjectUrl);
    mediaObjectUrl = URL.createObjectURL(blob);
    const download = $('media-download');
    download.href = mediaObjectUrl;
    download.download = `${stem}.avl`;
    download.textContent = `Save ${stem}.avl`;
    download.hidden = false;
    progress.max = 1;
    progress.value = 1;
    const audio = result.audioChannels ? ` · ${result.audioChannels}ch/${result.audioRate} Hz audio` : ' · video-only';
    const storage = result.spool === 'opfs' ? ' · streamed via browser storage' : ' · compressed chunks buffered in memory';
    status.textContent = `${result.width}×${result.height} · ${result.frames} frames${audio} → ${formatBytes(blob.size)} · ${result.artifact}${storage}`;
    await player.load(new BlobRangeSource(blob, `${stem}.avl`));
  } finally {
    button.disabled = !pendingMediaFile;
    $('media-cancel').disabled = true;
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
$('media-file').addEventListener('change', event => queueMediaFile(event.target.files?.[0]));
$('media-convert').addEventListener('click', () => convertMediaFile().catch(error => {
  $('media-status').textContent = error?.name === 'AbortError' ? 'Conversion cancelled.' : (error.message ?? String(error));
  $('media-progress').hidden = true;
  reportUiError(error);
}));
$('media-cancel').addEventListener('click', cancelMediaConversion);
$('play').addEventListener('click', () => {
  if (player.state === 'playing') player.pause(); else player.play().catch(reportUiError);
});
$('seek').addEventListener('input', () => {
  if (player.state !== 'playing') player.updateTime(Number($('seek').value));
});
$('seek').addEventListener('change', () => player.seek(Number($('seek').value)).catch(reportUiError));
$('volume').addEventListener('input', () => player.audio.setVolume(Number($('volume').value)));
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
  if (!file) return;
  if (/\.avl$/i.test(file.name)) {
    player.load(new BlobRangeSource(file, file.name)).catch(reportUiError);
  } else {
    queueMediaFile(file);
    $('media-import').scrollIntoView({behavior: 'smooth', block: 'nearest'});
  }
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
  if (mediaObjectUrl) URL.revokeObjectURL(mediaObjectUrl);
  mediaImportWorker?.terminate();
  mediaImportWorker = null;
});

updateSampleDescription();
// Loading the index does not autoplay media or create an AudioContext, so the
// demo can be useful immediately without violating browser activation rules.
player.load(selectedSampleSource()).catch(reportUiError);
