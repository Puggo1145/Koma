// DEEP MODULE 5: MICRO-CRT RASTER DISPLAY & MULTI-PACK LIBRARY MANAGER
// Manages Empty / Cover / Bilingual Lyrics / Track Menu / Multi-Pack Settings views,
// strict per-screen rotary dial focus & actions, layout-safe lyric scrolling, and phosphor scope.

import { desktop, type ImportKind } from '../core/desktop';
import { foley } from '../core/foley';
import { DEFAULT_COVER_DATA_URI, type PlayerSnapshot } from '../core/player';
import { bindMicroFoley } from './controls';

export type CrtMode = 'cover' | 'lyrics' | 'menu' | 'settings';

interface SettingsCarouselItem {
  key: string;
  title: string;
  coverUrl: string;
  trackCount: number;
  albumIdx: number;
  sourceId: string;
  tooltip: string;
  enabled: boolean;
  missing: boolean;
  isCurrentAlbum: boolean;
  isPlayingNow: boolean;
}

export interface CrtCallbacks {
  onStepTrack: (dir: 1 | -1) => void;
  onSelectTrack: (albumIdx: number, trackIdx: number) => void;
  onSeekLyric: (lineIdx: number) => void;
  onCycleAlbum: () => void;
  onRequestImport: (kind: ImportKind) => void;
  onToggleSource: (sourceId: string) => void;
  onReimportSource: (sourceId: string) => void;
  onRevealSource: (sourceId: string) => void;
  onRemoveSource: (sourceId: string) => void;
  onRescanLibrary: () => void;
  onClearLibrary: () => void;
}

const ALL_MODES: CrtMode[] = ['cover', 'lyrics', 'menu', 'settings'];

export class MicroCrtDisplay {
  private mode: CrtMode = 'cover';
  private coverExpanded = false;
  private emptyCursorIdx = 0;
  private lyricCursorIdx: number | null = null;
  private menuAlbumIdx = 0;
  private menuCursorIdx = 0;
  private settingsCarouselIdx = 0;
  private settingsFocusedKey = '';
  private prevCarouselKeys = new Set<string>();
  private pendingFollowSourceId: string | null = null;
  private suppressNextCarouselClick = false;

  private renderedTrackKey = '';
  private prevActiveLyricIdx = -1;
  private userLyricScrollUntil = 0;
  private volOsdTimer: number | null = null;
  private confirmingClear = false;
  private clearResetTimer: number | null = null;
  private dismissedMissingIds = new Set<string>();
  private activeMissingSourceId: string | null = null;

  private latestSnap: PlayerSnapshot | null = null;
  private smoothAmp = 0;
  private rafId: number | null = null;

  private readonly callbacks: CrtCallbacks;

  constructor(callbacks: CrtCallbacks) {
    this.callbacks = callbacks;
    this.bindScreenElements();
    this.syncEmptyCursorDom();

    document.addEventListener('visibilitychange', () => {
      this.evaluateScopeLoop();
    });
  }

  getMode(): CrtMode {
    return this.mode;
  }

  getDialNotchThresholdDeg(): number {
    if (!this.latestSnap?.hasAnySource) return 18;
    return this.mode === 'cover' ? 18 : 15;
  }

  setMode(next: CrtMode, withPulse = true): void {
    if (this.mode === next) {
      if (next === 'settings') this.renderSettings();
      return;
    }
    this.mode = next;

    if (next !== 'settings' && this.confirmingClear) {
      this.resetClearConfirmation();
    }
    if (next !== 'lyrics') {
      this.clearLyricCursor();
    }

    if (next === 'menu' && this.latestSnap?.hasLibrary) {
      this.menuAlbumIdx = this.latestSnap.albumIdx;
      this.menuCursorIdx = this.latestSnap.trackIdx;
      this.renderMenu();
    } else if (next === 'settings') {
      this.syncCarouselToCurrentAlbum();
      this.renderSettings();
    }

    this.syncModeVisibility();

    if (withPulse) this.pulseBeam();
    if (next === 'lyrics') {
      this.userLyricScrollUntil = 0;
      requestAnimationFrame(() => this.scrollActiveLyricIntoView(false));
    }
    this.evaluateScopeLoop();
  }

  cycleMode(): void {
    if (!this.latestSnap?.hasAnySource) return;
    const next = ALL_MODES[(ALL_MODES.indexOf(this.mode) + 1) % ALL_MODES.length];
    this.setMode(next, true);
  }

  // Strictly executes the slide/rotate action belonging ONLY to the currently visible view
  stepCurrentView(dir: 1 | -1): void {
    if (!this.latestSnap?.hasAnySource) {
      const btns = this.getEmptyButtons();
      if (!btns.length) return;
      this.emptyCursorIdx = (this.emptyCursorIdx + dir + btns.length) % btns.length;
      this.syncEmptyCursorDom();
      return;
    }

    switch (this.mode) {
      case 'cover':
        if (this.latestSnap.hasLibrary) {
          this.callbacks.onStepTrack(dir);
        }
        break;
      case 'lyrics':
        this.stepLyricCursor(dir);
        break;
      case 'menu':
        this.stepMenuCursor(dir);
        break;
      case 'settings':
        this.stepSettingsCarousel(dir);
        break;
    }
  }

  // Strictly executes the press/confirm action belonging ONLY to the currently visible view
  activateCurrentView(): void {
    if (!this.latestSnap?.hasAnySource) {
      const btns = this.getEmptyButtons();
      btns[this.emptyCursorIdx]?.click();
      return;
    }

    switch (this.mode) {
      case 'cover':
        this.toggleCoverExpanded();
        break;
      case 'lyrics':
        this.confirmLyricCursor();
        break;
      case 'menu':
        this.confirmMenuCursor();
        break;
      case 'settings':
        this.confirmSettingsCarousel();
        break;
    }
  }

