import {createAveluneStreamingAvEncoder} from './avelune-loader.js';

let ffmpegPromise = null;
let activeLog = () => {};
let activeProgress = () => {};
let deviceMinor = 1;
let opfsPrepared = false;

function safeStem(name) {
  return (name || 'input').replace(/[^a-zA-Z0-9._-]+/g, '_').slice(-120);
}

function coreLogger(core, capture = null) {
  core.setLogger(({type, message}) => {
    if (capture && type === 'stdout') capture.push(message);
    activeLog(`${type}: ${message}`);
  });
}

async function loadFfmpeg() {
  if (!ffmpegPromise) {
    ffmpegPromise = (async () => {
      activeProgress({phase: 'ffmpeg-load', detail: 'Loading embedded FFmpeg (~31 MiB)…'});
      const {default: createFFmpegCore} = await import('./ffmpeg/ffmpeg-core.js');
      const core = await createFFmpegCore();
      coreLogger(core);
      core.setProgress(({progress, time}) => activeProgress({phase: 'ffmpeg', progress, time}));
      return core;
    })().catch(error => { ffmpegPromise = null; throw error; });
  }
  return ffmpegPromise;
}

function normalizeOptions(options) {
  if (!options || typeof options !== 'object') throw Error('missing media conversion options');
  return {
    ...options,
    startSeconds: options.startSeconds == null || options.startSeconds === '' ? 0 : Number(options.startSeconds),
    durationSeconds: options.durationSeconds == null || options.durationSeconds === '' ? 0 : Number(options.durationSeconds),
  };
}

function validateOptions(options) {
  for (const [label, value] of [['video Q', options.videoQ], ['audio Q', options.audioQ]]) {
    if (!Number.isInteger(value) || value < 1 || value > 65535) throw Error(`${label} must be in 1..65535`);
  }
  if (![0, 1, 2].includes(options.audioChannels)) throw Error('audio channels must be 0, 1, or 2');
  if (![32000, 44100, 48000].includes(options.audioRate)) throw Error('unsupported audio sample rate');
  if (!['fast', 'balanced', 'quality'].includes(options.preset)) throw Error('invalid encoder preset');
  if (!Number.isFinite(options.epochSeconds) || options.epochSeconds <= 0 || options.epochSeconds > 60) throw Error('invalid epoch length');
  if (!Number.isFinite(options.startSeconds) || options.startSeconds < 0) throw Error('invalid start time');
  if (!Number.isFinite(options.durationSeconds) || options.durationSeconds < 0) throw Error('invalid duration');
}

function run(core, args, label) {
  const code = core.exec(...args);
  core.reset();
  if (code !== 0) throw Error(`${label} failed in embedded FFmpeg (exit ${code})`);
}

function parseY4mHeader(header) {
  if (!header.startsWith('YUV4MPEG2 ')) throw Error(`FFmpeg returned an invalid Y4M header: ${header.slice(0, 160)}`);
  let width, height, fpsN = 30, fpsD = 1, chroma = '420', fullRange = false;
  for (const token of header.trim().split(/\s+/).slice(1)) {
    if (token.startsWith('W')) width = Number(token.slice(1));
    else if (token.startsWith('H')) height = Number(token.slice(1));
    else if (token.startsWith('F')) {
      const [n, d = '1'] = token.slice(1).split(':');
      fpsN = Number(n); fpsD = Number(d);
    } else if (token.startsWith('C')) chroma = token.slice(1);
    else if (token.toUpperCase() === 'XCOLORRANGE=FULL') fullRange = true;
  }
  for (const [label, value] of Object.entries({width, height, fpsN, fpsD})) {
    if (!Number.isSafeInteger(value) || value <= 0) throw Error(`invalid Y4M ${label}`);
  }
  if (width % 2 || height % 2) throw Error('FFmpeg produced odd YUV420 dimensions');
  if (!['420', '420jpeg', '420mpeg2'].includes(chroma)) throw Error(`FFmpeg probe produced unsupported Y4M chroma C${chroma}`);
  const gcd = (a, b) => { while (b) [a, b] = [b, a % b]; return a; };
  const g = gcd(fpsN, fpsD); fpsN /= g; fpsD /= g;
  if (fpsN > 0xffff || fpsD > 0xffff) {
    const rate = fpsN / fpsD;
    const common = [
      [24000, 1001], [24, 1], [25, 1], [30000, 1001], [30, 1],
      [50, 1], [60000, 1001], [60, 1], [120, 1],
    ].sort((a, b) => Math.abs(a[0] / a[1] - rate) - Math.abs(b[0] / b[1] - rate))[0];
    [fpsN, fpsD] = common;
  }
  return {
    width, height, fps: {n: fpsN, d: fpsD},
    chromaLocation: chroma === '420mpeg2' ? 1 : chroma === '420jpeg' ? 2 : 0,
    fullRange,
  };
}

