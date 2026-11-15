// DEEP MODULE 1: REAL RECORDED HARDWARE FOLEY ENGINE
// Public contract exposes semantic physical gestures only; WebAudio graph,
// 44.1kHz PCM decoding, cabinet warm-filtering, pitch jitter & rate-limiting stay internal.

import { FOLEY_SAMPLES_B64, type FoleySampleId } from '../assets/foley-samples';

class HardwareFoleyEngine {
  private ctx: AudioContext | null = null;
  private buffers = new Map<FoleySampleId, AudioBuffer>();
  private bodyFilter: BiquadFilterNode | null = null;
  private lastTriggerMs = new Map<FoleySampleId, number>();
  private decodingStarted = false;

  private ensureReady(): AudioContext | null {
    if (!this.ctx) {
      const AC = window.AudioContext || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
      if (!AC) return null;

      this.ctx = new AC();
      this.bodyFilter = this.ctx.createBiquadFilter();
      this.bodyFilter.type = 'lowpass';
      this.bodyFilter.frequency.value = 9600;
      this.bodyFilter.Q.value = 0.55;

      const master = this.ctx.createGain();
      master.gain.value = 0.85;

      this.bodyFilter.connect(master);
      master.connect(this.ctx.destination);
    }

    if (this.ctx.state === 'suspended') {
      this.ctx.resume().catch(() => {});
    }

    if (!this.decodingStarted) {
      this.decodingStarted = true;
      this.decodeAllSamples();
    }
    return this.ctx;
  }

  private decodeAllSamples(): void {
    const ctx = this.ctx;
    if (!ctx) return;

    (Object.keys(FOLEY_SAMPLES_B64) as FoleySampleId[]).forEach((id) => {
      const b64 = FOLEY_SAMPLES_B64[id];
      const binary = atob(b64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) {
        bytes[i] = binary.charCodeAt(i);
      }
      ctx.decodeAudioData(
        bytes.buffer.slice(0),
        (decoded) => {
          this.buffers.set(id, decoded);
        },
        () => {}
      );
    });
  }

  private fire(
    id: FoleySampleId,
    opts: { gain: number; rate: number; jitter: number; minGapMs: number }
  ): void {
    const ctx = this.ensureReady();
    if (!ctx || !this.bodyFilter) return;

    const now = performance.now();
    const prev = this.lastTriggerMs.get(id) ?? 0;
    if (now - prev < opts.minGapMs) return;
    this.lastTriggerMs.set(id, now);

    const buf = this.buffers.get(id);
    if (!buf) {
      window.setTimeout(() => {
        const readyBuf = this.buffers.get(id);
        if (readyBuf) this.spawnVoice(readyBuf, opts.gain, opts.rate, opts.jitter);
      }, 10);
      return;
    }

    this.spawnVoice(buf, opts.gain, opts.rate, opts.jitter);
  }

  private spawnVoice(buf: AudioBuffer, gain: number, rate: number, jitter: number): void {
    if (!this.ctx || !this.bodyFilter) return;
    const src = this.ctx.createBufferSource();
    src.buffer = buf;

    const offset = (Math.random() - 0.5) * jitter;
    src.playbackRate.value = Math.max(0.75, Math.min(1.35, rate + offset));

    const voiceGain = this.ctx.createGain();
    voiceGain.gain.value = Math.max(0.02, Math.min(1.25, gain));

    src.connect(voiceGain);
    voiceGain.connect(this.bodyFilter);
    src.start(0);
  }

  // --- Narrow Public Semantic Interface ---

  warmUp(): void {
    this.ensureReady();
  }

  pressSwitch(heavy = true): void {
    this.fire(heavy ? 'btnDown' : 'microDown', {
      gain: heavy ? 0.78 : 0.52,
      rate: heavy ? 0.98 : 1.02,
      jitter: 0.035,
      minGapMs: 20,
    });
  }

  releaseSwitch(heavy = true): void {
    this.fire(heavy ? 'btnUp' : 'microUp', {
      gain: heavy ? 0.66 : 0.44,
      rate: heavy ? 1.0 : 1.03,
      jitter: 0.035,
      minGapMs: 20,
    });
  }

  stepRotary(dir: 1 | -1, velocity = 1): void {
    const dynamicGain = Math.min(0.88, 0.48 + velocity * 0.18);
    this.fire(dir >= 0 ? 'dialCW' : 'dialCCW', {
      gain: dynamicGain,
      rate: dir >= 0 ? 1.01 : 0.96,
      jitter: 0.035,
      minGapMs: 25,
    });
  }

  latchBand(): void {
    this.fire('bandLatch', {
      gain: 0.82,
      rate: 0.96,
      jitter: 0.02,
      minGapMs: 55,
    });
  }

  tickFine(pitchBias = 1.0): void {
    this.fire('fineTick', {
      gain: 0.36,
      rate: pitchBias,
      jitter: 0.03,
      minGapMs: 22,
    });
  }

  brushSurface(): void {
    if (!this.ctx || this.ctx.state !== 'running') return;
    this.fire('softTouch', {
      gain: 0.12,
      rate: 1.08,
      jitter: 0.05,
      minGapMs: 90,
    });
  }
}

export const foley = new HardwareFoleyEngine();