  toggleCoverExpanded(): void {
    if (!this.latestSnap?.hasLibrary) return;
    this.coverExpanded = !this.coverExpanded;
    this.pulseBeam();
    document.getElementById('crt-cover-layout')?.classList.toggle('expanded', this.coverExpanded);
    if (!this.coverExpanded) {
      this.drawScopeFrame(performance.now());
    }
    this.evaluateScopeLoop();
  }

  pulseBeam(): void {
    const flash = document.getElementById('crt-beam-flash');
    if (!flash) return;
    flash.classList.remove('pulse');
    void flash.offsetWidth;
    flash.classList.add('pulse');
  }

  flashVolume(volume: number): void {
    const osd = document.getElementById('crt-vol-osd');
    const bars = document.getElementById('crt-vol-bars');
    const num = document.getElementById('crt-vol-num');
    if (!osd || !bars || !num) return;

    const pct = Math.round(volume * 100);
    const filled = Math.round(volume * 10);
    bars.textContent = '▮'.repeat(filled) + '▯'.repeat(10 - filled);
    num.textContent = String(pct).padStart(2, '0');

    osd.classList.add('visible');
    if (this.volOsdTimer !== null) window.clearTimeout(this.volOsdTimer);
    this.volOsdTimer = window.setTimeout(() => osd.classList.remove('visible'), 1100);
  }

  renderTrackSnapshot(snap: PlayerSnapshot): void {
    this.latestSnap = snap;
    const unit = document.getElementById('rt01-unit');
    const tube = document.getElementById('crt-tube');
    tube?.classList.toggle('is-empty', !snap.hasAnySource);

    if (snap.hasAnySource && !snap.hasLibrary && this.mode !== 'settings') {
      this.mode = 'settings';
      this.syncModeVisibility();
    } else if (!snap.hasAnySource && this.mode === 'settings') {
      this.mode = 'cover';
      this.syncModeVisibility();
    }

    if (!snap.hasAnySource) {
      this.syncEmptyCursorDom();
    }

    if (unit) {
      unit.style.setProperty('--crt-fg', snap.album?.phosphor || '#dff2ff');
      unit.style.setProperty('--crt-glow', snap.album?.glow || 'rgba(130, 202, 255, 0.44)');
    }

    const freqText = document.getElementById('crt-freq-text');
    if (freqText) {
      freqText.textContent = snap.album
        ? `${String(snap.albumIdx + 1).padStart(2, '0')} · ${snap.album.freq}`
        : '待机';
    }

    document.getElementById('crt-lock-dot')?.classList.toggle('playing', snap.playing);

    const coverImg = document.getElementById('crt-cover-img') as HTMLImageElement | null;
    const ambBg = document.getElementById('crt-ambient-bg');
    if (coverImg && coverImg.getAttribute('src') !== snap.coverUrl) {
      coverImg.setAttribute('src', snap.coverUrl);
    }
    if (ambBg) {
      ambBg.style.backgroundImage = snap.hasLibrary ? `url("${snap.coverUrl}")` : 'none';
    }

    const trkNo = document.getElementById('crt-track-no');
    const songTitle = document.getElementById('crt-song-title');
    const albumTitle = document.getElementById('crt-album-title');
    const fullCap = document.getElementById('crt-fullart-title');

    if (trkNo) {
      trkNo.textContent = snap.album
        ? `${String(snap.trackIdx + 1).padStart(2, '0')} / ${String(
            snap.album.tracks.length
          ).padStart(2, '0')}`
        : '-- / --';
    }
    if (songTitle) songTitle.textContent = snap.displayTitle;

    const artistAlbumLine =
      snap.displayArtist && snap.displayArtist !== snap.displayAlbum
        ? `${snap.displayArtist} · ${snap.displayAlbum}`
        : snap.displayAlbum;

    if (albumTitle) albumTitle.textContent = artistAlbumLine;
    if (fullCap) {
      fullCap.textContent = artistAlbumLine
        ? `${snap.displayTitle} · ${artistAlbumLine}`
        : snap.displayTitle;
    }

    if (this.menuAlbumIdx >= snap.albums.length) {
      this.menuAlbumIdx = Math.max(0, snap.albumIdx);
      this.menuCursorIdx = 0;
    }

    const firstLyricSample = snap.lyrics[0]?.text ?? '';
    const lastLyricSample = snap.lyrics[snap.lyrics.length - 1]?.text ?? '';
    const nextKey = snap.album
      ? `${snap.album.id}:${snap.trackIdx}:${snap.displayTitle}:${snap.lyrics.length}:${firstLyricSample}:${lastLyricSample}`
      : 'empty';
    if (nextKey !== this.renderedTrackKey) {
      this.renderedTrackKey = nextKey;
      this.prevActiveLyricIdx = -1;
      this.lyricCursorIdx = null;
      this.userLyricScrollUntil = 0;
      this.menuAlbumIdx = snap.albumIdx;
      this.menuCursorIdx = snap.trackIdx;
      this.rebuildLyricsDom(snap);
    }

    this.renderMenu();
    this.renderSettings();
    this.syncMissingPrompt(snap);
    this.renderProgressSnapshot(snap);
    this.drawScopeFrame(performance.now());
    this.evaluateScopeLoop();
  }

  resetDismissedMissingPrompts(): void {
    this.dismissedMissingIds.clear();
  }

  private syncMissingPrompt(snap: PlayerSnapshot): void {
    const stillMissingIds = new Set(
      snap.sources.filter((s) => s.missing).map((s) => s.id)
    );
    for (const id of Array.from(this.dismissedMissingIds)) {
      if (!stillMissingIds.has(id)) {
        this.dismissedMissingIds.delete(id);
      }
    }

    const missingSrc = desktop.isDesktop
      ? snap.sources.find(
          (s) => s.enabled && s.missing && !this.dismissedMissingIds.has(s.id)
        )
      : undefined;

    const promptEl = document.getElementById('crt-missing-prompt') as HTMLElement | null;
    const nameEl = document.getElementById('crt-missing-name');
    if (!promptEl) return;

    if (!missingSrc) {
      this.activeMissingSourceId = null;
      promptEl.hidden = true;
      return;
    }

    this.activeMissingSourceId = missingSrc.id;
    if (nameEl) {
      nameEl.textContent = missingSrc.name;
      nameEl.title = missingSrc.originPath || missingSrc.path || missingSrc.name;
    }
    promptEl.hidden = false;
  }