class Y4mHeaderSink {
  constructor() { this.bytes = []; this.header = null; this.total = 0; }
  write(bytes) {
    this.total += bytes.length;
    if (this.header !== null) return;
    for (const byte of bytes) {
      if (byte === 10) {
        this.header = new TextDecoder().decode(new Uint8Array(this.bytes));
        this.bytes = [];
        return;
      }
      if (this.bytes.length >= 4096) throw Error('FFmpeg Y4M header exceeds 4 KiB');
      this.bytes.push(byte);
    }
  }
  finish() {
    if (this.header === null) throw Error('FFmpeg did not produce a Y4M header while probing the source');
    return parseY4mHeader(this.header);
  }
}

function scaleFilter(mode) {
  if (!mode || mode === 'source') return 'scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=lanczos';
  const match = /^(\d+)x(\d+)$/.exec(mode);
  if (!match) throw Error(`invalid output resolution: ${mode}`);
  return `scale=${match[1]}:${match[2]}:force_original_aspect_ratio=decrease:flags=lanczos,pad=${match[1]}:${match[2]}:(ow-iw)/2:(oh-ih)/2:color=black`;
}

function mkdirQuiet(fs, path) {
  try { fs.mkdir(path); } catch (error) {
    if (!/exist/i.test(error?.message ?? '')) throw error;
  }
}

function unlinkQuiet(fs, path) {
  try { fs.unlink(path); } catch {}
}

function mountInput(core, file) {
  if (!(file instanceof File)) throw Error('media importer requires a File object');
  const workerfs = core.FS.filesystems?.WORKERFS;
  if (!workerfs) throw Error('embedded FFmpeg was built without WORKERFS; streaming source access is unavailable');
  const root = '/avelune-input';
  mkdirQuiet(core.FS, root);
  try { core.FS.unmount(root); } catch {}
  core.FS.mount(workerfs, {files: [file]}, root);
  return {path: `${root}/${file.name}`, unmount: () => { try { core.FS.unmount(root); } catch {} }};
}

function createWriteDevice(core, path, onWrite) {
  unlinkQuiet(core.FS, path);
  const dev = core.FS.makedev(81, deviceMinor++ & 0xff);
  core.FS.registerDevice(dev, {
    write(_stream, buffer, offset, length) {
      const bytes = new Uint8Array(buffer.buffer, buffer.byteOffset + offset, length);
      onWrite(bytes);
      return length;
    },
  });
  core.FS.mkdev(path, 0o666, dev);
  return () => unlinkQuiet(core.FS, path);
}

class EpochSpool {
  constructor(writer = null) {
    this.writer = writer;
    this.parts = writer ? null : [];
    this.bytes = 0;
  }
  write(bytes) {
    if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
    if (!bytes.length) return;
    if (this.writer) this.writer.write(bytes, this.bytes);
    else this.parts.push(bytes.slice());
    this.bytes += bytes.length;
  }
}

async function createEpochSpool(requestId) {
  try {
    if (!navigator.storage?.getDirectory) throw Error('OPFS unavailable');
    const root = await navigator.storage.getDirectory();
    if (!opfsPrepared) {
      // A cancelled conversion terminates this worker while FFmpeg is executing, so its finally
      // block cannot run. Remove leftovers from previous page/worker lifetimes before creating the
      // first spool for this worker. Never do this between conversions in the same worker because
      // an older result may still be the player's active Blob source.
      for await (const [entryName] of root.entries()) {
        if (entryName.startsWith('avelune-import-')) {
          try { await root.removeEntry(entryName); } catch {}
        }
      }
      opfsPrepared = true;
    }
    const epochName = `avelune-import-${requestId}-${crypto.randomUUID()}-epochs.tmp`;
    const epochHandle = await root.getFileHandle(epochName, {create: true});
    const access = await epochHandle.createSyncAccessHandle();
    const spool = new EpochSpool({
      write(bytes, at) {
        let offset = 0;
        while (offset < bytes.length) {
          const written = access.write(bytes.subarray(offset), {at: at + offset});
          if (!Number.isInteger(written) || written <= 0) throw Error('OPFS failed to make progress while spooling encoded epochs');
          offset += written;
        }
      },
    });
    return {
      kind: 'opfs', spool,
      async finish(prefix) {
        access.flush();
        access.close();
        const epochFile = await epochHandle.getFile();
        // Blob/File concatenation keeps the large encoded epoch body file-backed. The only in-RAM
        // part here is the small front index, so final assembly does not copy the compressed movie
        // through JS or duplicate it into a second OPFS file.
        return {
          file: new File([prefix, epochFile], 'converted.avl', {type: 'application/octet-stream'}),
          storage: 'opfs',
        };
      },
      async abort() {
        try { access.close(); } catch {}
        try { await root.removeEntry(epochName); } catch {}
      },
    };
  } catch (error) {
    activeLog(`storage: OPFS spool unavailable (${error.message}); retaining compressed epoch chunks in memory`);
    const spool = new EpochSpool();
    return {
      kind: 'memory', spool,
      async finish(prefix) { return {file: new File([prefix, ...spool.parts], 'converted.avl', {type: 'application/octet-stream'}), storage: 'memory'}; },
      async abort() {},
    };
  }
}

