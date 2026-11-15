// DEEP MODULE 3: UNIVERSAL AUDIO, MULTI-PACK ARCHIVE & LYRIC STATE ENGINE
// Encapsulates empty state, multi-pack source catalog, per-track cover resolution,
// non-intrusive import (never auto-plays on import), 100ms high-precision lyric sync,
// browser .zip/folder fallback ingestion, and Windows SMTC / macOS MediaSession integration.

import { desktop, type LibrarySource } from './desktop';

export interface LyricLine {
  t: number | null;
  text: string;
  subtext?: string;
}

export interface Track {
  no: number;
  title: string;
  artist: string;
  audio: string;
  cover?: string;
  duration: number;
  fullDur: string;
  env: number[];
  lyrics: LyricLine[];
}

export interface Album {
  id: string;
  sourceId: string;
  code: string;
  title: string;
  artist: string;
  subtitle: string;
  year: string;
  freq: string;
  cover: string;
  phosphor: string;
  glow: string;
  tracks: Track[];
}

export interface PlayerSnapshot {
  hasLibrary: boolean;
  hasAnySource: boolean;
  albums: Album[];
  sources: LibrarySource[];
  albumIdx: number;
  trackIdx: number;
  flatIdx: number;
  totalFlatTracks: number;
  album: Album | null;
  track: Track | null;
  displayTitle: string;
  displayArtist: string;
  displayAlbum: string;
  coverUrl: string;
  lyrics: LyricLine[];
  activeLyricIdx: number;
  playing: boolean;
  currentTime: number;
  duration: number;
  progress: number;
  volume: number;
}

type Listener = (snap: PlayerSnapshot) => void;

interface FlatPointer {
  albumIdx: number;
  trackIdx: number;
}

interface BrowserPackRecord {
  source: LibrarySource;
  albums: Album[];
}

const PHOSPHOR_PALETTES: Array<[string, string]> = [
  ['#dff2ff', 'rgba(130, 202, 255, 0.44)'],
  ['#ffe9cf', 'rgba(255, 174, 96, 0.42)'],
  ['#d8ffec', 'rgba(106, 236, 174, 0.40)'],
  ['#f2e3ff', 'rgba(198, 146, 255, 0.40)'],
  ['#fff4d4', 'rgba(255, 212, 108, 0.40)'],
];