  renderProgressSnapshot(snap: PlayerSnapshot): void {
    this.latestSnap = snap;
    const curEl = document.getElementById('crt-time-cur');
    const durEl = document.getElementById('crt-time-dur');
    if (curEl) curEl.textContent = formatSec(snap.currentTime);
    if (durEl) durEl.textContent = formatSec(snap.duration);

    if (snap.activeLyricIdx !== this.prevActiveLyricIdx) {
      const container = document.getElementById('crt-lyrics-scroll');
      if (container) {
        if (this.prevActiveLyricIdx >= 0) {
          container.children[this.prevActiveLyricIdx]?.classList.remove('active');
        }
        if (snap.activeLyricIdx >= 0) {
          container.children[snap.activeLyricIdx]?.classList.add('active');
        }
      }
      this.prevActiveLyricIdx = snap.activeLyricIdx;
      if (this.mode === 'lyrics' && this.lyricCursorIdx === null) {
        this.scrollActiveLyricIntoView(true);
      }
    }
  }

  // --- Per-View Navigation Mechanics ---

  private getEmptyButtons(): HTMLElement[] {
    return Array.from(document.querySelectorAll<HTMLElement>('#crt-empty-state button'));
  }

  private syncEmptyCursorDom(): void {
    const btns = this.getEmptyButtons();
    if (!btns.length) return;
    if (this.emptyCursorIdx >= btns.length) this.emptyCursorIdx = 0;
    btns.forEach((b, idx) => b.classList.toggle('dial-cursor', idx === this.emptyCursorIdx));
  }

  private stepLyricCursor(dir: 1 | -1): void {
    const lines = this.latestSnap?.lyrics ?? [];
    if (!lines.length) return;

    const base =
      this.lyricCursorIdx !== null
        ? this.lyricCursorIdx
        : Math.max(0, this.latestSnap?.activeLyricIdx ?? 0);
    const next = Math.max(0, Math.min(lines.length - 1, base + dir));
    this.lyricCursorIdx = next;

    const container = document.getElementById('crt-lyrics-scroll');
    if (!container) return;

    Array.from(container.children).forEach((el, idx) => {
      el.classList.toggle('cursor', idx === next);
    });

    const cursorEl = container.children[next] as HTMLElement | undefined;
    if (cursorEl && container.clientHeight > 0) {
      const top = cursorEl.offsetTop - container.clientHeight / 2 + cursorEl.clientHeight / 2;
      container.scrollTo({ top: Math.max(0, top), behavior: 'smooth' });
    }
  }

  private confirmLyricCursor(): void {
    const lines = this.latestSnap?.lyrics ?? [];
    if (!lines.length) return;

    const targetIdx =
      this.lyricCursorIdx !== null
        ? this.lyricCursorIdx
        : Math.max(0, this.latestSnap?.activeLyricIdx ?? 0);

    this.clearLyricCursor();
    this.userLyricScrollUntil = 0;
    this.pulseBeam();
    this.callbacks.onSeekLyric(targetIdx);
  }

  private clearLyricCursor(): void {
    this.lyricCursorIdx = null;
    document
      .querySelectorAll('#crt-lyrics-scroll .crt-lyric-line.cursor')
      .forEach((el) => el.classList.remove('cursor'));
  }

  private stepMenuCursor(delta: 1 | -1): void {
    if (!this.latestSnap?.hasLibrary) return;
    const albums = this.latestSnap.albums;
    const curAlbum = albums[this.menuAlbumIdx];
    if (!curAlbum) return;

    let nextTrack = this.menuCursorIdx + delta;
    let nextAlbum = this.menuAlbumIdx;

    if (nextTrack >= curAlbum.tracks.length) {
      nextAlbum = (this.menuAlbumIdx + 1) % albums.length;
      nextTrack = 0;
      if (nextAlbum !== this.menuAlbumIdx) {
        foley.latchBand();
        this.pulseBeam();
      }
    } else if (nextTrack < 0) {
      nextAlbum = (this.menuAlbumIdx - 1 + albums.length) % albums.length;
      nextTrack = albums[nextAlbum].tracks.length - 1;
      if (nextAlbum !== this.menuAlbumIdx) {
        foley.latchBand();
        this.pulseBeam();
      }
    }

    this.menuAlbumIdx = nextAlbum;
    this.menuCursorIdx = nextTrack;
    this.renderMenu();

    document
      .querySelector<HTMLElement>('#crt-menu-albums .crt-album-chip.selected')
      ?.scrollIntoView({ inline: 'nearest', block: 'nearest' });
    document
      .querySelector<HTMLElement>('#crt-menu-tracks .crt-track-row.cursor')
      ?.scrollIntoView({ block: 'nearest' });
  }

  private confirmMenuCursor(): void {
    if (!this.latestSnap?.hasLibrary) return;
    this.callbacks.onSelectTrack(this.menuAlbumIdx, this.menuCursorIdx);
  }

