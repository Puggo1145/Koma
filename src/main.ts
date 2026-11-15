// COMPOSITION ROOT — Wires the 5 Deep Modules together with zero business logic leakage

import './styles/chassis.css';
import './styles/crt.css';

import { desktop } from './core/desktop';
import { foley } from './core/foley';
import { YoruPlayerEngine } from './core/player';
import { bindLinearScrubber, bindPushSwitch, bindRotaryControl } from './hardware/controls';
import { MicroCrtDisplay } from './hardware/crt';

async function bootstrap(): Promise<void> {
  // 1. Build 5x8 recessed speaker dot matrix
  const grilleEl = document.getElementById('rt01-grille');
  if (grilleEl) {
    grilleEl.insertAdjacentHTML(
      'beforeend',
      Array.from({ length: 40 }, () => '<span class="speaker-dot"></span>').join('')
    );
  }

  // 2. Pre-warm recorded foley AudioContext on first user gesture
  ['pointerdown', 'keydown', 'wheel'].forEach((ev) => {
    window.addEventListener(ev, () => foley.warmUp(), { once: true, passive: true });
  });

  // 3. Instantiate Deep Modules
  const player = new YoruPlayerEngine();

  const crt = new MicroCrtDisplay({
    onSelectTrack: (albumIdx, trackIdx) => {
      const { crossedAlbum } = player.selectTrack(albumIdx, trackIdx, true);
      if (crossedAlbum) foley.latchBand();
      crt.pulseBeam();
    },
    onSeekLyric: (lineIdx) => {
      player.seekToLyric(lineIdx);
    },
    onCycleAlbum: () => {
      const snap = player.getSnapshot();
      const nextAlb = (snap.albumIdx + 1) % Math.max(1, snap.albums.length);
      player.selectTrack(nextAlb, 0, snap.playing);
      foley.latchBand();
      crt.pulseBeam();
    },
  });

  // 4. Bind Physical Chassis Controls
  let tuneAngle = 0;
  let volAngle = 68; // -132..+132 => ~76%
  let volTickAccum = 0;

  const syncRotors = () => {
    const tuneRotor = document.getElementById('rt01-tune-rotor');
    const volRotor = document.getElementById('rt01-vol-rotor');
    if (tuneRotor) tuneRotor.style.transform = `rotate(${tuneAngle}deg)`;
    if (volRotor) volRotor.style.transform = `rotate(${volAngle}deg)`;
  };
  syncRotors();

  // Left Push-Switch: Play / Pause
  bindPushSwitch(document.getElementById('rt01-play-btn'), {
    heavy: true,
    onActivate: () => player.togglePlay(),
  });

  // Right Small Rotary Push-Knob: Volume + Cycle CRT View
  bindRotaryControl(document.getElementById('rt01-vol-knob'), {
    heavyPress: false,
    getNotchThresholdDeg: () => 999,
    onRotateDeg: (degDelta) => {
      volAngle = Math.max(-132, Math.min(132, volAngle + degDelta));
      const vol = player.setVolume((volAngle + 132) / 264);
      syncRotors();
      crt.flashVolume(vol);

      volTickAccum += Math.abs(degDelta);
      if (volTickAccum >= 11) {
        volTickAccum = 0;
        foley.tickFine(0.92 + vol * 0.16);
      }
    },
    onPressCenter: () => crt.cycleMode(),
  });

  // Big Right Matte-White Rotary Tuning Dial: Tune Songs / Navigate Menu + Center Confirm
  bindRotaryControl(document.getElementById('rt01-tune-dial'), {
    heavyPress: true,
    getNotchThresholdDeg: () => (crt.getMode() === 'menu' ? 16 : 20),
    onRotateDeg: (degDelta) => {
      tuneAngle += degDelta;
      syncRotors();
    },
    onNotch: (dir, velocity) => {
      foley.stepRotary(dir, velocity);
      if (crt.getMode() === 'menu') {
        crt.stepMenuCursor(dir);
      } else {
        const { crossedAlbum } = player.stepTrack(dir);
        if (crossedAlbum) foley.latchBand();
        crt.pulseBeam();
      }
    },
    onPressCenter: () => {
      if (crt.getMode() === 'menu') {
        crt.confirmMenuCursor();
      } else {
        crt.setMode('menu', true);
      }
    },
  });

  // Analog Tuning Scale Window: Scrub Song Progress
  bindLinearScrubber(document.getElementById('rt01-scale'), {
    onScrub: (ratio) => player.seek(ratio),
  });

  // 5. Connect Reactive Player Store -> Hardware Faceplate & CRT
  player.onTrackChange((snap) => {
    crt.renderTrackSnapshot(snap);

    document.getElementById('rt01-led')?.classList.toggle('on', snap.playing);
    const playIcon = document.getElementById('rt01-play-icon');
    if (playIcon) {
      playIcon.innerHTML = snap.playing
        ? '<rect x="4.3" y="3.2" width="2.7" height="9.6" rx="0.8"/><rect x="9.0" y="3.2" width="2.7" height="9.6" rx="0.8"/>'
        : '<path d="M4.8 3.2L13.2 8L4.8 12.8V3.2Z"/>';
    }

    const needle = document.getElementById('rt01-needle');
    if (needle) {
      const x = 14 + (snap.flatIdx / Math.max(1, snap.totalFlatTracks - 1)) * 152;
      needle.style.left = `${x.toFixed(1)}px`;
    }
  });

  player.onProgress((snap) => {
    crt.renderProgressSnapshot(snap);
    const progFill = document.getElementById('rt01-prog');
    if (progFill) progFill.style.width = `${(snap.progress * 100).toFixed(1)}%`;
  });

  // 6. Keyboard & Drag-and-Drop Support
  window.addEventListener('keydown', (e) => {
    if (e.target instanceof HTMLElement && ['INPUT', 'TEXTAREA'].includes(e.target.tagName)) return;
    if (e.code === 'Space') {
      e.preventDefault();
      document.getElementById('rt01-play-btn')?.classList.add('down');
      foley.pressSwitch(true);
    } else if (e.code === 'ArrowRight') {
      e.preventDefault();
      foley.stepRotary(1, 1);
      const { crossedAlbum } = player.stepTrack(1);
      if (crossedAlbum) foley.latchBand();
      crt.pulseBeam();
    } else if (e.code === 'ArrowLeft') {
      e.preventDefault();
      foley.stepRotary(-1, 1);
      const { crossedAlbum } = player.stepTrack(-1);
      if (crossedAlbum) foley.latchBand();
      crt.pulseBeam();
    } else if (e.code === 'KeyM') {
      foley.pressSwitch(false);
      crt.cycleMode();
    }
  });

  window.addEventListener('keyup', (e) => {
    if (e.code === 'Space') {
      const btn = document.getElementById('rt01-play-btn');
      if (btn?.classList.contains('down')) {
        btn.classList.remove('down');
        foley.releaseSwitch(true);
        player.togglePlay();
      }
    }
  });

  window.addEventListener('dragover', (e) => {
    e.preventDefault();
    document.body.classList.add('file-hover');
  });
  window.addEventListener('dragleave', (e) => {
    if (e.relatedTarget === null) document.body.classList.remove('file-hover');
  });
  window.addEventListener('drop', (e) => {
    e.preventDefault();
    document.body.classList.remove('file-hover');
    const files = Array.from(e.dataTransfer?.files ?? []);
    if (files.length) {
      foley.latchBand();
      crt.pulseBeam();
      player.ingestDroppedFiles(files);
    }
  });

  // 7. Boot Catalog & Native Tauri Shell Bridge
  await player.loadBuiltInCatalog();

  desktop.bindChassisShell(
    document.getElementById('rt01-wood-frame'),
    document.getElementById('rt01-grille')
  );

  await desktop.init({
    onPlayToggle: () => {
      foley.pressSwitch(true);
      player.togglePlay();
    },
    onStepTrack: (delta) => {
      foley.stepRotary(delta, 1);
      const { crossedAlbum } = player.stepTrack(delta);
      if (crossedAlbum) foley.latchBand();
      crt.pulseBeam();
    },
    onLocalAlbumsDiscovered: (localAlbums) => {
      player.mergeLocalAlbums(localAlbums);
    },
  });
}

void bootstrap();
