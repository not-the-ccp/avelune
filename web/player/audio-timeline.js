export class PcmTimeline {
  constructor(channels) {
    if (!Number.isInteger(channels) || channels <= 0 || channels > 8) throw new RangeError('invalid PCM channel count');
    this.channels = channels;
    this.chunks = [];
    this.renderedUntil = 0;
  }

  clear(atFrame = 0) {
    if (!Number.isSafeInteger(atFrame) || atFrame < 0) throw new RangeError('invalid PCM reset frame');
    this.chunks = [];
    this.renderedUntil = atFrame;
  }

  push(startFrame, interleaved) {
    if (!Number.isSafeInteger(startFrame) || startFrame < 0) throw new RangeError('invalid PCM start frame');
    if (!(interleaved instanceof Float32Array) || interleaved.length % this.channels) {
      throw new TypeError('PCM chunk must be channel-aligned Float32Array');
    }
    const frames = interleaved.length / this.channels;
    if (!frames) return {lateFrames: 0, queuedFrames: this.queuedFrames()};
    const endFrame = startFrame + frames;
    const lateFrames = Math.max(0, Math.min(frames, this.renderedUntil - startFrame));
    if (endFrame > this.renderedUntil) {
      this.chunks.push({startFrame, endFrame, interleaved});
      this.chunks.sort((a, b) => a.startFrame - b.startFrame);
    }
    return {lateFrames, queuedFrames: this.queuedFrames()};
  }

  render(outputs, startFrame, frameCount) {
    if (!Array.isArray(outputs) || outputs.length !== this.channels) throw new TypeError('PCM output channel mismatch');
    if (!Number.isSafeInteger(startFrame) || startFrame < 0 || !Number.isSafeInteger(frameCount) || frameCount < 0) {
      throw new RangeError('invalid PCM render range');
    }
    for (const channel of outputs) {
      if (!(channel instanceof Float32Array) || channel.length < frameCount) throw new TypeError('invalid PCM output buffer');
      channel.fill(0, 0, frameCount);
    }
    const endFrame = startFrame + frameCount;
    let copiedFrames = 0;
    for (const chunk of this.chunks) {
      if (chunk.endFrame <= startFrame) continue;
      if (chunk.startFrame >= endFrame) break;
      const first = Math.max(startFrame, chunk.startFrame);
      const last = Math.min(endFrame, chunk.endFrame);
      for (let frame = first; frame < last; frame++) {
        const srcFrame = frame - chunk.startFrame;
        const dstFrame = frame - startFrame;
        for (let channel = 0; channel < this.channels; channel++) {
          outputs[channel][dstFrame] = chunk.interleaved[srcFrame * this.channels + channel];
        }
      }
      copiedFrames += last - first;
    }
    this.renderedUntil = Math.max(this.renderedUntil, endFrame);
    this.chunks = this.chunks.filter(chunk => chunk.endFrame > this.renderedUntil);
    return {copiedFrames, silentFrames: Math.max(0, frameCount - copiedFrames)};
  }

  queuedFrames() {
    if (!this.chunks.length) return 0;
    const last = this.chunks.reduce((max, chunk) => Math.max(max, chunk.endFrame), this.renderedUntil);
    return Math.max(0, last - this.renderedUntil);
  }
}