  private buildSettingsCarouselItems(): SettingsCarouselItem[] {
    const snap = this.latestSnap;
    if (!snap) return [];

    const sourceById = new Map(snap.sources.map((s) => [s.id, s]));
    const coveredSourceIds = new Set<string>();
    const items: SettingsCarouselItem[] = [];

    snap.albums.forEach((alb, idx) => {
      coveredSourceIds.add(alb.sourceId);
      const src = sourceById.get(alb.sourceId);
      const rawCover = alb.cover || alb.tracks[0]?.cover || src?.cover || '';
      const coverUrl = rawCover ? desktop.resolveMediaUrl(rawCover) : DEFAULT_COVER_DATA_URI;
      const isCurrentAlbum = snap.hasLibrary && snap.albumIdx === idx;

      items.push({
        key: `alb:${alb.id}`,
        title: alb.title,
        coverUrl: coverUrl || DEFAULT_COVER_DATA_URI,
        trackCount: alb.tracks.length,
        albumIdx: idx,
        sourceId: alb.sourceId,
        tooltip: src?.originPath || src?.path || alb.title,
        enabled: true,
        missing: false,
        isCurrentAlbum,
        isPlayingNow: isCurrentAlbum && snap.playing,
      });
    });

    // Append disabled / missing / empty sources so they remain visible & toggleable on the shelf
    snap.sources.forEach((src) => {
      if (coveredSourceIds.has(src.id)) return;
      const coverUrl = src.cover ? desktop.resolveMediaUrl(src.cover) : DEFAULT_COVER_DATA_URI;
      items.push({
        key: `src:${src.id}`,
        title: src.name,
        coverUrl: coverUrl || DEFAULT_COVER_DATA_URI,
        trackCount: src.trackCount,
        albumIdx: -1,
        sourceId: src.id,
        tooltip: src.originPath || src.path || src.name,
        enabled: src.enabled,
        missing: src.missing,
        isCurrentAlbum: false,
        isPlayingNow: false,
      });
    });

    return items;
  }

  private syncCarouselToCurrentAlbum(): void {
    const items = this.buildSettingsCarouselItems();
    if (!items.length) {
      this.settingsCarouselIdx = 0;
      this.settingsFocusedKey = '';
      return;
    }
    const curAlbIdx = this.latestSnap?.albumIdx ?? 0;
    const found = items.findIndex((it) => it.albumIdx === curAlbIdx);
    this.settingsCarouselIdx = found >= 0 ? found : Math.min(this.settingsCarouselIdx, items.length - 1);
    this.settingsFocusedKey = items[this.settingsCarouselIdx]?.key ?? '';
  }

  private stepSettingsCarousel(dir: 1 | -1): void {
    const items = this.buildSettingsCarouselItems();
    if (!items.length) return;

    const next = Math.max(0, Math.min(items.length - 1, this.settingsCarouselIdx + dir));
    if (next === this.settingsCarouselIdx) return;

    this.settingsCarouselIdx = next;
    this.settingsFocusedKey = items[next]?.key ?? '';
    this.updateCarouselDom(items);
  }

  private confirmSettingsCarousel(): void {
    const items = this.buildSettingsCarouselItems();
    const item = items[this.settingsCarouselIdx];
    if (!item) return;

    if (item.albumIdx >= 0) {
      foley.latchBand();
      this.pulseBeam();
      this.callbacks.onSelectTrack(item.albumIdx, 0);
    } else if (item.missing) {
      this.pendingFollowSourceId = item.sourceId;
      this.callbacks.onReimportSource(item.sourceId);
    } else if (!item.enabled) {
      this.pendingFollowSourceId = item.sourceId;
      foley.latchBand();
      this.pulseBeam();
      this.callbacks.onToggleSource(item.sourceId);
    }
  }

  // --- Internal View & Scope Mechanics ---

  private syncModeVisibility(): void {
    document.querySelectorAll<HTMLElement>('.crt-tab[data-mode]').forEach((tab) => {
      tab.classList.toggle('active', tab.dataset.mode === this.mode);
    });
    ALL_MODES.forEach((m) => {
      document.getElementById(`view-${m}`)?.classList.toggle('active', this.mode === m);
    });
  }

  private resetClearConfirmation(): void {
    this.confirmingClear = false;
    if (this.clearResetTimer !== null) {
      window.clearTimeout(this.clearResetTimer);
      this.clearResetTimer = null;
    }
  }

  private bindScreenElements(): void {
    document.querySelectorAll<HTMLElement>('.crt-tab[data-mode]').forEach((btn) => {
      bindMicroFoley(btn, false);
      btn.addEventListener('click', () => {
        const target = btn.dataset.mode as CrtMode | undefined;
        if (target) this.setMode(target, true);
      });
    });

    document.querySelectorAll<HTMLElement>('[data-import-kind]').forEach((btn, idx) => {
      bindMicroFoley(btn, false);
      btn.addEventListener('pointerenter', () => {
        this.emptyCursorIdx = idx;
        this.syncEmptyCursorDom();
      });
      btn.addEventListener('click', () => {
        const kind = (btn.dataset.importKind ?? 'files') as ImportKind;
        this.callbacks.onRequestImport(kind);
      });
    });

    const bandBtn = document.getElementById('crt-band-btn');
    bindMicroFoley(bandBtn, false);
    bandBtn?.addEventListener('click', () => {
      if (this.latestSnap?.hasLibrary) {
        this.callbacks.onCycleAlbum();
      }
    });

    const artFrame = document.getElementById('crt-art-frame');
    bindMicroFoley(artFrame, false);
    artFrame?.addEventListener('click', () => {
      this.toggleCoverExpanded();
    });

    const coverImg = document.getElementById('crt-cover-img') as HTMLImageElement | null;
    coverImg?.addEventListener('error', () => {
      if (this.latestSnap && coverImg.src !== this.latestSnap.coverUrl) {
        coverImg.src = this.latestSnap.coverUrl;
      }
    });

    document.getElementById('crt-lyrics-scroll')?.addEventListener(
      'wheel',
      () => {
        this.userLyricScrollUntil = performance.now() + 2400;
      },
      { passive: true }
    );

    const relocateBtn = document.getElementById('crt-missing-relocate');
    const removeMissingBtn = document.getElementById('crt-missing-remove');
    const dismissMissingBtn = document.getElementById('crt-missing-dismiss');
    bindMicroFoley(relocateBtn, false);
    bindMicroFoley(removeMissingBtn, false);
    bindMicroFoley(dismissMissingBtn, false);

    relocateBtn?.addEventListener('click', () => {
      if (!this.activeMissingSourceId) return;
      this.pendingFollowSourceId = this.activeMissingSourceId;
      this.callbacks.onReimportSource(this.activeMissingSourceId);
    });

    removeMissingBtn?.addEventListener('click', () => {
      if (!this.activeMissingSourceId) return;
      const id = this.activeMissingSourceId;
      this.dismissedMissingIds.delete(id);
      foley.latchBand();
      this.pulseBeam();
      this.callbacks.onRemoveSource(id);
    });

    dismissMissingBtn?.addEventListener('click', () => {
      if (!this.activeMissingSourceId) return;
      this.dismissedMissingIds.add(this.activeMissingSourceId);
      foley.pressSwitch(false);
      if (this.latestSnap) {
        this.syncMissingPrompt(this.latestSnap);
      }
    });

    this.bindCarouselStageGestures();
  }

