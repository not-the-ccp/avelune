import assert from 'node:assert/strict';
import {PcmTimeline} from '../web/player/audio-timeline.js';

function outputs(channels, frames) {
  return Array.from({length: channels}, () => new Float32Array(frames));
}

{
  const timeline = new PcmTimeline(1);
  timeline.push(10, new Float32Array([1, 2, 3, 4]));
  const out = outputs(1, 8);
  const first = timeline.render(out, 8, 8);
  assert.deepEqual([...out[0]], [0, 0, 1, 2, 3, 4, 0, 0]);
  assert.equal(first.copiedFrames, 4);
  assert.equal(first.silentFrames, 4);
}

{
  const timeline = new PcmTimeline(2);
  timeline.push(0, new Float32Array([1, 10, 2, 20]));
  timeline.push(2, new Float32Array([3, 30, 4, 40]));
  const out = outputs(2, 4);
  const result = timeline.render(out, 0, 4);
  assert.deepEqual([...out[0]], [1, 2, 3, 4]);
  assert.deepEqual([...out[1]], [10, 20, 30, 40]);
  assert.equal(result.silentFrames, 0);
  assert.equal(timeline.queuedFrames(), 0);
}

{
  const timeline = new PcmTimeline(1);
  timeline.push(0, new Float32Array([1, 1]));
  timeline.push(4, new Float32Array([2, 2]));
  const out = outputs(1, 6);
  const result = timeline.render(out, 0, 6);
  assert.deepEqual([...out[0]], [1, 1, 0, 0, 2, 2]);
  assert.equal(result.silentFrames, 2, 'timestamp holes must render explicit silence rather than close up time');
}

{
  const timeline = new PcmTimeline(1);
  timeline.render(outputs(1, 8), 0, 8);
  const status = timeline.push(4, new Float32Array([1, 2, 3, 4, 5, 6, 7, 8]));
  assert.equal(status.lateFrames, 4, 'PCM arriving after already-rendered frames must be measurable');
  const out = outputs(1, 8);
  timeline.render(out, 8, 8);
  assert.deepEqual([...out[0].slice(0, 4)], [5, 6, 7, 8]);
}

{
  const timeline = new PcmTimeline(1);
  timeline.push(100, new Float32Array(20));
  assert.equal(timeline.queuedFrames(), 120, 'queued extent is measured from the processor timeline before rendering begins');
  timeline.clear(100);
  assert.equal(timeline.queuedFrames(), 0);
  assert.equal(timeline.renderedUntil, 100);
}

assert.throws(() => new PcmTimeline(0), /channel/);
assert.throws(() => new PcmTimeline(2).push(0, new Float32Array(3)), /channel-aligned/);
console.log('audio-timeline smoke: ok');
