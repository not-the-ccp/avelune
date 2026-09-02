const seconds = value => Number(value) / 1e6;
const DEADLINE_EPSILON = 0.005;

export class AudioUnderrunError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AudioUnderrunError';
  }
}

export class AudioSink {
  constructor() {
    this.context = null;
    this.gain = null;
    this.volume = 1;
    this.sources = new Set();
    this.workletNode = null;
    this.workletModuleLoaded = false;
    this.workletAvailable = false;
    this.requestedRate = null;
    this.requestedChannels = null;
    this.mode = 'uninitialized';
    this.fault = null;
    this.workletLateFrames = 0;
    this.scheduledPackets = 0;
  }

  async ensure(rate, channels) {
    if (!Number.isFinite(rate) || rate <= 0 || !Number.isInteger(channels) || channels <= 0 || channels > 8) {
      throw new RangeError('invalid audio stream parameters');
    }
    this.requestedRate = rate;
    this.requestedChannels = channels;

    if (!this.context) {
      try {
        this.context = new AudioContext({sampleRate: rate});
      } catch {
        this.context = new AudioContext();
      }
      this.gain = this.context.createGain();
      this.gain.gain.value = this.volume;
      this.gain.connect(this.context.destination);
    }

    // Resume immediately while the play action still has user activation. Loading the worklet
    // module may await network/module work and should not be the operation that wakes WebAudio.
    const resumed = this.context.resume();
    this.workletAvailable = false;
    if (this.context.sampleRate === rate && this.context.audioWorklet && typeof AudioWorkletNode === 'function') {
      try {
        if (!this.workletModuleLoaded) {
          await this.context.audioWorklet.addModule(new URL('./audio-worklet.js', import.meta.url));
          this.workletModuleLoaded = true;
        }
        this.workletAvailable = true;
      } catch {
        this.workletAvailable = false;
      }
    }
    await resumed;
    return this.outputMode();
  }

  begin(channels = this.requestedChannels) {
    if (!this.context || !this.gain) throw new Error('audio sink is not initialized');
    this.stopAll();
    this.fault = null;
    this.workletLateFrames = 0;
    this.scheduledPackets = 0;

    if (this.workletAvailable) {
      try {
        const node = new AudioWorkletNode(this.context, 'avelune-pcm-sink', {
          numberOfInputs: 0,
          numberOfOutputs: 1,
          outputChannelCount: [channels],
          channelCount: channels,
          channelCountMode: 'explicit',
          channelInterpretation: 'discrete',
          processorOptions: {channels},
        });
        node.port.onmessage = event => {
          const message = event.data ?? {};
          if (message.type === 'late' && Number.isFinite(message.lateFrames) && message.lateFrames > 0) {
            this.workletLateFrames += message.lateFrames;
            const ms = message.lateFrames / this.context.sampleRate * 1000;
            this.fault ??= new AudioUnderrunError(`audio worklet received PCM ${ms.toFixed(1)} ms after its presentation deadline`);
          }
        };
        node.onprocessorerror = () => {
          this.fault ??= new AudioUnderrunError('audio worklet processor failed');
        };
        node.connect(this.gain);
        this.workletNode = node;
        this.mode = 'AudioWorklet PCM';
        return this.mode;
      } catch {
        this.workletNode = null;
      }
    }

    this.mode = this.context.sampleRate === this.requestedRate
      ? 'scheduled AudioBuffer fallback'
      : `scheduled AudioBuffer fallback · ${this.context.sampleRate} Hz device`;
    return this.mode;
  }

  setVolume(value) {
    this.volume = value;
    if (this.gain) this.gain.gain.value = value;
  }

  stopAll() {
    if (this.workletNode) {
      try { this.workletNode.port.postMessage({type: 'stop'}); } catch {}
      try { this.workletNode.disconnect(); } catch {}
      try { this.workletNode.port.close(); } catch {}
      this.workletNode = null;
    }
    for (const source of this.sources) {
      try { source.stop(); } catch {}
    }
    this.sources.clear();
  }

  schedule(packet, mediaStart, contextStart) {
    let packetStart = seconds(packet.pts);
    const packetFrames = packet.pcm.length / packet.channels;
    const packetDuration = packetFrames / packet.rate;
    let skipFrames = 0;
    if (packetStart + packetDuration <= mediaStart) return;
    if (packetStart < mediaStart) {
      skipFrames = Math.min(packetFrames, Math.floor((mediaStart - packetStart) * packet.rate));
      packetStart += skipFrames / packet.rate;
    }
    const frameCount = packetFrames - skipFrames;
    if (frameCount <= 0) return;

    const when = contextStart + Math.max(0, packetStart - mediaStart);
    if (when < this.context.currentTime + DEADLINE_EPSILON) {
      const lateMs = Math.max(0, (this.context.currentTime - when) * 1000);
      throw new AudioUnderrunError(`audio packet missed its presentation deadline by ${lateMs.toFixed(1)} ms`);
    }

    if (this.workletNode && packet.rate === this.context.sampleRate) {
      const pcm = new Float32Array(frameCount * packet.channels);
      for (let i = 0; i < frameCount; i++) {
        const sourceOffset = (i + skipFrames) * packet.channels;
        const targetOffset = i * packet.channels;
        for (let channel = 0; channel < packet.channels; channel++) {
          pcm[targetOffset + channel] = packet.pcm[sourceOffset + channel] / 32768;
        }
      }
      const startFrame = Math.round(when * this.context.sampleRate);
      this.workletNode.port.postMessage({type: 'pcm', startFrame, pcm}, [pcm.buffer]);
      this.scheduledPackets++;
      return;
    }

    const buffer = this.context.createBuffer(packet.channels, frameCount, packet.rate);
    for (let channel = 0; channel < packet.channels; channel++) {
      const out = buffer.getChannelData(channel);
      for (let i = 0; i < frameCount; i++) {
        out[i] = packet.pcm[(i + skipFrames) * packet.channels + channel] / 32768;
      }
    }
    const source = this.context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gain);
    source.addEventListener('ended', () => this.sources.delete(source), {once: true});
    this.sources.add(source);
    source.start(when);
    this.scheduledPackets++;
  }

  takeFault() {
    const fault = this.fault;
    this.fault = null;
    return fault;
  }

  outputMode() {
    if (this.workletNode) return this.mode;
    if (this.workletAvailable) return 'AudioWorklet PCM available';
    if (!this.context) return 'uninitialized';
    return this.context.sampleRate === this.requestedRate
      ? 'scheduled AudioBuffer fallback'
      : `scheduled AudioBuffer fallback · ${this.context.sampleRate} Hz device`;
  }

  metrics() {
    return {
      mode: this.outputMode(),
      workletLateFrames: this.workletLateFrames,
      scheduledPackets: this.scheduledPackets,
      sampleRate: this.context?.sampleRate ?? null,
    };
  }
}
