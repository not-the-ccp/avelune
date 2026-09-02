const seconds = value => Number(value) / 1e6;

export class PlaybackBuffer {
  constructor({maxAudioSeconds = 4, maxVideoSeconds = 4, prebufferSeconds = 0.35} = {}) {
    if (!(maxAudioSeconds > 0) || !(maxVideoSeconds > 0) || !(prebufferSeconds >= 0)) {
      throw new RangeError('invalid playback buffer limits');
    }
    this.maxAudioSeconds = maxAudioSeconds;
    this.maxVideoSeconds = maxVideoSeconds;
    this.prebufferSeconds = prebufferSeconds;
    this.reset(0);
  }

  reset(mediaStart = 0) {
    this.mediaStart = Number(mediaStart) || 0;
    this.audio = [];
    this.video = [];
    this.audioFrontier = this.mediaStart;
    this.audioQueuedUntil = this.mediaStart;
    this.videoQueuedUntil = this.mediaStart;
    this.lateAudioPackets = 0;
    this.audioUnderruns = 0;
    this.lastUnderrunAt = null;
    this.decodeFinished = false;
  }

  pushAudio(packet) {
    if (!packet || !Number.isFinite(packet.rate) || packet.rate <= 0 || !Number.isInteger(packet.channels) || packet.channels <= 0) {
      throw new TypeError('invalid audio packet');
    }
    const frames = packet.pcm.length / packet.channels;
    if (!Number.isInteger(frames)) throw new TypeError('audio PCM is not channel-aligned');
    const start = seconds(packet.pts);
    const duration = frames / packet.rate;
    const end = start + duration;
    if (end <= this.mediaStart) return false;
    if (start + 1e-6 < this.audioFrontier) this.lateAudioPackets++;
    this.audio.push({packet, start, end});
    this.audio.sort((a, b) => a.start - b.start);
    this.#recomputeAudioFrontier();
    this.#trimAudio();
    return true;
  }

  pushVideo(frame) {
    const start = seconds(frame.pts);
    if (start < this.mediaStart) return false;
    this.video.push({frame, start});
    this.video.sort((a, b) => a.start - b.start);
    this.videoQueuedUntil = Math.max(this.videoQueuedUntil, start);
    this.#trimVideo();
    return true;
  }

  markDecodeFinished() {
    this.decodeFinished = true;
  }

  bufferedAudioSeconds(at = this.mediaStart) {
    return Math.max(0, this.audioFrontier - at);
  }

  bufferedVideoSeconds(at = this.mediaStart) {
    return Math.max(0, this.videoQueuedUntil - at);
  }

  readyToStart({hasAudio, hasVideo, at = this.mediaStart} = {}) {
    if (hasAudio && this.bufferedAudioSeconds(at) < this.prebufferSeconds && !this.decodeFinished) return false;
    if (hasVideo && !this.video.length && !this.decodeFinished) return false;
    return true;
  }

  takeAudioThrough(until) {
    const out = [];
    while (this.audio.length && this.audio[0].start <= until) out.push(this.audio.shift());
    return out;
  }

  takeVideoForTime(now) {
    let chosen = null;
    while (this.video.length && this.video[0].start <= now) chosen = this.video.shift();
    return chosen;
  }

  noteAudioPlaybackPosition(now) {
    if (now > this.audioFrontier + 0.002 && !this.decodeFinished) {
      if (this.lastUnderrunAt === null || now - this.lastUnderrunAt > 0.05) {
        this.audioUnderruns++;
        this.lastUnderrunAt = now;
      }
      return true;
    }
    return false;
  }

  metrics(at = this.mediaStart) {
    return {
      audioQueuedSeconds: this.bufferedAudioSeconds(at),
      videoQueuedSeconds: this.bufferedVideoSeconds(at),
      audioPackets: this.audio.length,
      videoFrames: this.video.length,
      lateAudioPackets: this.lateAudioPackets,
      audioUnderruns: this.audioUnderruns,
      audioFrontier: this.audioFrontier,
    };
  }

  #recomputeAudioFrontier() {
    let frontier = this.mediaStart;
    for (const item of this.audio) {
      if (item.end <= frontier) continue;
      if (item.start > frontier + 0.002) break;
      frontier = Math.max(frontier, item.end);
    }
    this.audioFrontier = frontier;
    this.audioQueuedUntil = Math.max(this.audioQueuedUntil, frontier);
  }

  #trimAudio() {
    const keepAfter = Math.max(this.mediaStart, this.audioFrontier - this.maxAudioSeconds);
    while (this.audio.length && this.audio[0].end < keepAfter) this.audio.shift();
  }

  #trimVideo() {
    if (!this.video.length) return;
    const newest = this.video[this.video.length - 1].start;
    const keepAfter = Math.max(this.mediaStart, newest - this.maxVideoSeconds);
    while (this.video.length > 1 && this.video[1].start < keepAfter) this.video.shift();
  }
}