class VideoFrameSink {
  constructor(frameBytes, encoder, spool, estimatedFrames) {
    this.buffer = new Uint8Array(frameBytes);
    this.used = 0;
    this.frames = 0;
    this.rawBytes = 0;
    this.encoder = encoder;
    this.spool = spool;
    this.estimatedFrames = estimatedFrames;
  }
  drainEpochs(chunks) { for (const chunk of chunks) this.spool.write(chunk); }
  write(bytes) {
    this.rawBytes += bytes.length;
    let offset = 0;
    while (offset < bytes.length) {
      const take = Math.min(this.buffer.length - this.used, bytes.length - offset);
      this.buffer.set(bytes.subarray(offset, offset + take), this.used);
      this.used += take; offset += take;
      if (this.used === this.buffer.length) {
        this.drainEpochs(this.encoder.pushFrame(this.buffer));
        this.frames++;
        this.used = 0;
        if (this.frames === 1 || this.frames % 8 === 0) {
          activeProgress({phase: 'avelune', detail: `Streaming decode + Avelune encode · ${this.frames}${this.estimatedFrames ? ` / ~${this.estimatedFrames}` : ''} frames`, done: this.frames, total: this.estimatedFrames || undefined});
        }
      }
    }
  }
  finish() {
    if (this.used) throw Error(`FFmpeg ended with a partial raw-video frame (${this.used}/${this.buffer.length} bytes)`);
  }
}

class AudioPcmSink {
  constructor(channels, encoder, spool) {
    this.channels = channels;
    this.encoder = encoder;
    this.spool = spool;
    this.carry = new Uint8Array();
    this.samples = 0;
    this.rawBytes = 0;
  }
  drainEpochs(chunks) { for (const chunk of chunks) this.spool.write(chunk); }
  write(bytes) {
    this.rawBytes += bytes.length;
    let input;
    if (this.carry.length) {
      input = new Uint8Array(this.carry.length + bytes.length);
      input.set(this.carry); input.set(bytes, this.carry.length);
    } else {
      input = bytes;
    }
    const alignment = this.channels * 2;
    const usable = input.length - input.length % alignment;
    if (usable) {
      const copy = input.slice(0, usable);
      const samples = new Int16Array(copy.buffer, copy.byteOffset, copy.byteLength / 2);
      this.drainEpochs(this.encoder.pushAudio(samples));
      this.samples += samples.length;
    }
    this.carry = input.slice(usable);
  }
  finish() {
    if (this.carry.length) throw Error(`FFmpeg ended with ${this.carry.length} unaligned PCM byte(s)`);
  }
}