  private bindCarouselStageGestures(): void {
    const stage = document.getElementById('crt-carousel-stage');
    if (!stage) return;

    let wheelAcc = 0;
    stage.addEventListener(
      'wheel',
      (e) => {
        if (this.mode !== 'settings') return;
        const dominant = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
        if (dominant === 0) return;
        e.preventDefault();
        wheelAcc += dominant;
        if (Math.abs(wheelAcc) >= 28) {
          const dir: 1 | -1 = wheelAcc > 0 ? 1 : -1;
          wheelAcc = 0;
          const prev = this.settingsCarouselIdx;
          this.stepSettingsCarousel(dir);
          if (this.settingsCarouselIdx !== prev) {
            foley.stepRotary(dir, 1);
          }
        }
      },
      { passive: false }
    );

    let dragPointerId: number | null = null;
    let lastX = 0;
    let dragAcc = 0;
    let dragged = false;

    stage.addEventListener('pointerdown', (e) => {
      if (e.button !== 0 || this.mode !== 'settings') return;
      dragPointerId = e.pointerId;
      lastX = e.clientX;
      dragAcc = 0;
      dragged = false;
      this.suppressNextCarouselClick = false;
    });

    stage.addEventListener('pointermove', (e) => {
      if (dragPointerId !== e.pointerId) return;
      const dx = e.clientX - lastX;
      lastX = e.clientX;
      dragAcc += dx;

      if (!dragged && Math.abs(dragAcc) > 6) {
        dragged = true;
        this.suppressNextCarouselClick = true;
        try {
          stage.setPointerCapture(e.pointerId);
        } catch {
          // Ignore capture failure
        }
      }

      const stepPx = 24;
      while (Math.abs(dragAcc) >= stepPx) {
        // Dragging shelf left brings right-hand album into center (+1); dragging right brings left-hand album (-1)
        const dir: 1 | -1 = dragAcc < 0 ? 1 : -1;
        dragAcc += dragAcc < 0 ? stepPx : -stepPx;
        const prev = this.settingsCarouselIdx;
        this.stepSettingsCarousel(dir);
        if (this.settingsCarouselIdx !== prev) {
          foley.stepRotary(dir, 1);
        }
      }
    });

    const endDrag = (e: PointerEvent) => {
      if (dragPointerId !== e.pointerId) return;
      if (dragged) {
        try {
          stage.releasePointerCapture(e.pointerId);
        } catch {
          // Ignore release failure
        }
        window.setTimeout(() => {
          this.suppressNextCarouselClick = false;
        }, 60);
      }
      dragPointerId = null;
      dragged = false;
    };

    stage.addEventListener('pointerup', endDrag);
    stage.addEventListener('pointercancel', endDrag);
  }

  private rebuildLyricsDom(snap: PlayerSnapshot): void {
    const container = document.getElementById('crt-lyrics-scroll');
    if (!container) return;

    container.innerHTML = snap.lyrics
      .map((line, i) => {
        const sub = line.subtext
          ? `<div class="crt-lyric-sub">${escapeHtml(line.subtext)}</div>`
          : '';
        return `<div class="crt-lyric-line ${i === snap.activeLyricIdx ? 'active' : ''}" data-lidx="${i}">
          <div class="crt-lyric-main">${escapeHtml(line.text)}</div>
          ${sub}
        </div>`;
      })
      .join('');

    container.scrollTop = 0;
    this.prevActiveLyricIdx = snap.activeLyricIdx;

    container.querySelectorAll<HTMLElement>('.crt-lyric-line').forEach((el) => {
      bindMicroFoley(el, false);
      el.addEventListener('click', () => {
        const idx = Number(el.dataset.lidx ?? 0);
        this.clearLyricCursor();
        this.userLyricScrollUntil = 0;
        this.pulseBeam();
        this.callbacks.onSeekLyric(idx);
      });
    });

    if (this.mode === 'lyrics') {
      requestAnimationFrame(() => this.scrollActiveLyricIntoView(false));
    }
  }

  private scrollActiveLyricIntoView(smooth: boolean): void {
    if (this.lyricCursorIdx !== null || performance.now() < this.userLyricScrollUntil) return;
    const container = document.getElementById('crt-lyrics-scroll');
    const activeEl = container?.querySelector<HTMLElement>('.crt-lyric-line.active');
    if (!container || !activeEl || container.clientHeight <= 0) return;

    const top = activeEl.offsetTop - container.clientHeight / 2 + activeEl.clientHeight / 2;
    container.scrollTo({ top: Math.max(0, top), behavior: smooth ? 'smooth' : 'auto' });
  }

