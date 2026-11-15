// COMPOSITION ROOT — Wires the 5 Deep Modules together with zero business logic leakage

import './styles/chassis.css';
import './styles/crt.css';

import { desktop, type ImportKind } from './core/desktop';
import { foley } from './core/foley';
import { KomaPlayerEngine } from './core/player';
import { bindLinearScrubber, bindPushSwitch, bindRotaryControl } from './hardware/controls';
import { MicroCrtDisplay } from './hardware/crt';

function renderAcousticGrilleSvg(): void {
  const svg = document.getElementById('rt01-grille-svg');
  if (!svg) return;

  // Render all 54 speaker apertures inside a single vector coordinate system (viewBox 0 0 108 208)
  // so Skia rasterizes every circle with identical subpixel anti-aliasing on any Windows DPI scale.
  const circles: string[] = [];
  for (let row = 0; row < 9; row++) {
    const cy = 16 + row * 22;
    for (let col = 0; col < 6; col++) {
      const cx = 11.5 + col * 17;
      circles.push(
        `<circle cx="${cx}" cy="${cy + 0.55}" r="3.75" fill="rgba(255,255,255,0.78)" />`,
        `<circle cx="${cx}" cy="${cy}" r="3.75" fill="url(#grille-cavity)" />`
      );
    }
  }

  svg.innerHTML = `
    <defs>
      <radialGradient id="grille-cavity" cx="42%" cy="36%" r="68%">
        <stop offset="0%" stop-color="#1c1c1a" />
        <stop offset="68%" stop-color="#2c2b28" />
        <stop offset="100%" stop-color="#141412" />
      </radialGradient>
    </defs>
    ${circles.join('')}
  `;
}

