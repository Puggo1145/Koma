// DEEP MODULE 4: PHYSICAL HARDWARE KINEMATICS
// Hides pointer capture, polar atan2 angle unwrapping, center press disambiguation,
// mousewheel normalization, and two-phase mechanical foley behind clean hardware primitives.

import { foley } from '../core/foley';

export interface PushSwitchSpec {
  heavy?: boolean;
  onActivate: () => void;
}

export interface RotaryKnobSpec {
  heavyPress?: boolean;
  getNotchThresholdDeg: () => number;
  onRotateDeg: (degDelta: number) => void;
  onNotch?: (dir: 1 | -1, velocity: number) => void;
  onPressCenter?: () => void;
}

export interface LinearScrubberSpec {
  tickEveryRatio?: number;
  onScrub: (ratio: number) => void;
}

export function bindMicroFoley(el: HTMLElement | null, heavy = false): void {
  if (!el) return;
  el.addEventListener('pointerdown', () => foley.pressSwitch(heavy));
  el.addEventListener('pointerup', () => foley.releaseSwitch(heavy));
}

export function bindPushSwitch(el: HTMLElement | null, spec: PushSwitchSpec): void {
  if (!el) return;
  const heavy = spec.heavy ?? true;
  let armed = false;

  el.addEventListener('pointerenter', () => foley.brushSurface());

  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    armed = true;
    el.classList.add('down');
    el.setPointerCapture(e.pointerId);
    foley.pressSwitch(heavy);
  });

  const finish = (shouldTrigger: boolean) => {
    if (!armed) return;
    armed = false;
    el.classList.remove('down');
    foley.releaseSwitch(heavy);
    if (shouldTrigger) spec.onActivate();
  };

  el.addEventListener('pointerup', () => finish(true));
  el.addEventListener('pointercancel', () => finish(false));
}

export function bindRotaryControl(el: HTMLElement | null, spec: RotaryKnobSpec): void {
  if (!el) return;
  const heavyPress = spec.heavyPress ?? true;

  let dragging = false;
  let prevAngleRad: number | null = null;
  let prevX = 0;
  let prevY = 0;
  let totalMovedDeg = 0;
  let notchAccum = 0;
  let lastMoveMs = 0;

  el.addEventListener('pointerenter', () => foley.brushSurface());

  const processDelta = (degDelta: number) => {
    if (!degDelta) return;
    const now = performance.now();
    const dt = Math.max(8, now - lastMoveMs);
    lastMoveMs = now;
    const velocity = Math.min(2.2, Math.abs(degDelta) / (dt * 0.18));

    spec.onRotateDeg(degDelta);

    if (spec.onNotch) {
      notchAccum += degDelta;
      const threshold = Math.max(6, spec.getNotchThresholdDeg());
      while (Math.abs(notchAccum) >= threshold) {
        const dir: 1 | -1 = notchAccum > 0 ? 1 : -1;
        notchAccum -= dir * threshold;
        spec.onNotch(dir, velocity);
      }
    }
  };

  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    dragging = true;
    totalMovedDeg = 0;
    notchAccum = 0;
    lastMoveMs = performance.now();
    prevX = e.clientX;
    prevY = e.clientY;

    const rect = el.getBoundingClientRect();
    const cx = rect.left + rect.width / 2;
    const cy = rect.top + rect.height / 2;
    const radius = Math.hypot(e.clientX - cx, e.clientY - cy);
    // Use polar atan2 swirling on large dials; linear vertical/horizontal drag on small knobs
    prevAngleRad = rect.width >= 72 && radius > 13 ? Math.atan2(e.clientY - cy, e.clientX - cx) : null;

    el.classList.add('down');
    el.setPointerCapture(e.pointerId);
    foley.pressSwitch(heavyPress);
  });

  el.addEventListener('pointermove', (e) => {
    if (!dragging) return;
    const rect = el.getBoundingClientRect();
    const cx = rect.left + rect.width / 2;
    const cy = rect.top + rect.height / 2;
    const dxCenter = e.clientX - cx;
    const dyCenter = e.clientY - cy;
    const radius = Math.hypot(dxCenter, dyCenter);

    let degDelta = 0;
    if (rect.width >= 72 && radius > 13) {
      const curRad = Math.atan2(dyCenter, dxCenter);
      if (prevAngleRad !== null) {
        let diff = curRad - prevAngleRad;
        while (diff > Math.PI) diff -= Math.PI * 2;
        while (diff < -Math.PI) diff += Math.PI * 2;
        degDelta = (diff * 180) / Math.PI;
      }
      prevAngleRad = curRad;
    } else {
      const dy = prevY - e.clientY;
      const dx = e.clientX - prevX;
      degDelta = dy * 1.45 + dx * 0.65;
      prevAngleRad = null;
    }

    prevX = e.clientX;
    prevY = e.clientY;
    totalMovedDeg += Math.abs(degDelta);

    if (totalMovedDeg > 4.2) {
      el.classList.remove('down');
      processDelta(degDelta);
    }
  });

  const endInteraction = (allowPress: boolean) => {
    if (!dragging) return;
    dragging = false;
    el.classList.remove('down');
    if (totalMovedDeg <= 4.2) {
      foley.releaseSwitch(heavyPress);
      if (allowPress && spec.onPressCenter) {
        spec.onPressCenter();
      }
    }
  };

  el.addEventListener('pointerup', () => endInteraction(true));
  el.addEventListener('pointercancel', () => endInteraction(false));

  let wheelAccum = 0;
  el.addEventListener(
    'wheel',
    (e) => {
      e.preventDefault();
      const deg = -e.deltaY * 0.25;
      spec.onRotateDeg(deg);
      if (spec.onNotch) {
        wheelAccum += deg;
        const threshold = Math.max(6, spec.getNotchThresholdDeg() * 0.9);
        if (Math.abs(wheelAccum) >= threshold) {
          const dir: 1 | -1 = wheelAccum > 0 ? 1 : -1;
          wheelAccum = 0;
          spec.onNotch(dir, 1.0);
        }
      }
    },
    { passive: false }
  );
}

export function bindLinearScrubber(el: HTMLElement | null, spec: LinearScrubberSpec): void {
  if (!el) return;
  let scrubbing = false;
  let lastTickRatio = 0;
  const tickStep = spec.tickEveryRatio ?? 0.06;

  el.addEventListener('pointerenter', () => foley.brushSurface());

  const updateAt = (clientX: number) => {
    const rect = el.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(0.996, (clientX - rect.left) / rect.width));
    if (Math.abs(ratio - lastTickRatio) >= tickStep) {
      lastTickRatio = ratio;
      foley.tickFine(1.08);
    }
    spec.onScrub(ratio);
  };

  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    scrubbing = true;
    el.setPointerCapture(e.pointerId);
    foley.pressSwitch(false);
    updateAt(e.clientX);
  });

  el.addEventListener('pointermove', (e) => {
    if (scrubbing) updateAt(e.clientX);
  });

  const stop = () => {
    if (!scrubbing) return;
    scrubbing = false;
    foley.releaseSwitch(false);
  };

  el.addEventListener('pointerup', stop);
  el.addEventListener('pointercancel', stop);
}
