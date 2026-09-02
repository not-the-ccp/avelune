import {PcmTimeline} from './audio-timeline.js';

class AvelunePcmSinkProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const channels = Number(options.processorOptions?.channels);
    this.timeline = new PcmTimeline(channels);
    this.timeline.clear(currentFrame);
    this.stopped = false;
    this.port.onmessage = event => {
      const message = event.data ?? {};
      if (message.type === 'pcm') {
        const pcm = message.pcm instanceof Float32Array ? message.pcm : new Float32Array(message.pcm);
        const status = this.timeline.push(Number(message.startFrame), pcm);
        if (status.lateFrames) this.port.postMessage({type: 'late', lateFrames: status.lateFrames});
      } else if (message.type === 'reset') {
        this.timeline.clear(Number(message.atFrame));
      } else if (message.type === 'stop') {
        this.stopped = true;
        this.timeline.clear(currentFrame);
      }
    };
  }

  process(_inputs, outputs) {
    const output = outputs[0] ?? [];
    if (!output.length) return !this.stopped;
    const frames = output[0].length;
    if (this.stopped) {
      for (const channel of output) channel.fill(0);
      return false;
    }
    this.timeline.render(output, currentFrame, frames);
    return true;
  }
}

registerProcessor('avelune-pcm-sink', AvelunePcmSinkProcessor);