async function convert({file, name, options, requestId}) {
  if (!(file instanceof File)) throw Error('media importer requires the original File so it can stream from browser storage');
  options = normalizeOptions(options);
  validateOptions(options);
  const core = await loadFfmpeg();
  const mounted = mountInput(core, file);
  const spoolState = await createEpochSpool(requestId);
  const videoPath = '/dev/avelune-video';
  const audioPath = '/dev/avelune-audio';
  let encoder;
  let cleanupVideo = () => {}, cleanupAudio = () => {};
  try {
    activeProgress({phase: 'probe', detail: 'Inspecting source through bounded FFmpeg streams…'});
    const probeVideoPath = '/dev/avelune-probe-video';
    const probeHeaderSink = new Y4mHeaderSink();
    const cleanupProbeVideo = createWriteDevice(core, probeVideoPath, bytes => probeHeaderSink.write(bytes));
    let probe;
    try {
      const probeArgs = ['-i', mounted.path, '-map', '0:v:0', '-an', '-sn', '-dn',
        '-vf', scaleFilter(options.resolution)];
      if (options.fps && options.fps !== 'source') probeArgs.push('-r', String(options.fps));
      probeArgs.push('-frames:v', '1', '-pix_fmt', 'yuv420p', '-f', 'yuv4mpegpipe', probeVideoPath);
      run(core, probeArgs, 'video metadata probe');
      probe = probeHeaderSink.finish();
    } finally {
      cleanupProbeVideo();
    }
    const geometry = {width: probe.width, height: probe.height};
    const fps = probe.fps;
    const epochFrames = Math.max(1, Math.round(fps.n / fps.d * options.epochSeconds));

    let audioChannels = 0;
    if (options.audioChannels > 0) {
      const probeAudioPath = '/dev/avelune-probe-audio';
      let probeAudioBytes = 0;
      const cleanupProbeAudio = createWriteDevice(core, probeAudioPath, bytes => { probeAudioBytes += bytes.length; });
      try {
        run(core, ['-i', mounted.path, '-map', '0:a:0?', '-vn', '-sn', '-dn', '-t', '0.05',
          '-ac', String(options.audioChannels), '-ar', String(options.audioRate), '-f', 's16le', probeAudioPath], 'audio metadata probe');
        if (probeAudioBytes > 0) audioChannels = options.audioChannels;
      } finally {
        cleanupProbeAudio();
      }
    }
    const selectedDuration = options.durationSeconds > 0 ? options.durationSeconds : 0;
    const estimatedFrames = selectedDuration > 0 ? Math.max(1, Math.round(selectedDuration * fps.n / fps.d)) : 0;

    encoder = await createAveluneStreamingAvEncoder({
      width: geometry.width, height: geometry.height, fpsN: fps.n, fpsD: fps.d,
      videoQ: options.videoQ, audioQ: options.audioQ, preset: options.preset, epochFrames,
      chromaLocation: probe.chromaLocation, fullRange: probe.fullRange,
      audioRate: audioChannels ? options.audioRate : 0, audioChannels,
    }, {artifact: options.artifact});

    const videoSink = new VideoFrameSink(geometry.width * geometry.height * 3 / 2, encoder, spoolState.spool, estimatedFrames);
    const audioSink = audioChannels ? new AudioPcmSink(audioChannels, encoder, spoolState.spool) : null;
    cleanupVideo = createWriteDevice(core, videoPath, bytes => videoSink.write(bytes));
    if (audioSink) cleanupAudio = createWriteDevice(core, audioPath, bytes => audioSink.write(bytes));

    const inputArgs = [];
    if (options.startSeconds > 0) inputArgs.push('-ss', String(options.startSeconds));
    inputArgs.push('-i', mounted.path);
    const durationArgs = options.durationSeconds > 0 ? ['-t', String(options.durationSeconds)] : [];
    const ffmpegArgs = [
      ...inputArgs,
      '-map', '0:v:0', '-an', '-sn', '-dn', ...durationArgs,
      '-vf', scaleFilter(options.resolution), '-r', `${fps.n}/${fps.d}`,
      '-pix_fmt', 'yuv420p', '-f', 'rawvideo', videoPath,
    ];
    if (audioSink) {
      ffmpegArgs.push(
        '-map', '0:a:0', '-vn', '-sn', '-dn', ...durationArgs,
        '-ac', String(audioChannels), '-ar', String(options.audioRate), '-f', 's16le', audioPath,
      );
    }
    activeProgress({phase: 'stream', detail: `Streaming ${safeStem(name)} through FFmpeg → ${encoder.artifact} Avelune…`});
    run(core, ffmpegArgs, 'streaming media conversion');
    videoSink.finish();
    audioSink?.finish();
    const final = encoder.finish();
    for (const chunk of final.epochs) spoolState.spool.write(chunk);
    const assembled = await spoolState.finish(final.prefix);
    return {
      file: assembled.file,
      artifact: encoder.artifact,
      frames: videoSink.frames,
      width: geometry.width,
      height: geometry.height,
      fpsN: fps.n,
      fpsD: fps.d,
      audioSamples: audioSink?.samples ?? 0,
      audioRate: audioChannels ? options.audioRate : 0,
      audioChannels,
      encodedBytes: assembled.file.size,
      spool: assembled.storage,
      sourceBytes: file.size,
      rawVideoBytes: videoSink.rawBytes,
      rawAudioBytes: audioSink?.rawBytes ?? 0,
    };
  } catch (error) {
    await spoolState.abort();
    throw error;
  } finally {
    cleanupVideo(); cleanupAudio(); mounted.unmount(); encoder?.destroy();
  }
}

let conversionRunning = false;

self.addEventListener('message', async event => {
  if (event.data?.type !== 'convert') return;
  const requestId = event.data.requestId;
  if (conversionRunning) {
    self.postMessage({type: 'error', requestId, message: 'a media conversion is already running'});
    return;
  }
  conversionRunning = true;
  activeLog = message => self.postMessage({type: 'log', requestId, message});
  activeProgress = data => self.postMessage({type: 'progress', requestId, ...data});
  try {
    const result = await convert({...event.data, requestId});
    self.postMessage({type: 'done', requestId, ...result});
  } catch (error) {
    self.postMessage({type: 'error', requestId, message: error?.message ?? String(error)});
  } finally {
    conversionRunning = false;
    activeLog = () => {};
    activeProgress = () => {};
  }
});