export const DEFAULT_COVER_DATA_URI =
  'data:image/svg+xml;utf8,' +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 240">
      <defs>
        <radialGradient id="g" cx="50%" cy="45%" r="65%">
          <stop offset="0%" stop-color="#1d2830"/>
          <stop offset="100%" stop-color="#0b1014"/>
        </radialGradient>
      </defs>
      <rect width="240" height="240" fill="url(#g)"/>
      <circle cx="120" cy="120" r="82" fill="none" stroke="rgba(223,242,255,0.16)" stroke-width="1.5"/>
      <circle cx="120" cy="120" r="62" fill="none" stroke="rgba(223,242,255,0.11)" stroke-width="1"/>
      <circle cx="120" cy="120" r="42" fill="none" stroke="rgba(223,242,255,0.18)" stroke-width="1.5"/>
      <circle cx="120" cy="120" r="14" fill="rgba(238,98,38,0.82)"/>
      <circle cx="120" cy="120" r="5" fill="#0b1014"/>
      <text x="120" y="218" text-anchor="middle" fill="rgba(223,242,255,0.45)" font-family="sans-serif" font-size="11" letter-spacing="2">无封面</text>
    </svg>`
  );

export class KomaPlayerEngine {
  private albums: Album[] = [];
  private sources: LibrarySource[] = [];
  private flat: FlatPointer[] = [];
  private browserPacks = new Map<string, BrowserPackRecord>();
  private browserBlobUrls = new Set<string>();
  private browserSeq = 0;

  private flatIdx = 0;
  private playing = false;
  private currentTime = 0;
  private duration = 0;
  private progress = 0;
  private volume = 0.76;

  private activeLyricIdx = -1;
  private syncTimer: number | null = null;
  private loadSeq = 0;
  private switchTimer: number | null = null;
  private lastStepMs = 0;

  private readonly audio = new Audio();
  private trackListeners = new Set<Listener>();
  private progressListeners = new Set<Listener>();
  private mediaErrorListeners = new Set<() => void>();

  constructor() {
    this.audio.preload = 'auto';
    this.audio.volume = this.volume;

    this.audio.addEventListener('loadedmetadata', () => {
      if (this.audio.duration && Number.isFinite(this.audio.duration)) {
        const snap = this.getSnapshot();
        if (!snap.track) return;
        const expected = desktop.resolveMediaUrl(snap.track.audio);
        if (this.audio.getAttribute('data-src') !== expected) return;
        this.duration = this.audio.duration;
        snap.track.duration = this.audio.duration;
        if (!snap.track.fullDur || snap.track.fullDur === '--:--') {
          snap.track.fullDur = formatDuration(this.audio.duration);
        }
        if (this.progress > 0) {
          this.currentTime = this.progress * this.duration;
          try {
            this.audio.currentTime = this.currentTime;
          } catch {
            // Ignore transient pre-seek errors
          }
        }
        this.recomputeActiveLyric();
        this.emitProgress();
      }
    });

    this.audio.addEventListener('timeupdate', () => {
      this.pullAudioClock();
    });

    this.audio.addEventListener('play', () => {
      this.startSyncTimer();
    });

    this.audio.addEventListener('pause', () => {
      this.stopSyncTimer();
    });

    this.audio.addEventListener('ended', () => {
      this.stopSyncTimer();
      this.stepTrack(1, true);
    });

    this.audio.addEventListener('error', () => {
      if (!this.audio.getAttribute('src') || !this.audio.error) return;
      this.playing = false;
      this.stopSyncTimer();
      this.syncMediaSessionPlayback();
      this.emitTrackChange();
      this.mediaErrorListeners.forEach((fn) => fn());
    });

    this.bindMediaSession();
  }

  setCatalog(
    nextAlbums: Album[],
    nextSources: LibrarySource[] = this.sources,
    selectAlbumId: string | null = null,
    autoPlaySelected = false
  ): void {
    const prevSnap = this.getSnapshot();
    const prevAudioKey = prevSnap.track ? `${prevSnap.album?.id}::${prevSnap.track.audio}` : null;
    const wasPlaying = this.playing;

    this.sources = nextSources;
    this.rebuildCatalog(nextAlbums);

    if (!this.flat.length) {
      ++this.loadSeq;
      this.clearSwitchTimer();
      this.playing = false;
      this.stopSyncTimer();
      this.audio.pause();
      this.audio.removeAttribute('src');
      this.audio.removeAttribute('data-src');
      this.currentTime = 0;
      this.duration = 0;
      this.progress = 0;
      this.activeLyricIdx = -1;
      this.syncMediaSessionPlayback();
      this.emitTrackChange();
      return;
    }

    // If a song is currently playing and still exists in the updated library,
    // keep playback completely uninterrupted even when new packs are imported.
    if (wasPlaying && prevAudioKey && !autoPlaySelected) {
      const preservedPlayingIdx = this.flat.findIndex((p) => {
        const a = this.albums[p.albumIdx];
        const t = a?.tracks[p.trackIdx];
        return a && t && `${a.id}::${t.audio}` === prevAudioKey;
      });
      if (preservedPlayingIdx >= 0) {
        this.flatIdx = preservedPlayingIdx;
        this.emitTrackChange();
        return;
      }
    }

    // Check if previous track still exists when paused (e.g. rescan or toggling another pack)
    if (!selectAlbumId && prevAudioKey) {
      const preservedIdx = this.flat.findIndex((p) => {
        const a = this.albums[p.albumIdx];
        const t = a?.tracks[p.trackIdx];
        return a && t && `${a.id}::${t.audio}` === prevAudioKey;
      });
      if (preservedIdx >= 0) {
        this.flatIdx = preservedIdx;
        this.emitTrackChange();
        return;
      }
    }

    if (selectAlbumId) {
      const aIdx = this.albums.findIndex((a) => a.id === selectAlbumId);
      if (aIdx >= 0) {
        const targetFlat = this.flat.findIndex((p) => p.albumIdx === aIdx && p.trackIdx === 0);
        if (targetFlat >= 0) {
          this.flatIdx = targetFlat;
        }
      }
    }

    // Global play/pause state: preserve wasPlaying across pack changes; never auto-play on fresh import when paused
    this.playing = autoPlaySelected ? true : wasPlaying;
    if (!this.playing) {
      this.stopSyncTimer();
      this.audio.pause();
    }
    this.prepareAudioSource(this.playing, false);
    this.emitTrackChange();
  }

  toggleBrowserSource(sourceId: string): void {
    const rec = this.browserPacks.get(sourceId);
    if (!rec) return;
    rec.source.enabled = !rec.source.enabled;
    this.syncFromBrowserPacks(null);
  }

  removeBrowserSource(sourceId: string): void {
    this.browserPacks.delete(sourceId);
    this.syncFromBrowserPacks(null);
  }

  clearBrowserLibrary(): void {
    this.browserPacks.clear();
    this.browserBlobUrls.forEach((u) => URL.revokeObjectURL(u));
    this.browserBlobUrls.clear();
    this.setCatalog([], [], null, false);
  }

  onTrackChange(fn: Listener): () => void {
    this.trackListeners.add(fn);
    fn(this.getSnapshot());
    return () => this.trackListeners.delete(fn);
  }

  onProgress(fn: Listener): () => void {
    this.progressListeners.add(fn);
    return () => this.progressListeners.delete(fn);
  }

  onMediaError(fn: () => void): () => void {
    this.mediaErrorListeners.add(fn);
    return () => this.mediaErrorListeners.delete(fn);
  }

  getSnapshot(): PlayerSnapshot {
    const hasAnySource = this.albums.length > 0 || this.sources.length > 0;
    if (!this.albums.length || !this.flat.length) {
      return {
        hasLibrary: false,
        hasAnySource,
        albums: [],
        sources: this.sources,
        albumIdx: 0,
        trackIdx: 0,
        flatIdx: 0,
        totalFlatTracks: 0,
        album: null,
        track: null,
        displayTitle: '未播放',
        displayArtist: '',
        displayAlbum: '',
        coverUrl: DEFAULT_COVER_DATA_URI,
        lyrics: [],
        activeLyricIdx: -1,
        playing: false,
        currentTime: 0,
        duration: 0,
        progress: 0,
        volume: this.volume,
      };
    }

    const ptr = this.flat[this.flatIdx] ?? { albumIdx: 0, trackIdx: 0 };
    const album = this.albums[ptr.albumIdx] ?? this.albums[0];
    const track = album.tracks[ptr.trackIdx] ?? album.tracks[0];
    const rawCover = track.cover || album.cover;
    const resolvedCover = rawCover ? desktop.resolveMediaUrl(rawCover) : DEFAULT_COVER_DATA_URI;
    const displayArtist = track.artist || album.artist || album.subtitle || '';

    return {
      hasLibrary: true,
      hasAnySource: true,
      albums: this.albums,
      sources: this.sources,
      albumIdx: ptr.albumIdx,
      trackIdx: ptr.trackIdx,
      flatIdx: this.flatIdx,
      totalFlatTracks: this.flat.length,
      album,
      track,
      displayTitle: track.title,
      displayArtist,
      displayAlbum: album.title,
      coverUrl: resolvedCover || DEFAULT_COVER_DATA_URI,
      lyrics: track.lyrics ?? [],
      activeLyricIdx: this.activeLyricIdx,
      playing: this.playing,
      currentTime: this.currentTime,
      duration: this.duration,
      progress: this.progress,
      volume: this.volume,
    };
  }

  togglePlay(): boolean {
    if (!this.flat.length) return false;
    const seq = ++this.loadSeq;
    this.clearSwitchTimer();
    this.playing = !this.playing;

    if (this.playing) {
      this.ensureAudioLoaded();
      if (
        this.currentTime > 0 &&
        this.audio.readyState >= 1 &&
        Math.abs(this.audio.currentTime - this.currentTime) > 0.2
      ) {
        try {
          this.audio.currentTime = this.currentTime;
        } catch {
          // Will apply on loadedmetadata
        }
      }
      this.startPlaybackWithGuard(seq);
    } else {
      this.stopSyncTimer();
      this.audio.pause();
    }

    this.syncMediaSessionPlayback();
    this.emitTrackChange();
    return this.playing;
  }

  stepTrack(delta: 1 | -1, autoContinue = false): { crossedAlbum: boolean } {
    if (!this.flat.length) return { crossedAlbum: false };
    const prevAlbumIdx = this.flat[this.flatIdx]?.albumIdx ?? 0;
    this.flatIdx = (this.flatIdx + delta + this.flat.length) % this.flat.length;
    const nextAlbumIdx = this.flat[this.flatIdx]?.albumIdx ?? 0;

    if (autoContinue) this.playing = true;

    const now = performance.now();
    const rapidSpin = !autoContinue && now - this.lastStepMs < 90;
    this.lastStepMs = now;

    this.prepareAudioSource(this.playing, rapidSpin);
    this.emitTrackChange();

    return { crossedAlbum: prevAlbumIdx !== nextAlbumIdx };
  }

  selectTrack(albumIdx: number, trackIdx: number, forcePlay = true): { crossedAlbum: boolean } {
    const prevAlbumIdx = this.flat[this.flatIdx]?.albumIdx ?? 0;
    const targetFlat = this.flat.findIndex((p) => p.albumIdx === albumIdx && p.trackIdx === trackIdx);
    if (targetFlat < 0) return { crossedAlbum: false };

    this.flatIdx = targetFlat;
    if (forcePlay) this.playing = true;
    this.prepareAudioSource(this.playing, false);
    this.emitTrackChange();

    return { crossedAlbum: prevAlbumIdx !== albumIdx };
  }

  seek(ratio: number): void {
    if (!this.flat.length) return;
    const clamped = Math.max(0, Math.min(0.996, ratio));
    this.progress = clamped;
    const dur =
      this.audio.duration && Number.isFinite(this.audio.duration)
        ? this.audio.duration
        : this.duration;
    this.currentTime = clamped * dur;
    if (this.audio.src && this.audio.readyState >= 1 && Number.isFinite(this.audio.duration)) {
      try {
        this.audio.currentTime = this.currentTime;
      } catch {
        // Ignore transient seek before metadata lock
      }
    }
    this.recomputeActiveLyric();
    this.emitProgress();
  }

  seekToLyric(lineIdx: number): void {
    const snap = this.getSnapshot();
    if (!snap.hasLibrary || !snap.track) return;
    const item = snap.lyrics[lineIdx];
    if (!item) return;

    if (item.t !== null && this.duration > 0) {
      this.seek(item.t / this.duration);
    } else if (item.t === null && snap.lyrics.length > 1) {
      this.seek(lineIdx / snap.lyrics.length);
    } else {
      this.activeLyricIdx = lineIdx;
      this.seek(0);
    }
    if (!this.playing) {
      this.togglePlay();
    }
  }

  setVolume(nextVolume: number): number {
    this.volume = Math.max(0, Math.min(1, nextVolume));
    this.audio.volume = this.volume;
    return this.volume;
  }

  // Browser-preview fallback importer (supports multiple .zip packs, folders, and loose files)
  async ingestBrowserFiles(files: File[]): Promise<void> {
    if (!files.length) return;

    // Group each .zip as its own independent pack; group non-zip files together
    const zipFiles = files.filter((f) => f.name.toLowerCase().endsWith('.zip'));
    const otherFiles = files.filter((f) => !f.name.toLowerCase().endsWith('.zip'));

    // Single .lrc drop attaches to active song
    if (
      zipFiles.length === 0 &&
      otherFiles.length === 1 &&
      otherFiles[0].name.toLowerCase().endsWith('.lrc') &&
      this.flat.length > 0
    ) {
      const text = await otherFiles[0].text();
      const parsed = parseLrcFull(text);
      const snap = this.getSnapshot();
      if (parsed.lines.length && snap.track) {
        snap.track.lyrics = parsed.lines;
        this.recomputeActiveLyric();
        this.emitTrackChange();
      }
      return;
    }

    let firstSelectId: string | null = null;

    for (const zf of zipFiles) {
      const extracted = await unpackZipInBrowser(zf);
      const seq = ++this.browserSeq;
      const sourceId = `zip:browser-${seq}`;
      const packName = zf.name.normalize('NFC').replace(/\.zip$/i, '');
      const albums = await buildAlbumsFromVirtualEntries(
        extracted,
        sourceId,
        this.albums.length + seq,
        (url) => this.browserBlobUrls.add(url)
      );
      if (!albums.length) continue;
      const trackCount = albums.reduce((s, a) => s + a.tracks.length, 0);
      this.browserPacks.set(sourceId, {
        source: {
          id: sourceId,
          kind: 'zip',
          name: packName,
          path: zf.name,
          cover: albums[0]?.cover || '',
          enabled: true,
          albumCount: albums.length,
          trackCount,
          missing: false,
        },
        albums,
      });
      if (!firstSelectId) firstSelectId = albums[0].id;
    }

    if (otherFiles.length > 0) {
      const entries: VirtualFileEntry[] = otherFiles.map((file) => {
        const relPath =
          (file as File & { webkitRelativePath?: string }).webkitRelativePath || file.name;
        return { relPath: relPath.normalize('NFC').replace(/\\/g, '/'), blob: file };
      });
      const hasRelFolder = otherFiles.some(
        (f) => Boolean((f as File & { webkitRelativePath?: string }).webkitRelativePath)
      );
      const seq = ++this.browserSeq;
      const sourceId = `${hasRelFolder ? 'folder' : 'files'}:browser-${seq}`;
      const albums = await buildAlbumsFromVirtualEntries(
        entries,
        sourceId,
        this.albums.length + seq,
        (url) => this.browserBlobUrls.add(url)
      );
      if (albums.length > 0) {
        const trackCount = albums.reduce((s, a) => s + a.tracks.length, 0);
        this.browserPacks.set(sourceId, {
          source: {
            id: sourceId,
            kind: hasRelFolder ? 'folder' : 'files',
            name: albums[0].title,
            path: albums[0].title,
            cover: albums[0]?.cover || '',
            enabled: true,
            albumCount: albums.length,
            trackCount,
            missing: false,
          },
          albums,
        });
        if (!firstSelectId) firstSelectId = albums[0].id;
      }
    }

    if (firstSelectId) {
      this.syncFromBrowserPacks(firstSelectId);
    }
  }

  // --- Internal Mechanics ---

  private syncFromBrowserPacks(selectAlbumId: string | null): void {
    const activeAlbums: Album[] = [];
    const allSources: LibrarySource[] = [];
    for (const rec of this.browserPacks.values()) {
      allSources.push({ ...rec.source });
      if (rec.source.enabled) {
        activeAlbums.push(...rec.albums);
      }
    }
    activeAlbums.forEach((alb, i) => {
      const p = PHOSPHOR_PALETTES[i % PHOSPHOR_PALETTES.length];
      alb.code = `CH-${String(i + 1).padStart(2, '0')}`;
      alb.freq = (19.4 + i * 0.8).toFixed(1);
      alb.phosphor = p[0];
      alb.glow = p[1];
    });
    this.setCatalog(activeAlbums, allSources, selectAlbumId, false);
  }

  private startSyncTimer(): void {
    if (this.syncTimer !== null) return;
    this.syncTimer = window.setInterval(() => {
      if (!this.playing || this.audio.paused) return;
      this.pullAudioClock();
    }, 100);
  }

  private stopSyncTimer(): void {
    if (this.syncTimer !== null) {
      window.clearInterval(this.syncTimer);
      this.syncTimer = null;
    }
  }

  private pullAudioClock(): void {
    const dur =
      this.audio.duration && Number.isFinite(this.audio.duration)
        ? this.audio.duration
        : this.duration;
    if (dur > 0) {
      this.duration = dur;
      this.currentTime = this.audio.currentTime;
      this.progress = Math.min(1, this.currentTime / dur);
      this.recomputeActiveLyric();
      this.emitProgress();
    }
  }

  private rebuildCatalog(list: Album[]): void {
    this.albums = list;
    this.flat = [];
    this.albums.forEach((alb, aIdx) => {
      alb.tracks.forEach((_, tIdx) => {
        this.flat.push({ albumIdx: aIdx, trackIdx: tIdx });
      });
    });
    if (this.flatIdx >= this.flat.length) {
      this.flatIdx = 0;
    }
  }

  private clearSwitchTimer(): void {
    if (this.switchTimer !== null) {
      window.clearTimeout(this.switchTimer);
      this.switchTimer = null;
    }
  }

  private ensureAudioLoaded(): boolean {
    const snap = this.getSnapshot();
    if (!snap.track) return false;
    const resolved = desktop.resolveMediaUrl(snap.track.audio);
    if (this.audio.getAttribute('data-src') !== resolved) {
      this.audio.setAttribute('data-src', resolved);
      this.audio.src = resolved;
      return true;
    }
    return false;
  }

  private startPlaybackWithGuard(seq: number): void {
    this.audio
      .play()
      .then(() => {
        if (seq !== this.loadSeq || !this.playing) {
          if (!this.playing) this.audio.pause();
          return;
        }
        this.startSyncTimer();
        this.syncMediaSessionPlayback();
      })
      .catch((err: unknown) => {
        if (seq !== this.loadSeq || !this.playing) return;
        const errName = (err as { name?: string })?.name;
        // AbortError occurs normally when a track load is superseded by the next dial notch;
        // NEVER overwrite global `this.playing` state on transient load interruptions!
        if (errName === 'AbortError') return;

        const onCanPlay = () => {
          if (seq !== this.loadSeq || !this.playing) return;
          this.audio
            .play()
            .then(() => {
              if (seq === this.loadSeq && this.playing) {
                this.startSyncTimer();
                this.syncMediaSessionPlayback();
              }
            })
            .catch(() => {});
        };
        this.audio.addEventListener('canplay', onCanPlay, { once: true });
      });
  }

  private commitAudioSource(seq: number, shouldPlay: boolean): void {
    if (seq !== this.loadSeq || !this.flat.length) return;
    const changedSrc = this.ensureAudioLoaded();
    if (!changedSrc) {
      try {
        this.audio.currentTime = 0;
      } catch {
        // Initial load starts at 0 automatically
      }
    }

    if (shouldPlay && this.playing) {
      this.startPlaybackWithGuard(seq);
    } else {
      this.stopSyncTimer();
      this.audio.pause();
      this.syncMediaSessionPlayback();
    }
  }

  private prepareAudioSource(autoPlay: boolean, debounceRapidSpin: boolean): void {
    if (!this.flat.length) return;
    const snap = this.getSnapshot();
    if (!snap.track) return;

    const seq = ++this.loadSeq;
    this.clearSwitchTimer();

    this.progress = 0;
    this.currentTime = 0;
    this.duration = snap.track.duration || 0;
    this.activeLyricIdx = -1;

    this.recomputeActiveLyric();
    this.syncMediaSessionMetadata(snap);
    this.syncMediaSessionPlayback();

    if (debounceRapidSpin) {
      this.stopSyncTimer();
      this.audio.pause();
      this.switchTimer = window.setTimeout(() => {
        this.switchTimer = null;
        this.commitAudioSource(seq, autoPlay);
      }, 36);
      return;
    }

    this.commitAudioSource(seq, autoPlay);
  }

  private recomputeActiveLyric(): void {
    const snap = this.getSnapshot();
    const lines = snap.lyrics;
    if (!lines.length || !snap.track) {
      this.activeLyricIdx = -1;
      return;
    }

    const hasTimedLines = lines.some((l) => l.t !== null);
    let idx = 0;

    if (hasTimedLines) {
      idx = -1;
      const tNow = this.currentTime + 0.08;
      for (let i = 0; i < lines.length; i++) {
        const ts = lines[i].t;
        if (ts !== null && tNow >= ts) {
          idx = i;
        }
      }
      if (idx < 0) {
        idx = 0;
      }
    } else if (lines.length > 1) {
      idx = Math.min(lines.length - 1, Math.floor(this.progress * lines.length));
    }
    this.activeLyricIdx = idx;
  }

  private bindMediaSession(): void {
    if (!('mediaSession' in navigator)) return;
    const ms = navigator.mediaSession;
    try {
      ms.setActionHandler('play', () => {
        if (!this.playing) this.togglePlay();
      });
      ms.setActionHandler('pause', () => {
        if (this.playing) this.togglePlay();
      });
      ms.setActionHandler('previoustrack', () => this.stepTrack(-1));
      ms.setActionHandler('nexttrack', () => this.stepTrack(1));
    } catch {
      // Unsupported handler in environment
    }
  }

  private syncMediaSessionMetadata(snap: PlayerSnapshot): void {
    if (!('mediaSession' in navigator) || typeof MediaMetadata === 'undefined' || !snap.hasLibrary) {
      return;
    }
    const artwork =
      snap.coverUrl && !snap.coverUrl.startsWith('data:image/svg+xml')
        ? [{ src: snap.coverUrl, sizes: '512x512' }]
        : [];
    navigator.mediaSession.metadata = new MediaMetadata({
      title: snap.displayTitle,
      artist: snap.displayArtist,
      album: snap.displayAlbum,
      artwork,
    });
  }

  private syncMediaSessionPlayback(): void {
    if (!('mediaSession' in navigator)) return;
    navigator.mediaSession.playbackState = this.playing ? 'playing' : 'paused';
  }

  private emitTrackChange(): void {
    const snap = this.getSnapshot();
    this.trackListeners.forEach((fn) => fn(snap));
    this.progressListeners.forEach((fn) => fn(snap));
  }

  private emitProgress(): void {
    const snap = this.getSnapshot();
    this.progressListeners.forEach((fn) => fn(snap));
  }
}

// ============================================================================
// LRC PARSER & BROWSER VIRTUAL ARCHIVE / FOLDER READER (.zip / Folder / Files)
// ============================================================================

interface ParsedLrcResult {
  title?: string;
  artist?: string;
  album?: string;
  lines: LyricLine[];
}

export function parseLrcFull(raw: string): ParsedLrcResult {
  let offsetSec = 0;
  let title: string | undefined;
  let artist: string | undefined;
  let album: string | undefined;
  const timed: Array<{ t: number; text: string }> = [];
  const untimed: string[] = [];

  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim().replace(/^\uFEFF/, '');
    if (!trimmed) continue;

    const metaMatch = trimmed.match(/^\[(ti|ar|al|offset):\s*(.*?)\s*\]$/i);
    if (metaMatch) {
      const key = metaMatch[1].toLowerCase();
      const val = metaMatch[2].trim();
      if (key === 'offset') {
        const parsedOff = Number(val);
        if (Number.isFinite(parsedOff)) offsetSec = parsedOff / 1000;
      } else if (key === 'ti' && val) {
        title = val;
      } else if (key === 'ar' && val) {
        artist = val;
      } else if (key === 'al' && val) {
        album = val;
      }
      continue;
    }

    let rest = trimmed;
    const times: number[] = [];
    let hadTag = false;

    while (rest.startsWith('[')) {
      const closeIdx = rest.indexOf(']');
      if (closeIdx < 0) break;
      hadTag = true;
      const tag = rest.slice(1, closeIdx).trim();
      const tm = tag.match(/^(\d+):(\d+(?:[.:]\d+)?)$/);
      if (tm) {
        times.push(Number(tm[1]) * 60 + Number(tm[2].replace(':', '.')));
        rest = rest.slice(closeIdx + 1);
      } else {
        rest = '';
        break;
      }
    }

    const body = rest.trim();
    if (times.length > 0) {
      if (body && body !== '//') {
        for (const t of times) {
          timed.push({ t: Math.max(0, t - offsetSec), text: body });
        }
      }
    } else if (!hadTag && body) {
      untimed.push(body);
    }
  }

  if (timed.length > 0) {
    timed.sort((a, b) => a.t - b.t);
    const merged: LyricLine[] = [];
    for (const item of timed) {
      const prev = merged[merged.length - 1];
      if (
        prev &&
        prev.t !== null &&
        Math.abs(item.t - prev.t) <= 0.08 &&
        prev.text !== item.text &&
        !prev.subtext
      ) {
        prev.subtext = item.text;
      } else {
        merged.push({ t: item.t, text: item.text });
      }
    }
    return { title, artist, album, lines: merged };
  }

  return {
    title,
    artist,
    album,
    lines: untimed.map((text) => ({ t: null, text })),
  };
}

interface VirtualFileEntry {
  relPath: string;
  blob: Blob;
}

async function unpackZipInBrowser(archiveFile: File): Promise<VirtualFileEntry[]> {
  const buf = await archiveFile.arrayBuffer();
  const view = new DataView(buf);
  const bytes = new Uint8Array(buf);
  const decoder = new TextDecoder('utf-8');

  let eocdOffset = -1;
  for (let i = bytes.length - 22; i >= Math.max(0, bytes.length - 65557); i--) {
    if (view.getUint32(i, true) === 0x06054b50) {
      eocdOffset = i;
      break;
    }
  }
  if (eocdOffset < 0) return [];

  const totalEntries = view.getUint16(eocdOffset + 10, true);
  const cdOffset = view.getUint32(eocdOffset + 16, true);

  const out: VirtualFileEntry[] = [];
  let ptr = cdOffset;

  for (let i = 0; i < totalEntries && ptr + 46 <= bytes.length; i++) {
    if (view.getUint32(ptr, true) !== 0x02014b50) break;
    const method = view.getUint16(ptr + 10, true);
    const compSize = view.getUint32(ptr + 20, true);
    const nameLen = view.getUint16(ptr + 28, true);
    const extraLen = view.getUint16(ptr + 30, true);
    const commentLen = view.getUint16(ptr + 32, true);
    const localHeaderOffset = view.getUint32(ptr + 42, true);

    const relPath = decoder.decode(bytes.subarray(ptr + 46, ptr + 46 + nameLen)).normalize('NFC');
    ptr += 46 + nameLen + extraLen + commentLen;

    if (
      relPath.endsWith('/') ||
      relPath.includes('__MACOSX') ||
      relPath.endsWith('.DS_Store') ||
      relPath.endsWith('Thumbs.db')
    ) {
      continue;
    }

    if (localHeaderOffset + 30 > bytes.length) continue;
    const lNameLen = view.getUint16(localHeaderOffset + 26, true);
    const lExtraLen = view.getUint16(localHeaderOffset + 28, true);
    const dataStart = localHeaderOffset + 30 + lNameLen + lExtraLen;
    const compSlice = bytes.subarray(dataStart, dataStart + compSize);

    if (method === 0) {
      out.push({ relPath, blob: new Blob([compSlice]) });
    } else if (method === 8 && typeof DecompressionStream !== 'undefined') {
      const ds = new DecompressionStream('deflate-raw');
      const stream = new Blob([compSlice]).stream().pipeThrough(ds);
      const decompressed = await new Response(stream).blob();
      out.push({ relPath, blob: decompressed });
    }
  }

  return out;
}

async function buildAlbumsFromVirtualEntries(
  entries: VirtualFileEntry[],
  sourceId: string,
  startPaletteIdx: number,
  registerUrl: (url: string) => void
): Promise<Album[]> {
  const byDir = new Map<string, Map<string, Blob>>();
  for (const e of entries) {
    const idx = e.relPath.lastIndexOf('/');
    const dir = idx >= 0 ? e.relPath.slice(0, idx) : '';
    const name = idx >= 0 ? e.relPath.slice(idx + 1) : e.relPath;
    if (!byDir.has(dir)) byDir.set(dir, new Map());
    byDir.get(dir)!.set(name, e.blob);
  }

  const albums: Album[] = [];
  const sortedDirs = Array.from(byDir.keys()).sort();

  for (const dir of sortedDirs) {
    const filesMap = byDir.get(dir)!;
    const audioNames = Array.from(filesMap.keys())
      .filter((n) => /\.(flac|m4a|mp3|wav|ogg|opus|aac|webm)$/i.test(n))
      .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
    if (!audioNames.length) continue;

    const createBlobUrl = (blob: Blob): string => {
      const u = URL.createObjectURL(blob);
      registerUrl(u);
      return u;
    };

    let folderCoverUrl = '';
    for (const [name, blob] of filesMap.entries()) {
      if (/^(cover|folder|front|album|art)\.(jpg|jpeg|png|webp)$/i.test(name)) {
        folderCoverUrl = createBlobUrl(blob);
        break;
      }
    }
    if (!folderCoverUrl) {
      const images = Array.from(filesMap.entries()).filter(([n]) => /\.(jpg|jpeg|png|webp)$/i.test(n));
      if (images.length === 1) {
        const imgStem = images[0][0].replace(/\.[^.]+$/, '').toLowerCase();
        const matchesTrack = audioNames.some((a) => a.replace(/\.[^.]+$/, '').toLowerCase() === imgStem);
        if (!matchesTrack) folderCoverUrl = createBlobUrl(images[0][1]);
      }
    }

    const tracks: Track[] = [];
    let detectedArtist = '';
    let detectedAlbum = '';

    for (let i = 0; i < audioNames.length; i++) {
      const name = audioNames[i];
      const stem = name.replace(/\.[^.]+$/, '');
      const lrcBlob = filesMap.get(`${stem}.lrc`) ?? filesMap.get(`${stem}.LRC`);
      const parsedLrc = lrcBlob ? parseLrcFull(await lrcBlob.text()) : null;

      const title = parsedLrc?.title || stripTrackPrefix(stem);
      const artist = parsedLrc?.artist || '';
      if (!detectedArtist && artist) detectedArtist = artist;
      if (!detectedAlbum && parsedLrc?.album) detectedAlbum = parsedLrc.album;

      const lyrics =
        parsedLrc && parsedLrc.lines.length > 0
          ? parsedLrc.lines
          : [{ t: null, text: title }];

      let trCoverUrl: string | undefined;
      for (const ext of ['jpg', 'jpeg', 'png', 'webp']) {
        const found = filesMap.get(`${stem}.${ext}`) ?? filesMap.get(`${stem}.${ext.toUpperCase()}`);
        if (found) {
          trCoverUrl = createBlobUrl(found);
          break;
        }
      }

      tracks.push({
        no: i + 1,
        title,
        artist,
        audio: createBlobUrl(filesMap.get(name)!),
        cover: trCoverUrl,
        duration: 0,
        fullDur: '--:--',
        env: makeEnvelope(title),
        lyrics,
      });
    }

    if (!tracks.length) continue;

    const pIdx = startPaletteIdx + albums.length;
    const palette = PHOSPHOR_PALETTES[pIdx % PHOSPHOR_PALETTES.length];
    const rawDirTitle = dir.split('/').filter(Boolean).pop() || '';
    const cleanDirTitle = rawDirTitle.replace(/^\d{4}\s*[-_·.]\s*/, '').trim();
    const albTitle = cleanDirTitle || detectedAlbum || '未命名专辑';
    const slug = albTitle.toLowerCase().replace(/[^a-z0-9\u3040-\u9fff]+/g, '-') || `alb-${pIdx + 1}`;

    albums.push({
      id: `${sourceId}-${slug}-${albums.length}`,
      sourceId,
      code: '',
      title: albTitle,
      artist: detectedArtist,
      subtitle: detectedArtist,
      year: '',
      freq: '',
      cover: folderCoverUrl || tracks.find((t) => t.cover)?.cover || '',
      phosphor: palette[0],
      glow: palette[1],
      tracks,
    });
  }

  return albums;
}

function stripTrackPrefix(nameOrStem: string): string {
  const noExt = nameOrStem.replace(/\.[^.]+$/, '').trim();
  const stripped = noExt.replace(/^\d+[\s._-]+/, '').trim();
  return stripped || noExt;
}

function formatDuration(sec: number): string {
  const total = Math.max(0, Math.round(sec));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`;
}

function makeEnvelope(seedStr: string): number[] {
  let hash = 2166136261;
  for (let i = 0; i < seedStr.length; i++) {
    hash ^= seedStr.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  const offset = ((hash >>> 0) % 97) * 0.07;
  return Array.from({ length: 120 }, (_, i) => {
    const phase = i * 0.21 + offset;
    const v = 52 + 26 * Math.sin(phase) + 14 * Math.cos(phase * 2.3);
    return Math.max(16, Math.min(94, Math.round(v)));
  });
}
