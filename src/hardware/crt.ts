// DEEP MODULE 5: MICRO-CRT RASTER DISPLAY & ZERO-IDLE-CPU PHOSPHOR OSCILLOSCOPE
// Manages Cover / Lyrics / OSD Menu views, diff-guarded DOM updates, electron-beam sync flashes,
// and an on-demand requestAnimationFrame oscilloscope that drops to 0.0% CPU when idle or hidden.

import { desktop } from '../core/desktop';
import { foley } from '../core/foley';
import type { PlayerSnapshot } from '../core/player';
import { bindMicroFoley } from './controls';

export type CrtMode = 'cover' | 'lyrics' | 'menu';

export interface CrtCallbacks {
  onSelectTrack: (albumIdx: number, trackIdx: number) => void;
  onSeekLyric: (lineIdx: number) => void;
  onCycleAlbum: () => void;
}

export class MicroCrtDisplay {
  private mode: CrtMode = 'cover';
  private coverExpanded = false;
  private menuAlbumIdx = 0;
  private menuCursorIdx = 0;

  private renderedTrackKey = '';
  private prevActiveLyricIdx = -1;
  private userLyricScrollUntil = 0;
  private volOsdTimer: number | null = null;

  private latestSnap: PlayerSnapshot | null = null;
  private smoothAmp = 0;
  private rafId: number | null = null;

  private readonly callbacks: CrtCallbacks;

  constructor(callbacks: CrtCallbacks) {
    this.callbacks = callbacks;
    this.bindScreenElements();

    document.addEventListener('visibilitychange', () => {
      this.evaluateScopeLoop();
    });
  }

  getMode(): CrtMode {
    return this.mode;
  }

  setMode(next: CrtMode, withPulse = true): void {
    if (this.mode === next) return;
    this.mode = next;

    if (next === 'menu' && this.latestSnap) {
      this.menuAlbumIdx = this.latestSnap.albumIdx;
      this.menuCursorIdx = this.latestSnap.trackIdx;
      this.renderMenu();
    }

    document.querySelectorAll<HTMLElement>('.crt-tab').forEach((tab) => {
      tab.classList.toggle('active', tab.dataset.mode === this.mode);
    });
    (['cover', 'lyrics', 'menu'] as CrtMode[]).forEach((m) => {
      document.getElementById(`view-${m}`)?.classList.toggle('active', this.mode === m);
    });

    if (withPulse) this.pulseBeam();
    if (next === 'lyrics') this.scrollActiveLyricIntoView(false);
    this.evaluateScopeLoop();
  }

  cycleMode(): void {
    const order: CrtMode[] = ['cover', 'lyrics', 'menu'];
    const next = order[(order.indexOf(this.mode) + 1) % order.length];
    this.setMode(next, true);
  }

  stepMenuCursor(delta: 1 | -1): void {
    if (!this.latestSnap) return;
    const albums = this.latestSnap.albums;
    const curAlbum = albums[this.menuAlbumIdx];
    if (!curAlbum) return;

    let nextTrack = this.menuCursorIdx + delta;
    let nextAlbum = this.menuAlbumIdx;

    if (nextTrack >= curAlbum.tracks.length) {
      nextAlbum = (this.menuAlbumIdx + 1) % albums.length;
      nextTrack = 0;
      foley.latchBand();
      this.pulseBeam();
    } else if (nextTrack < 0) {
      nextAlbum = (this.menuAlbumIdx - 1 + albums.length) % albums.length;
      nextTrack = albums[nextAlbum].tracks.length - 1;
      foley.latchBand();
      this.pulseBeam();
    }

    this.menuAlbumIdx = nextAlbum;
    this.menuCursorIdx = nextTrack;
    this.renderMenu();

    document
      .querySelector<HTMLElement>('#crt-menu-tracks .crt-track-row.cursor')
      ?.scrollIntoView({ block: 'nearest' });
  }

