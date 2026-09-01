import {createAveluneAvEncoder} from './avelune-loader.js';
import {parseY4m} from './y4m.js';

const MAX_INPUT_BYTES = 512 * 1024 * 1024;
let ffmpegPromise = null;
let activeLog = () => {};
let activeProgress = () => {};

function safeStem(name) {
  return (name || 'input').replace(/[^a-zA-Z0-9._-]+/g, '_').slice(-120);
}

async function loadFfmpeg() {
  if (!ffmpegPromise) {
    ffmpegPromise = (async () => {
      activeProgress({phase: 'ffmpeg-load', detail: 'Loading embedded FFmpeg (~31 MiB)…'});
      const {default: createFFmpegCore} = await import('./ffmpeg/ffmpeg-core.js');
      const core = await createFFmpegCore();
      core.setLogger(({type, message}) => activeLog(`${type}: ${message}`));
      core.setProgress(({progress, time}) => activeProgress({phase: 'ffmpeg', progress, time}));
      return core;
    })().catch(error => { ffmpegPromise = null; throw error; });
  }
  return ffmpegPromise;
}

function unlinkQuiet(core, path) {
  try { core.FS.unlink(path); } catch {}
}

function scaleFilter(mode) {
  if (!mode || mode === 'source') return 'scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=lanczos';
  const match = /^(\d+)x(\d+)$/.exec(mode);
  if (!match) throw Error(`invalid output resolution: ${mode}`);
  return `scale=${match[1]}:${match[2]}:force_original_aspect_ratio=decrease:flags=lanczos,pad=${match[1]}:${match[2]}:(ow-iw)/2:(oh-ih)/2:color=black`;
}


function trimArgs(options) {
  const args = [];
  if (Number(options.startSeconds) > 0) args.push('-ss', String(options.startSeconds));
  if (Number(options.durationSeconds) > 0) args.push('-t', String(options.durationSeconds));
  return args;
}