async function bootstrap(): Promise<void> {
  // 1. Build precision 5x8 vector acoustic speaker matrix
  renderAcousticGrilleSvg();

  // 2. Pre-warm recorded foley AudioContext on first user gesture
  ['pointerdown', 'keydown', 'wheel'].forEach((ev) => {
    window.addEventListener(ev, () => foley.warmUp(), { once: true, passive: true });
  });

  // 3. Instantiate Deep Modules
  const player = new KomaPlayerEngine();

  const fileInput = document.getElementById('browser-file-input') as HTMLInputElement | null;
  const folderInput = document.getElementById('browser-folder-input') as HTMLInputElement | null;

  const triggerImportFlow = async (kind: ImportKind) => {
    if (desktop.isDesktop) {
      await desktop.pickAndImport(kind);
      return;
    }
    if (kind === 'folder') {
      folderInput?.click();
    } else {
      fileInput?.click();
    }
  };

  const crt = new MicroCrtDisplay({
    onStepTrack: (dir) => {
      const { crossedAlbum } = player.stepTrack(dir);
      if (crossedAlbum) foley.latchBand();
      crt.pulseBeam();
    },
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
      if (!snap.hasLibrary) return;
      const nextAlb = (snap.albumIdx + 1) % Math.max(1, snap.albums.length);
      player.selectTrack(nextAlb, 0, snap.playing);
      foley.latchBand();
      crt.pulseBeam();
    },
    onRequestImport: (kind) => {
      void triggerImportFlow(kind);
    },
    onToggleSource: (sourceId) => {
      if (desktop.isDesktop) {
        void desktop.toggleSource(sourceId);
      } else {
        player.toggleBrowserSource(sourceId);
      }
    },
    onReimportSource: (sourceId) => {
      if (desktop.isDesktop) {
        void desktop.reimportSource(sourceId);
      }
    },
    onRevealSource: (sourceId) => {
      if (desktop.isDesktop) {
        void desktop.revealSource(sourceId);
      }
    },
    onRemoveSource: (sourceId) => {
      if (desktop.isDesktop) {
        void desktop.removeSource(sourceId);
      } else {
        player.removeBrowserSource(sourceId);
      }
    },
    onRescanLibrary: () => {
      if (desktop.isDesktop) {
        void desktop.rescanLibrary();
      }
    },
    onClearLibrary: () => {
      if (desktop.isDesktop) {
        void desktop.clearLibrary();
      } else {
        player.clearBrowserLibrary();
      }
    },
  });

  // Browser file/folder picker handlers
  const handleBrowserInput = async (input: HTMLInputElement | null) => {
    const files = Array.from(input?.files ?? []);
    if (files.length > 0) {
      foley.latchBand();
      crt.pulseBeam();
      await player.ingestBrowserFiles(files);
    }
    if (input) input.value = '';
  };
  fileInput?.addEventListener('change', () => void handleBrowserInput(fileInput));
  folderInput?.addEventListener('change', () => void handleBrowserInput(folderInput));

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

  // Control 1 (Play Button): Strictly Play / Pause only
  bindPushSwitch(document.getElementById('rt01-play-btn'), {
    heavy: true,
    onActivate: () => {
      player.togglePlay();
    },
  });

  // Control 2 (Volume Knob): Slide/Wheel = Volume · Press = Switch CRT Screen
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

  // Control 3 (Big Dial): Strictly isolated to the active CRT screen
  //   画: Slide = Step song · Press = Toggle album cover zoom
  //   词: Slide = Scroll lyric selection · Press = Seek & play selected lyric
  //   选: Slide = Scroll song selection · Press = Play selected song
  //   设: Slide = Flip through Album Cover Flow Carousel · Press = Play Track 1 / Enable pack
  bindRotaryControl(document.getElementById('rt01-tune-dial'), {
    heavyPress: true,
    getNotchThresholdDeg: () => crt.getDialNotchThresholdDeg(),
    onRotateDeg: (degDelta) => {
      tuneAngle += degDelta;
      syncRotors();
    },
    onNotch: (dir, velocity) => {
      foley.stepRotary(dir, velocity);
      crt.stepCurrentView(dir);
    },
    onPressCenter: () => {
      crt.activateCurrentView();
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

    const needle = document.getElementById('rt01-needle');
    const scaleEl = document.getElementById('rt01-scale');
    if (needle) {
      const travel = Math.max(120, (scaleEl?.clientWidth ?? 204) - 28);
      const x = snap.hasLibrary
        ? 13 + (snap.flatIdx / Math.max(1, snap.totalFlatTracks - 1)) * travel
        : 13;
      needle.style.left = `${x.toFixed(1)}px`;
    }
  });

  player.onProgress((snap) => {
    crt.renderProgressSnapshot(snap);
    const progFill = document.getElementById('rt01-prog');
    if (progFill) progFill.style.width = `${(snap.progress * 100).toFixed(1)}%`;
  });

  let lastMediaErrorRescanAt = 0;
  player.onMediaError(() => {
    if (!desktop.isDesktop) return;
    const now = performance.now();
    if (now - lastMediaErrorRescanAt < 1500) return;
    lastMediaErrorRescanAt = now;
    crt.resetDismissedMissingPrompts();
    void desktop.rescanLibrary();
  });

  // 6. Keyboard & Browser Drag-and-Drop Support
  window.addEventListener('keydown', (e) => {
    if (e.target instanceof HTMLElement && ['INPUT', 'TEXTAREA'].includes(e.target.tagName)) return;
    if (e.code === 'Space') {
      e.preventDefault();
      document.getElementById('rt01-play-btn')?.classList.add('down');
      foley.pressSwitch(true);
    } else if (e.code === 'ArrowRight') {
      e.preventDefault();
      tuneAngle += 18;
      syncRotors();
      foley.stepRotary(1, 1);
      crt.stepCurrentView(1);
    } else if (e.code === 'ArrowLeft') {
      e.preventDefault();
      tuneAngle -= 18;
      syncRotors();
      foley.stepRotary(-1, 1);
      crt.stepCurrentView(-1);
    } else if (e.code === 'Enter') {
      e.preventDefault();
      foley.pressSwitch(true);
      crt.activateCurrentView();
    } else if (e.code === 'KeyM') {
      foley.pressSwitch(false);
      crt.cycleMode();
    } else if (e.code === 'KeyO' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      foley.pressSwitch(false);
      void triggerImportFlow('files');
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

  // Browser-mode HTML5 drag-drop fallback (Native Tauri uses Webview.onDragDropEvent in desktop.ts)
  if (!desktop.isDesktop) {
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
        void player.ingestBrowserFiles(files);
      }
    });
  }

  // 7. Bind Native Tauri Shell Bridge & Persisted Library
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
    onOpenSettings: () => {
      foley.pressSwitch(false);
      crt.setMode('settings', true);
    },
    onLibraryUpdated: (state, fromUserImport) => {
      if (fromUserImport) {
        foley.latchBand();
        crt.pulseBeam();
      }
      // Do NOT auto-play after importing music
      player.setCatalog(state.albums, state.sources, state.selectAlbumId, false);
    },
  });
}

void bootstrap();