  private renderMenu(): void {
    const albBar = document.getElementById('crt-menu-albums');
    const trkBox = document.getElementById('crt-menu-tracks');
    if (!albBar || !trkBox) return;

    if (!this.latestSnap?.hasLibrary) {
      albBar.innerHTML = '';
      trkBox.innerHTML = '';
      return;
    }

    const snap = this.latestSnap;
    albBar.innerHTML =
      snap.albums
        .map((alb, aIdx) => {
          const isSelected = aIdx === this.menuAlbumIdx;
          return `
          <div class="crt-album-chip ${isSelected ? 'selected' : ''}">
            <button class="crt-album-pill ${isSelected ? 'selected' : ''}" data-malb="${aIdx}" title="${escapeHtml(alb.title)}">
              ${escapeHtml(alb.freq)} · ${escapeHtml(alb.title)}
            </button>
          </div>
        `;
        })
        .join('') +
      `<button class="crt-album-add" id="crt-menu-add-btn" title="导入音乐">+</button>`;

    albBar.querySelectorAll<HTMLElement>('.crt-album-pill').forEach((btn) => {
      bindMicroFoley(btn, false);
      btn.addEventListener('click', () => {
        const idx = Number(btn.dataset.malb ?? 0);
        if (idx !== this.menuAlbumIdx) {
          this.menuAlbumIdx = idx;
          this.menuCursorIdx = 0;
          foley.latchBand();
          this.pulseBeam();
          this.renderMenu();
        }
      });
    });

    const addBtn = document.getElementById('crt-menu-add-btn');
    bindMicroFoley(addBtn, false);
    addBtn?.addEventListener('click', () => {
      this.callbacks.onRequestImport('files');
    });

    const shownAlbum = snap.albums[this.menuAlbumIdx] ?? snap.albums[0];
    if (!shownAlbum) return;

    trkBox.innerHTML = shownAlbum.tracks
      .map((tr, tIdx) => {
        const isCurTrack = snap.albumIdx === this.menuAlbumIdx && snap.trackIdx === tIdx;
        const isCursor = tIdx === this.menuCursorIdx;
        const marker = isCurTrack ? (snap.playing ? '▶' : '●') : String(tIdx + 1).padStart(2, '0');
        return `
          <button class="crt-track-row ${isCurTrack ? 'playing' : ''} ${isCursor ? 'cursor' : ''}" data-mtrk="${tIdx}">
            <span class="crt-track-row-left">
              <span style="width:15px;display:inline-block;font-family:var(--font-mono);">${marker}</span>
              <span class="crt-track-row-title">${escapeHtml(tr.title)}</span>
            </span>
            <span class="crt-track-row-dur">${escapeHtml(tr.fullDur)}</span>
          </button>
        `;
      })
      .join('');

    trkBox.querySelectorAll<HTMLElement>('.crt-track-row').forEach((row) => {
      bindMicroFoley(row, false);
      row.addEventListener('pointerenter', () => {
        this.menuCursorIdx = Number(row.dataset.mtrk ?? 0);
        trkBox.querySelectorAll('.crt-track-row').forEach((r, idx) => {
          r.classList.toggle('cursor', idx === this.menuCursorIdx);
        });
      });
      row.addEventListener('click', () => {
        const tIdx = Number(row.dataset.mtrk ?? 0);
        this.callbacks.onSelectTrack(this.menuAlbumIdx, tIdx);
      });
    });
  }

  private renderSettings(): void {
    const bar = document.getElementById('crt-mgr-bar');
    if (!bar) return;

    const sources = this.latestSnap?.sources ?? [];
    const showDesktopOps = desktop.isDesktop;

    bar.innerHTML = `
      <div class="crt-mgr-bar-left">
        <button class="crt-mgr-btn primary" data-mgr-act="add-files" title="导入歌曲压缩包或音频文件">+ 导入</button>
        <button class="crt-mgr-btn" data-mgr-act="add-folder" title="添加本地音乐文件夹">+ 文件夹</button>
      </div>
      <div class="crt-mgr-bar-right">
        ${showDesktopOps ? `<button class="crt-mgr-btn ghost" data-mgr-act="rescan" title="刷新曲库">刷新</button>` : ''}
        ${
          sources.length > 0
            ? `<button class="crt-mgr-btn ghost danger ${this.confirmingClear ? 'confirm' : ''}" data-mgr-act="clear">${
                this.confirmingClear ? '确认' : '清空'
              }</button>`
            : ''
        }
      </div>
    `;

    bar.querySelectorAll<HTMLElement>('[data-mgr-act]').forEach((btn) => {
      bindMicroFoley(btn, false);
      btn.addEventListener('click', () => {
        const act = btn.dataset.mgrAct;
        if (act === 'add-files') {
          this.callbacks.onRequestImport('files');
        } else if (act === 'add-folder') {
          this.callbacks.onRequestImport('folder');
        } else if (act === 'rescan') {
          this.dismissedMissingIds.clear();
          foley.latchBand();
          this.pulseBeam();
          this.callbacks.onRescanLibrary();
        } else if (act === 'clear') {
          if (!this.confirmingClear) {
            this.confirmingClear = true;
            if (this.clearResetTimer !== null) window.clearTimeout(this.clearResetTimer);
            this.clearResetTimer = window.setTimeout(() => {
              this.confirmingClear = false;
              this.renderSettings();
            }, 2400);
            this.renderSettings();
          } else {
            this.resetClearConfirmation();
            foley.latchBand();
            this.pulseBeam();
            this.callbacks.onClearLibrary();
          }
        }
      });
    });

    const items = this.buildSettingsCarouselItems();
    if (!items.length) {
      this.settingsCarouselIdx = 0;
      this.settingsFocusedKey = '';
      this.pendingFollowSourceId = null;
    } else {
      if (this.pendingFollowSourceId) {
        const bySrc = items.findIndex((it) => it.sourceId === this.pendingFollowSourceId);
        if (bySrc >= 0) {
          this.settingsCarouselIdx = bySrc;
        }
        this.pendingFollowSourceId = null;
      } else if (
        this.mode === 'settings' &&
        this.prevCarouselKeys.size > 0 &&
        items.length > this.prevCarouselKeys.size
      ) {
        const addedIdx = items.findIndex((it) => !this.prevCarouselKeys.has(it.key));
        if (addedIdx >= 0) {
          this.settingsCarouselIdx = addedIdx;
        }
      } else if (this.settingsFocusedKey) {
        const byKey = items.findIndex((it) => it.key === this.settingsFocusedKey);
        if (byKey >= 0) {
          this.settingsCarouselIdx = byKey;
        }
      }
      this.settingsCarouselIdx = Math.max(0, Math.min(items.length - 1, this.settingsCarouselIdx));
      this.settingsFocusedKey = items[this.settingsCarouselIdx]?.key ?? '';
    }
    this.prevCarouselKeys = new Set(items.map((it) => it.key));

    this.updateCarouselDom(items);
  }

