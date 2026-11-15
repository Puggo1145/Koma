// DEEP MODULE 3: YORUSHIKA AUDIO & LYRIC STATE ENGINE
// Encapsulates discography indexing, HTMLAudio lifecycle, LRC vs studio-clip vocal cadence,
// local file drop ingestion, and macOS MediaSession integration behind a clean reactive store.

import { desktop } from './desktop';

export interface LyricLine {
  t: number | null;
  text: string;
}

export interface Track {
  no: number;
  title: string;
  audio: string;
  clipDur: number;
  fullDur: string;
  hookIdx: number;
  env: number[];
  lyrics: LyricLine[];
}

export interface Album {
  id: string;
  code: string;
  title: string;
  subtitle: string;
  year: string;
  freq: string;
  cover: string;
  phosphor: string;
  glow: string;
  tracks: Track[];
}

export interface PlayerSnapshot {
  albums: Album[];
  albumIdx: number;
  trackIdx: number;
  flatIdx: number;
  totalFlatTracks: number;
  album: Album;
  track: Track;
  displayTitle: string;
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

export class YoruPlayerEngine {
  private seedAlbums: Album[] = [];
  private albums: Album[] = [];
  private flat: FlatPointer[] = [];

  private flatIdx = 0;
  private playing = false;
  private currentTime = 0;
  private duration = 30;
  private progress = 0;
  private volume = 0.76;

  private lyricAnchorIdx = 0;
  private activeLyricIdx = -1;

  private customAudioUrl: string | null = null;
  private customTitle: string | null = null;
  private customLyrics: LyricLine[] | null = null;

  private readonly audio = new Audio();
  private trackListeners = new Set<Listener>();
  private progressListeners = new Set<Listener>();

  constructor() {
    this.audio.preload = 'auto';
    this.audio.volume = this.volume;

    this.audio.addEventListener('loadedmetadata', () => {
      if (this.audio.duration && Number.isFinite(this.audio.duration)) {
        this.duration = this.audio.duration;
        this.recomputeActiveLyric();
        this.emitProgress();
      }
    });

    this.audio.addEventListener('timeupdate', () => {
      if (this.audio.duration && Number.isFinite(this.audio.duration)) {
        this.duration = this.audio.duration;
        this.currentTime = this.audio.currentTime;
        this.progress = Math.min(1, this.currentTime / this.duration);
        this.recomputeActiveLyric();
        this.emitProgress();
      }
    });

    this.audio.addEventListener('ended', () => {
      this.stepTrack(1, true);
    });

    this.bindMediaSession();
  }

  async loadBuiltInCatalog(): Promise<void> {
    const res = await fetch('/library/catalog.json');
    this.seedAlbums = (await res.json()) as Album[];
    this.rebuildCatalog(this.seedAlbums);
    this.prepareAudioSource(false);
    this.emitTrackChange();
  }

  mergeLocalAlbums(localAlbums: Album[]): void {
    if (!localAlbums.length) return;
    const byId = new Map<string, Album>();
    this.seedAlbums.forEach((a) => byId.set(a.id, a));
    localAlbums.forEach((a) => byId.set(a.id, a));
    this.rebuildCatalog(Array.from(byId.values()));
    this.prepareAudioSource(this.playing);
    this.emitTrackChange();
  }

  onTrackChange(fn: Listener): () => void {
    this.trackListeners.add(fn);
    if (this.albums.length) fn(this.getSnapshot());
    return () => this.trackListeners.delete(fn);
  }

  onProgress(fn: Listener): () => void {
    this.progressListeners.add(fn);
    return () => this.progressListeners.delete(fn);
  }

  getSnapshot(): PlayerSnapshot {
    const ptr = this.flat[this.flatIdx] ?? { albumIdx: 0, trackIdx: 0 };
    const album = this.albums[ptr.albumIdx] ?? this.albums[0];
    const track = album.tracks[ptr.trackIdx] ?? album.tracks[0];
    const lyrics = this.customLyrics ?? track.lyrics ?? [];
    return {
      albums: this.albums,
      albumIdx: ptr.albumIdx,
      trackIdx: ptr.trackIdx,
      flatIdx: this.flatIdx,
      totalFlatTracks: this.flat.length,
      album,
      track,
      displayTitle: this.customTitle ?? track.title,
      lyrics,
      activeLyricIdx: this.activeLyricIdx,
      playing: this.playing,
      currentTime: this.currentTime,
      duration: this.duration,
      progress: this.progress,
      volume: this.volume,
    };
  }