  confirmMenuCursor(): void {
    if (!this.latestSnap) return;
    const isAlreadyPlayingThis =
      this.latestSnap.albumIdx === this.menuAlbumIdx &&
      this.latestSnap.trackIdx === this.menuCursorIdx &&
      this.latestSnap.playing;

    if (isAlreadyPlayingThis) {
      this.setMode('cover', true);
    } else {
      this.callbacks.onSelectTrack(this.menuAlbumIdx, this.menuCursorIdx);
    }
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
    if (unit) {
      unit.style.setProperty('--crt-fg', snap.album.phosphor || '#dff2ff');
      unit.style.setProperty('--crt-glow', snap.album.glow || 'rgba(130, 202, 255, 0.44)');
    }

    const freqText = document.getElementById('crt-freq-text');
    if (freqText) freqText.textContent = `${snap.album.code} · ${snap.album.freq}`;

    document.getElementById('crt-lock-dot')?.classList.toggle('playing', snap.playing);

    const coverUrl = desktop.resolveMediaUrl(snap.album.cover);
    const coverImg = document.getElementById('crt-cover-img') as HTMLImageElement | null;
    const ambBg = document.getElementById('crt-ambient-bg');
    if (coverImg && coverImg.getAttribute('src') !== coverUrl) {
      coverImg.setAttribute('src', coverUrl);
    }
    if (ambBg) {
      ambBg.style.backgroundImage = `url("${coverUrl}")`;
    }

    const trkNo = document.getElementById('crt-track-no');
    const songTitle = document.getElementById('crt-song-title');
    const albumTitle = document.getElementById('crt-album-title');
    const fullCap = document.getElementById('crt-fullart-title');

    if (trkNo) {
      trkNo.textContent = `TRK ${String(snap.trackIdx + 1).padStart(2, '0')} / ${String(
        snap.album.tracks.length
      ).padStart(2, '0')}`;
    }
    if (songTitle) songTitle.textContent = snap.displayTitle;
    if (albumTitle) albumTitle.textContent = snap.album.title;
    if (fullCap) fullCap.textContent = `${snap.displayTitle} · ${snap.album.title}`;

    // Rebuild lyric DOM only when song or custom lyric list actually changed
    const nextKey = `${snap.album.id}:${snap.trackIdx}:${snap.displayTitle}:${snap.lyrics.length}`;
    if (nextKey !== this.renderedTrackKey) {
      this.renderedTrackKey = nextKey;
      this.prevActiveLyricIdx = -1;
      this.menuAlbumIdx = snap.albumIdx;
      this.menuCursorIdx = snap.trackIdx;
      this.rebuildLyricsDom(snap);
    }

    this.renderMenu();
    this.renderProgressSnapshot(snap);
    this.drawScopeFrame(performance.now());
    this.evaluateScopeLoop();
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
      if (this.mode === 'lyrics') {
        this.scrollActiveLyricIntoView(true);
      }
    }
  }

  // --- Internal View & Scope Mechanics ---

  private bindScreenElements(): void {
    document.querySelectorAll<HTMLElement>('.crt-tab').forEach((btn) => {
      bindMicroFoley(btn, false);
      btn.addEventListener('click', () => {
        const target = btn.dataset.mode as CrtMode | undefined;
        if (target) this.setMode(target, true);
      });
    });

    const bandBtn = document.getElementById('crt-band-btn');
    bindMicroFoley(bandBtn, false);
    bandBtn?.addEventListener('click', () => this.callbacks.onCycleAlbum());

    const artFrame = document.getElementById('crt-art-frame');
    bindMicroFoley(artFrame, false);
    artFrame?.addEventListener('click', () => {
      this.coverExpanded = !this.coverExpanded;
      this.pulseBeam();
      document.getElementById('crt-cover-layout')?.classList.toggle('expanded', this.coverExpanded);
      this.evaluateScopeLoop();
    });

    document.getElementById('crt-lyrics-scroll')?.addEventListener(
      'wheel',
      () => {
        this.userLyricScrollUntil = performance.now() + 2600;
      },
      { passive: true }
    );
  }

  private rebuildLyricsDom(snap: PlayerSnapshot): void {
    const container = document.getElementById('crt-lyrics-scroll');
    if (!container) return;

    container.innerHTML = snap.lyrics
      .map(
        (line, i) =>
          `<div class="crt-lyric-line ${i === snap.activeLyricIdx ? 'active' : ''}" data-lidx="${i}">${escapeHtml(
            line.text
          )}</div>`
      )
      .join('');

    this.prevActiveLyricIdx = snap.activeLyricIdx;

    container.querySelectorAll<HTMLElement>('.crt-lyric-line').forEach((el) => {
      bindMicroFoley(el, false);
      el.addEventListener('click', () => {
        const idx = Number(el.dataset.lidx ?? 0);
        this.userLyricScrollUntil = 0;
        this.callbacks.onSeekLyric(idx);
      });
    });
  }

  private scrollActiveLyricIntoView(smooth: boolean): void {
    if (performance.now() < this.userLyricScrollUntil) return;
    const container = document.getElementById('crt-lyrics-scroll');
    const activeEl = container?.querySelector<HTMLElement>('.crt-lyric-line.active');
    if (!container || !activeEl) return;

    const top = activeEl.offsetTop - container.clientHeight / 2 + activeEl.clientHeight / 2;
    container.scrollTo({ top: Math.max(0, top), behavior: smooth ? 'smooth' : 'auto' });
  }

  private renderMenu(): void {
    if (!this.latestSnap) return;
    const albBar = document.getElementById('crt-menu-albums');
    const trkBox = document.getElementById('crt-menu-tracks');
    if (!albBar || !trkBox) return;

    const snap = this.latestSnap;
    albBar.innerHTML = snap.albums
      .map(
        (alb, aIdx) => `
        <button class="crt-album-pill ${aIdx === this.menuAlbumIdx ? 'selected' : ''}" data-malb="${aIdx}">
          ${escapeHtml(alb.freq)} · ${escapeHtml(alb.title)}
        </button>
      `
      )
      .join('');

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

    const shownAlbum = snap.albums[this.menuAlbumIdx] ?? snap.albums[0];
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

  // On-demand 60/120fps oscilloscope scheduler: sleeps completely (0% CPU) when paused or occluded
  private evaluateScopeLoop(): void {
    const shouldAnimate =
      !document.hidden &&
      Boolean(this.latestSnap?.playing || this.smoothAmp > 0.004) &&
      (this.mode === 'cover' && !this.coverExpanded || Boolean(this.latestSnap?.playing));

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

    const env = snap.track.env?.length ? snap.track.env : [35];
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

    // Subtle physical speaker cone excursion behind 5x8 holes
    const cone = document.getElementById('rt01-cone');
    if (cone) {
      const s = 1 + this.smoothAmp * 0.065;
      cone.style.transform = `scale(${s.toFixed(3)})`;
    }

    if (this.mode !== 'cover' || this.coverExpanded) return;

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

    const beamColor = snap.album.phosphor || '#dff2ff';
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