  private updateCarouselDom(items: SettingsCarouselItem[]): void {
    const trackEl = document.getElementById('crt-carousel-track');
    const footerEl = document.getElementById('crt-carousel-footer');
    if (!trackEl || !footerEl) return;

    if (!items.length) {
      trackEl.innerHTML = '';
      footerEl.innerHTML = '';
      return;
    }

    // Reconcile card elements by key so CSS 3D Cover Flow transitions animate smoothly at 60fps
    const existingByKey = new Map<string, HTMLButtonElement>();
    Array.from(trackEl.querySelectorAll<HTMLButtonElement>('.crt-cf-card')).forEach((el) => {
      const k = el.dataset.cfKey;
      if (k) existingByKey.set(k, el);
    });

    const seenKeys = new Set<string>();

    items.forEach((item, idx) => {
      seenKeys.add(item.key);
      let card = existingByKey.get(item.key);

      if (!card) {
        card = document.createElement('button');
        card.type = 'button';
        card.className = 'crt-cf-card';
        card.dataset.cfKey = item.key;
        card.innerHTML = `
          <img alt="" draggable="false" />
          <div class="crt-cf-scan"></div>
          <span class="crt-cf-badge" hidden></span>
        `;
        const img = card.querySelector('img') as HTMLImageElement;
        img.addEventListener('error', () => {
          if (img.src !== DEFAULT_COVER_DATA_URI) {
            img.src = DEFAULT_COVER_DATA_URI;
          }
        });
        bindMicroFoley(card, false);
        card.addEventListener('click', () => {
          if (this.suppressNextCarouselClick) return;
          const clickedIdx = Number(card?.dataset.cfIdx ?? 0);
          if (clickedIdx !== this.settingsCarouselIdx) {
            const dir: 1 | -1 = clickedIdx > this.settingsCarouselIdx ? 1 : -1;
            this.settingsCarouselIdx = clickedIdx;
            const latestItems = this.buildSettingsCarouselItems();
            this.settingsFocusedKey = latestItems[clickedIdx]?.key ?? '';
            foley.stepRotary(dir, 1);
            this.updateCarouselDom(latestItems);
          } else {
            this.confirmSettingsCarousel();
          }
        });
        trackEl.appendChild(card);
      }

      card.dataset.cfIdx = String(idx);
      card.title =
        idx === this.settingsCarouselIdx
          ? item.albumIdx >= 0
            ? `${item.title}（点击播放第 1 首）`
            : item.missing
              ? `${item.title}（原文件已移除，点击重新选择）`
              : `${item.title}（点击启用）`
          : item.title;

      const img = card.querySelector('img') as HTMLImageElement | null;
      if (img && img.getAttribute('src') !== item.coverUrl) {
        img.setAttribute('src', item.coverUrl);
      }

      const off = idx - this.settingsCarouselIdx;
      const absOff = Math.abs(off);
      const sig = off < 0 ? -1 : 1;

      card.classList.toggle('is-center', off === 0);
      card.classList.toggle('is-inactive', !item.enabled || item.missing);

      let tx = 0;
      let ty = 0;
      let tz = 0;
      let sc = 1;
      let ry = 0;
      let op = 1;
      let zIdx = 20 - absOff;

      if (off === 0) {
        tx = 0;
        ty = 0;
        tz = 0;
        sc = 1;
        ry = 0;
        op = 1;
      } else if (absOff === 1) {
        tx = sig * 44;
        ty = 1;
        tz = -32;
        sc = 0.76;
        ry = -sig * 38;
        op = 0.74;
      } else if (absOff === 2) {
        tx = sig * 69;
        ty = 2;
        tz = -60;
        sc = 0.58;
        ry = -sig * 46;
        op = 0.36;
      } else {
        tx = sig * 88;
        ty = 3;
        tz = -84;
        sc = 0.44;
        ry = -sig * 50;
        op = 0;
        zIdx = 1;
      }

      card.style.transform = `translate3d(${tx}px, ${ty}px, ${tz}px) scale(${sc}) rotateY(${ry}deg)`;
      card.style.opacity = String(op);
      card.style.zIndex = String(zIdx);
      card.style.pointerEvents = absOff <= 2 ? 'auto' : 'none';

      const badge = card.querySelector<HTMLElement>('.crt-cf-badge');
      if (badge) {
        if (item.missing) {
          badge.hidden = false;
          badge.className = 'crt-cf-badge muted';
          badge.textContent = '失效';
        } else if (!item.enabled) {
          badge.hidden = false;
          badge.className = 'crt-cf-badge muted';
          badge.textContent = '已停用';
        } else if (item.isCurrentAlbum) {
          badge.hidden = false;
          badge.className = `crt-cf-badge ${item.isPlayingNow ? 'playing' : ''}`;
          badge.textContent = item.isPlayingNow ? '▶' : '●';
        } else {
          badge.hidden = true;
          badge.textContent = '';
        }
      }
    });

    existingByKey.forEach((el, k) => {
      if (!seenKeys.has(k)) {
        el.remove();
      }
    });

    const focused = items[this.settingsCarouselIdx];
    if (!focused) {
      footerEl.innerHTML = '';
      return;
    }

    const posLabel = `${this.settingsCarouselIdx + 1}/${items.length}`;
    const stateLabel = focused.missing
      ? '未找到原文件'
      : !focused.enabled
        ? '已停用'
        : `${focused.trackCount}首`;
    const showDesktopOps = desktop.isDesktop;

    footerEl.innerHTML = `
      <div class="crt-cf-title" title="${escapeHtml(focused.tooltip)}">${escapeHtml(focused.title)}</div>
      <div class="crt-cf-subbar">
        <span class="crt-cf-meta">${escapeHtml(posLabel)} · ${escapeHtml(stateLabel)}</span>
        <div class="crt-cf-actions">
          ${
            focused.missing
              ? `${
                  showDesktopOps
                    ? `<button class="crt-cf-act primary" data-cf-act="reimport" title="重新选择音乐包位置">重新选择</button>`
                    : ''
                }
                <button class="crt-cf-act danger" data-cf-act="remove" title="从曲库移除">移除</button>`
              : `<button class="crt-cf-act" data-cf-act="toggle" title="${focused.enabled ? '停用该曲库包' : '启用该曲库包'}">${
                  focused.enabled ? '停用' : '启用'
                }</button>
                ${
                  showDesktopOps
                    ? `<button class="crt-cf-act icon" data-cf-act="reimport" title="刷新该曲库包">↻</button>
                       <button class="crt-cf-act icon" data-cf-act="reveal" title="打开所在目录">↗</button>`
                    : ''
                }
                <button class="crt-cf-act icon danger" data-cf-act="remove" title="移除该曲库包">×</button>`
          }
        </div>
      </div>
    `;

    footerEl.querySelectorAll<HTMLElement>('[data-cf-act]').forEach((btn) => {
      bindMicroFoley(btn, false);
      btn.addEventListener('click', () => {
        const act = btn.dataset.cfAct;
        const curItem = this.buildSettingsCarouselItems()[this.settingsCarouselIdx];
        if (!curItem) return;

        if (act === 'toggle') {
          this.pendingFollowSourceId = curItem.sourceId;
          foley.latchBand();
          this.pulseBeam();
          this.callbacks.onToggleSource(curItem.sourceId);
        } else if (act === 'reimport') {
          this.pendingFollowSourceId = curItem.sourceId;
          this.callbacks.onReimportSource(curItem.sourceId);
        } else if (act === 'reveal') {
          this.callbacks.onRevealSource(curItem.sourceId);
        } else if (act === 'remove') {
          foley.latchBand();
          this.pulseBeam();
          this.callbacks.onRemoveSource(curItem.sourceId);
        }
      });
    });
  }