  togglePlay(): boolean {
    if (!this.albums.length) return false;
    this.playing = !this.playing;

    if (this.playing) {
      this.ensureAudioLoaded();
      this.audio.play().catch(() => {
        this.playing = false;
        this.emitTrackChange();
      });
    } else {
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

    this.clearCustomOverrides();
    if (autoContinue) this.playing = true;
    this.prepareAudioSource(this.playing);
    this.emitTrackChange();

    return { crossedAlbum: prevAlbumIdx !== nextAlbumIdx };
  }

  selectTrack(albumIdx: number, trackIdx: number, forcePlay = true): { crossedAlbum: boolean } {
    const prevAlbumIdx = this.flat[this.flatIdx]?.albumIdx ?? 0;
    const targetFlat = this.flat.findIndex((p) => p.albumIdx === albumIdx && p.trackIdx === trackIdx);
    if (targetFlat < 0) return { crossedAlbum: false };

    this.flatIdx = targetFlat;
    this.clearCustomOverrides();
    if (forcePlay) this.playing = true;
    this.prepareAudioSource(this.playing);
    this.emitTrackChange();

    return { crossedAlbum: prevAlbumIdx !== albumIdx };
  }

  seek(ratio: number): void {
    const clamped = Math.max(0, Math.min(0.996, ratio));
    this.progress = clamped;
    const dur = this.audio.duration && Number.isFinite(this.audio.duration) ? this.audio.duration : this.duration;
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
    const item = snap.lyrics[lineIdx];
    if (!item) return;

    this.lyricAnchorIdx = lineIdx;
    if (this.duration > 45 && item.t !== null) {
      this.seek(item.t / this.duration);
    } else {
      this.activeLyricIdx = -1;
      this.seek(0.01);
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

  ingestDroppedFiles(files: File[]): void {
    files.forEach((file) => {
      const lower = file.name.toLowerCase();
      if (lower.endsWith('.lrc')) {
        const reader = new FileReader();
        reader.onload = () => {
          const parsed = parseLrcText(String(reader.result));
          if (parsed.length) {
            this.customLyrics = parsed;
            this.lyricAnchorIdx = 0;
            this.activeLyricIdx = -1;
            this.recomputeActiveLyric();
            this.emitTrackChange();
          }
        };
        reader.readAsText(file);
      } else if (file.type.startsWith('audio/') || /\.(flac|alac|mp3|m4a|wav|ogg)$/i.test(lower)) {
        if (this.customAudioUrl) URL.revokeObjectURL(this.customAudioUrl);
        this.customAudioUrl = URL.createObjectURL(file);
        this.customTitle = file.name.replace(/\.[^.]+$/, '');
        this.playing = true;
        this.prepareAudioSource(true);
        this.emitTrackChange();
      }
    });
  }

  // --- Internal Mechanics ---

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

  private clearCustomOverrides(): void {
    if (this.customAudioUrl) {
      URL.revokeObjectURL(this.customAudioUrl);
    }
    this.customAudioUrl = null;
    this.customTitle = null;
    this.customLyrics = null;
  }

  private ensureAudioLoaded(): void {
    const snap = this.getSnapshot();
    const resolved = this.customAudioUrl ?? desktop.resolveMediaUrl(snap.track.audio);
    if (this.audio.getAttribute('data-src') !== resolved) {
      this.audio.src = resolved;
      this.audio.setAttribute('data-src', resolved);
      this.audio.load();
    }
  }

  private prepareAudioSource(autoPlay: boolean): void {
    if (!this.albums.length) return;
    const snap = this.getSnapshot();
    this.progress = 0;
    this.currentTime = 0;
    this.duration = snap.track.clipDur || 30;
    this.lyricAnchorIdx = snap.track.hookIdx || 0;
    this.activeLyricIdx = -1;

    this.ensureAudioLoaded();
    try {
      this.audio.currentTime = 0;
    } catch {
      // Initial load will start at 0 automatically
    }

    this.recomputeActiveLyric();
    this.syncMediaSessionMetadata(snap);

    if (autoPlay) {
      this.audio.play().then(() => {
        this.playing = true;
        this.syncMediaSessionPlayback();
        this.emitTrackChange();
      }).catch(() => {
        this.playing = false;
        this.syncMediaSessionPlayback();
        this.emitTrackChange();
      });
    }
  }

  private recomputeActiveLyric(): void {
    const snap = this.getSnapshot();
    const lines = snap.lyrics;
    if (!lines.length) {
      this.activeLyricIdx = -1;
      return;
    }

    let idx = 0;
    if (this.duration > 45 && lines[0].t !== null) {
      for (let i = 0; i < lines.length; i++) {
        const ts = lines[i].t;
        if (ts !== null && this.currentTime >= ts) idx = i;
      }
    } else {
      const stepOffset = Math.floor(this.currentTime / 3.1);
      idx = Math.min(lines.length - 1, this.lyricAnchorIdx + stepOffset);
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
    if (!('mediaSession' in navigator) || typeof MediaMetadata === 'undefined') return;
    const coverUrl = desktop.resolveMediaUrl(snap.album.cover);
    navigator.mediaSession.metadata = new MediaMetadata({
      title: snap.displayTitle,
      artist: 'ヨルシカ (Yorushika)',
      album: snap.album.title,
      artwork: coverUrl ? [{ src: coverUrl, sizes: '600x600', type: 'image/jpeg' }] : [],
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

export function parseLrcText(raw: string): LyricLine[] {
  return raw
    .split(/\r?\n/)
    .map((line) => {
      const trimmed = line.trim();
      if (!trimmed) return null;
      const m = trimmed.match(/^\[(\d+):(\d+(?:\.\d+)?)\]\s*(.*)$/);
      if (m) {
        const text = m[3].trim();
        return text ? { t: Number(m[1]) * 60 + Number(m[2]), text } : null;
      }
      if (!trimmed.startsWith('[')) {
        return { t: null, text: trimmed };
      }
      return null;
    })
    .filter((item): item is LyricLine => item !== null);
}
