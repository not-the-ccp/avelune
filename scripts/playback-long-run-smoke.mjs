import {PcmTimeline} from '../web/player/audio-timeline.js';
import {PlaybackBuffer} from '../web/player/playback-buffer.js';

// Simulate thirty minutes of playback without waiting thirty wall-clock minutes. The playhead,
// decoder producer, scheduler, and worklet timeline advance in 100 ms quanta so queue growth is
// measured under the same bounded-ahead rules used by the browser player.
const DURATION_SECONDS = 30 * 60;
const STEP_SECONDS = 0.1;
const STEP_US = 100_000;
const AUDIO_RATE = 48_000;
const CHANNELS = 2;
const AUDIO_FRAMES_PER_STEP = AUDIO_RATE * STEP_SECONDS;
const TOTAL_STEPS = Math.round(DURATION_SECONDS / STEP_SECONDS);
const SCHEDULE_AHEAD_SECONDS = 0.75;

const pcm = new Int16Array(AUDIO_FRAMES_PER_STEP * CHANNELS);
const floats = new Float32Array(AUDIO_FRAMES_PER_STEP * CHANNELS);
const q = new PlaybackBuffer({prebufferSeconds: 0.5, maxAudioSeconds: 4, maxVideoSeconds: 4});
const timeline = new PcmTimeline(CHANNELS);
q.reset(0);
timeline.clear(0);

let nextAudio = 0;
let nextVideo = 0;
let maxAudioPackets = 0;
let maxVideoFrames = 0;
let maxTimelineChunks = 0;
let maxTimelineFrames = 0;
let renderedVideo = 0;

function fillDecodeAhead(now) {
  while (nextAudio < TOTAL_STEPS && !q.atCapacity({hasAudio: true, at: now})) {
    q.pushAudio({
      pts: nextAudio * STEP_US,
      rate: AUDIO_RATE,
      channels: CHANNELS,
      pcm,
    });
    nextAudio++;
  }
  while (nextVideo < TOTAL_STEPS && !q.atCapacity({hasVideo: true, at: now})) {
    q.pushVideo({pts: nextVideo * STEP_US, id: nextVideo});
    nextVideo++;
  }
  maxAudioPackets = Math.max(maxAudioPackets, q.audio.length);
  maxVideoFrames = Math.max(maxVideoFrames, q.video.length);
}

fillDecodeAhead(0);
if (!q.readyToStart({hasAudio: true, hasVideo: true, at: 0})) throw Error('long-run queue did not satisfy initial prebuffer');

const output = [new Float32Array(AUDIO_FRAMES_PER_STEP), new Float32Array(AUDIO_FRAMES_PER_STEP)];
for (let step = 0; step < TOTAL_STEPS; step++) {
  const now = step * STEP_SECONDS;
  fillDecodeAhead(now);

  for (const item of q.takeAudioThrough(Math.min(DURATION_SECONDS, now + SCHEDULE_AHEAD_SECONDS))) {
    const startFrame = Math.round(Number(item.packet.pts) * AUDIO_RATE / 1_000_000);
    const status = timeline.push(startFrame, floats);
    if (status.lateFrames) throw Error(`long-run timeline received ${status.lateFrames} late frames at ${now}s`);
  }
  maxTimelineChunks = Math.max(maxTimelineChunks, timeline.chunks.length);
  maxTimelineFrames = Math.max(maxTimelineFrames, timeline.queuedFrames());

  const startFrame = step * AUDIO_FRAMES_PER_STEP;
  const result = timeline.render(output, startFrame, AUDIO_FRAMES_PER_STEP);
  if (result.silentFrames) throw Error(`long-run timeline inserted ${result.silentFrames} silent frames at ${now}s`);

  const due = q.takeVideoForTime(now + 1e-6);
  if (due) renderedVideo++;
  if (q.noteAudioPlaybackPosition(Math.min(DURATION_SECONDS, now + STEP_SECONDS))) {
    throw Error(`long-run decoded audio underrun at ${now}s`);
  }
}

q.markDecodeFinished();
if (nextAudio !== TOTAL_STEPS || nextVideo !== TOTAL_STEPS) {
  throw Error(`long-run producer stopped early audio=${nextAudio}/${TOTAL_STEPS} video=${nextVideo}/${TOTAL_STEPS}`);
}
if (q.audio.length !== 0) throw Error(`long-run playback retained ${q.audio.length} scheduled audio packets`);
if (q.video.length !== 0) throw Error(`long-run playback retained ${q.video.length} video frames`);
if (timeline.chunks.length !== 0 || timeline.queuedFrames() !== 0) {
  throw Error(`long-run worklet timeline retained ${timeline.chunks.length} chunks / ${timeline.queuedFrames()} frames`);
}
if (renderedVideo !== TOTAL_STEPS) throw Error(`long-run rendered ${renderedVideo}/${TOTAL_STEPS} video frames`);

const metrics = q.metrics(DURATION_SECONDS);
if (metrics.audioUnderruns !== 0 || metrics.lateAudioPackets !== 0) {
  throw Error(`long-run playback reported underruns=${metrics.audioUnderruns} late=${metrics.lateAudioPackets}`);
}

// One producer push may cross a high-water mark by one 100 ms packet/frame. These bounds make a
// future accidental unbounded queue obvious without relying on noisy process-RSS measurements.
if (maxAudioPackets > 42) throw Error(`long-run audio queue grew to ${maxAudioPackets} packets`);
if (maxVideoFrames > 42) throw Error(`long-run video queue grew to ${maxVideoFrames} frames`);
if (maxTimelineChunks > 10) throw Error(`long-run worklet queue grew to ${maxTimelineChunks} chunks`);
if (maxTimelineFrames > Math.ceil((SCHEDULE_AHEAD_SECONDS + STEP_SECONDS) * AUDIO_RATE)) {
  throw Error(`long-run worklet queue grew to ${maxTimelineFrames} frames`);
}

console.log(JSON.stringify({
  virtualPlaybackSeconds: DURATION_SECONDS,
  steps: TOTAL_STEPS,
  renderedVideo,
  maxAudioPackets,
  maxVideoFrames,
  maxTimelineChunks,
  maxTimelineFrames,
  audioUnderruns: metrics.audioUnderruns,
  lateAudioPackets: metrics.lateAudioPackets,
}));