  private evaluateScopeLoop(): void {
    const shouldAnimate =
      !document.hidden &&
      Boolean(this.latestSnap?.playing || this.smoothAmp > 0.004) &&
      ((this.mode === 'cover' && !this.coverExpanded) || Boolean(this.latestSnap?.playing));

    if (shouldAnimate && this.rafId === null) {
      const step = (now: number) => {
        this.rafId = null;
        this.drawScopeFrame(now);
        this.evaluateScopeLoop();
      };
      this.rafId = requestAnimationFrame(step);
    } else if (!shouldAnimate && this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
      this.drawScopeFrame(performance.now());
    }
  }

  private drawScopeFrame(now: number): void {
    const snap = this.latestSnap;
    if (!snap) return;

    const env = snap.track?.env?.length ? snap.track.env : [35];
    let targetAmp = 0;
    if (snap.playing) {
      const fIdx = Math.min(env.length - 1, Math.max(0, snap.progress * (env.length - 1)));
      const i0 = Math.floor(fIdx);
      const i1 = Math.min(env.length - 1, i0 + 1);
      const frac = fIdx - i0;
      const rawEnv = env[i0] * (1 - frac) + env[i1] * frac;
      targetAmp = (rawEnv / 100) * (0.45 + snap.volume * 0.55);
    }

    this.smoothAmp += (targetAmp - this.smoothAmp) * 0.22;
    if (!snap.playing && this.smoothAmp < 0.004) this.smoothAmp = 0;

    const cone = document.getElementById('rt01-cone');
    if (cone) {
      const s = 1 + this.smoothAmp * 0.065;
      cone.style.transform = `scale(${s.toFixed(3)})`;
    }

    if (!snap.hasLibrary || this.mode !== 'cover' || this.coverExpanded) return;

    const canvas = document.getElementById('crt-scope') as HTMLCanvasElement | null;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;

    const w = canvas.width;
    const h = canvas.height;
    const midY = h / 2;

    ctx.clearRect(0, 0, w, h);

    ctx.strokeStyle = 'rgba(220, 240, 255, 0.09)';
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, midY);
    ctx.lineTo(w, midY);
    ctx.stroke();

    const beamColor = snap.album?.phosphor || '#dff2ff';
    ctx.save();
    ctx.strokeStyle = beamColor;
    ctx.shadowColor = beamColor;
    ctx.shadowBlur = 8;
    ctx.lineWidth = 1.9;
    ctx.beginPath();

    const t = now * 0.0085;
    for (let x = 0; x < w; x += 2) {
      const nx = x / w;
      const win = Math.sin(nx * Math.PI);
      const wave =
        Math.sin(nx * 15.0 - t * 1.9) * 0.52 +
        Math.sin(nx * 31.0 + t * 2.7) * 0.31 +
        Math.cos(nx * 49.0 - t * 3.8) * 0.17;
      const y = midY + wave * win * this.smoothAmp * (h * 0.44);
      if (x === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();

    if (!snap.playing) {
      ctx.fillStyle = beamColor;
      ctx.globalAlpha = 0.78;
      ctx.beginPath();
      ctx.arc(w - 6, midY, 2.2, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();
  }
}

function formatSec(sec: number): string {
  const s = Math.max(0, Math.floor(sec || 0));
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}:${String(r).padStart(2, '0')}`;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}