function normalizeOptions(options) {
  if (!options || typeof options !== 'object') throw Error('missing media conversion options');
  return {
    ...options,
    // Trimming is optional. Treat omitted/blank controls as "from the start / full duration"
    // so callers outside player.js do not need to manufacture zeroes.
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

function pcm16(bytes) {
  if (!bytes?.length) return new Int16Array();
  if (bytes.length % 2) throw Error('FFmpeg produced an odd-length s16le audio stream');
  const copy = bytes.slice();
  return new Int16Array(copy.buffer, copy.byteOffset, copy.byteLength / 2);
}

async function convert({buffer, name, options}) {
  if (!(buffer instanceof ArrayBuffer)) throw Error('media importer requires an ArrayBuffer');
  if (buffer.byteLength > MAX_INPUT_BYTES) throw Error('browser importer is limited to 512 MiB input files');
  options = normalizeOptions(options);
  validateOptions(options);
  const core = await loadFfmpeg();
  const stem = safeStem(name);
  const suffix = /\.[a-zA-Z0-9]{1,8}$/.exec(stem)?.[0] ?? '.bin';
  const input = `/input${suffix}`;
  const y4mPath = '/video.y4m';
  const pcmPath = '/audio.s16le';
  unlinkQuiet(core, input); unlinkQuiet(core, y4mPath); unlinkQuiet(core, pcmPath);
  core.FS.writeFile(input, new Uint8Array(buffer));

  try {
    // Audio is intentionally extracted first. PCM is relatively small, and removing its MEMFS
    // file before decoding YUV materially reduces peak browser memory for normal A/V inputs.
    let audio = new Int16Array();
    if (options.audioChannels > 0) {
      activeProgress({phase: 'audio-decode', detail: 'FFmpeg: decoding audio to PCM…'});
      try {
        const audioArgs = ['-i', input, ...trimArgs(options), '-map', '0:a:0', '-vn', '-sn', '-dn',
          '-ac', String(options.audioChannels), '-ar', String(options.audioRate), '-f', 's16le', pcmPath];
        run(core, audioArgs, 'audio conversion');
        audio = pcm16(core.FS.readFile(pcmPath));
      } catch (error) {
        activeLog(`audio: ${error.message}; continuing video-only`);
        audio = new Int16Array();
      } finally {
        unlinkQuiet(core, pcmPath);
      }
    }

    activeProgress({phase: 'video-decode', detail: 'FFmpeg: decoding video to YUV420…'});
    const videoArgs = ['-i', input, ...trimArgs(options), '-map', '0:v:0', '-an', '-sn', '-dn',
      '-vf', scaleFilter(options.resolution), '-pix_fmt', 'yuv420p'];
    if (options.fps && options.fps !== 'source') videoArgs.push('-r', String(options.fps));
    videoArgs.push('-f', 'yuv4mpegpipe', y4mPath);
    run(core, videoArgs, 'video conversion');
    const y4mBytes = core.FS.readFile(y4mPath);
    unlinkQuiet(core, y4mPath);
    const y4m = parseY4m(y4mBytes);

    const fps = y4m.fpsN / y4m.fpsD;
    const epochFrames = Math.max(1, Math.round(fps * options.epochSeconds));
    const hasAudio = audio.length > 0;
    const audioSampleCount = audio.length;
    activeProgress({phase: 'avelune', detail: `Avelune (${options.artifact === 'auto' ? 'SIMD preferred' : options.artifact}): encoding ${y4m.frames.length} frames${hasAudio ? ' + audio' : ''}…`, done: 0, total: y4m.frames.length});
    const encoder = await createAveluneAvEncoder({
      width: y4m.width,
      height: y4m.height,
      fpsN: y4m.fpsN,
      fpsD: y4m.fpsD,
      videoQ: options.videoQ,
      audioQ: options.audioQ,
      preset: options.preset,
      epochFrames,
      chromaLocation: y4m.chromaLocation,
      fullRange: y4m.fullRange,
      audioRate: hasAudio ? options.audioRate : 0,
      audioChannels: hasAudio ? options.audioChannels : 0,
    }, {artifact: options.artifact});
    try {
      if (hasAudio) {
        const chunkSamples = Math.max(options.audioChannels, 1_048_576 - (1_048_576 % options.audioChannels));
        for (let offset = 0; offset < audio.length; offset += chunkSamples) {
          encoder.pushAudio(audio.subarray(offset, Math.min(audio.length, offset + chunkSamples)));
        }
        audio = new Int16Array();
      }
      let lastReport = performance.now();
      for (let i = 0; i < y4m.frames.length; i++) {
        encoder.pushFrame(y4m.frames[i]);
        const now = performance.now();
        if (now - lastReport >= 100 || i + 1 === y4m.frames.length) {
          lastReport = now;
          activeProgress({phase: 'avelune', done: i + 1, total: y4m.frames.length});
        }
      }
      const encoded = encoder.finish();
      return {
        encoded,
        artifact: encoder.artifact,
        frames: y4m.frames.length,
        width: y4m.width,
        height: y4m.height,
        fpsN: y4m.fpsN,
        fpsD: y4m.fpsD,
        audioSamples: audioSampleCount,
        audioRate: hasAudio ? options.audioRate : 0,
        audioChannels: hasAudio ? options.audioChannels : 0,
      };
    } finally {
      encoder.destroy();
    }
  } finally {
    unlinkQuiet(core, input); unlinkQuiet(core, y4mPath); unlinkQuiet(core, pcmPath);
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
    const result = await convert(event.data);
    self.postMessage({type: 'done', requestId, ...result, encoded: result.encoded.buffer}, [result.encoded.buffer]);
  } catch (error) {
    self.postMessage({type: 'error', requestId, message: error?.message ?? String(error)});
  } finally {
    conversionRunning = false;
    activeLog = () => {};
    activeProgress = () => {};
  }
});
