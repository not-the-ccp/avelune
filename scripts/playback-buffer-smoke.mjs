import assert from 'node:assert/strict';
import {PlaybackBuffer} from '../web/player/playback-buffer.js';

function packet({pts, frames = 4800, rate = 48000, channels = 2}) {
  return {pts, rate, channels, pcm: new Int16Array(frames * channels)};
}

function frame(pts, id = 0) {
  return {pts, id};
}

{
  const q = new PlaybackBuffer({prebufferSeconds: 0.25});
  q.pushAudio(packet({pts: 0}));
  q.pushAudio(packet({pts: 100_000}));
  q.pushAudio(packet({pts: 200_000}));
  assert.equal(q.readyToStart({hasAudio: true}), true, 'contiguous 300 ms audio must satisfy prebuffer');
  assert(Math.abs(q.metrics().audioQueuedSeconds - 0.3) < 1e-6);
}

{
  const q = new PlaybackBuffer({prebufferSeconds: 0.2});
  q.pushAudio(packet({pts: 0}));
  q.pushAudio(packet({pts: 250_000}));
  assert.equal(q.readyToStart({hasAudio: true}), false, 'a timestamp gap must not count as buffered audio');
  assert(Math.abs(q.metrics().audioQueuedSeconds - 0.1) < 1e-6);
  assert.equal(q.noteAudioPlaybackPosition(0.15), true, 'running beyond the contiguous frontier must report underrun');
  assert.equal(q.metrics().audioUnderruns, 1);
}

{
  const q = new PlaybackBuffer();
  q.pushVideo(frame(0, 1));
  q.pushVideo(frame(33_333, 2));
  q.pushVideo(frame(66_666, 3));
  assert.equal(q.takeVideoForTime(0.05)?.frame.id, 2, 'presentation should select the newest due frame without blocking decode');
  assert.equal(q.video.length, 1, 'future video stays queued');
}

{
  const q = new PlaybackBuffer();
  q.pushAudio(packet({pts: 100_000}));
  q.pushAudio(packet({pts: 0}));
  assert(Math.abs(q.metrics().audioFrontier - 0.2) < 1e-6, 'out-of-order decode delivery should still form a contiguous frontier');
}

{
  const q = new PlaybackBuffer({prebufferSeconds: 1});
  q.pushAudio(packet({pts: 0}));
  assert.equal(q.readyToStart({hasAudio: true}), false);
  q.markDecodeFinished();
  assert.equal(q.readyToStart({hasAudio: true}), true, 'short finished clips must not wait forever for the target prebuffer');
}

console.log('playback-buffer smoke: ok');
