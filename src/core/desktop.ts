// DEEP MODULE 2: NATIVE CROSS-PLATFORM DESKTOP SHELL BRIDGE
// Hides Tauri v2 IPC, Windows/macOS frameless window drag, native OS file/folder pickers,
// WebView2 native file-drop paths, tray command listeners, multi-pack source management,
// and zero-copy asset:// URL resolution.

import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Album } from './player';

export type ImportKind = 'files' | 'folder';
export type LibrarySourceKind = 'zip' | 'folder' | 'files';

export interface LibrarySource {
  id: string;
  kind: LibrarySourceKind;
  name: string;
  path: string;
  originPath?: string;
  cover?: string;
  enabled: boolean;
  albumCount: number;
  trackCount: number;
  missing: boolean;
}

export interface LibraryState {
  albums: Album[];
  sources: LibrarySource[];
  selectAlbumId: string | null;
  importedTracks: number;
}

export interface DesktopHandlers {
  onPlayToggle: () => void;
  onStepTrack: (delta: 1 | -1) => void;
  onOpenSettings: () => void;
  onLibraryUpdated: (state: LibraryState, fromUserImport: boolean) => void;
}

class DesktopStageBridge {
  readonly isDesktop: boolean;

  constructor() {
    this.isDesktop = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
    if (this.isDesktop) {
      document.documentElement.classList.add('is-desktop');
    }
  }

  async init(handlers: DesktopHandlers): Promise<void> {
    if (!this.isDesktop) return;

    await Promise.all([
      listen('tray://play-toggle', () => handlers.onPlayToggle()),
      listen('tray://next-track', () => handlers.onStepTrack(1)),
      listen('tray://prev-track', () => handlers.onStepTrack(-1)),
      listen('tray://open-settings', () => handlers.onOpenSettings()),
      listen<LibraryState>('library://updated', (ev) => {
        if (ev.payload && Array.isArray(ev.payload.albums)) {
          handlers.onLibraryUpdated(
            {
              albums: ev.payload.albums,
              sources: Array.isArray(ev.payload.sources) ? ev.payload.sources : [],
              selectAlbumId: ev.payload.selectAlbumId ?? null,
              importedTracks: ev.payload.importedTracks ?? 0,
            },
            Boolean(ev.payload.selectAlbumId)
          );
        }
      }),
      getCurrentWebview().onDragDropEvent((ev) => {
        const payload = ev.payload;
        if (payload.type === 'enter' || payload.type === 'over') {
          document.body.classList.add('file-hover');
        } else if (payload.type === 'leave') {
          document.body.classList.remove('file-hover');
        } else if (payload.type === 'drop') {
          document.body.classList.remove('file-hover');
          if (payload.paths.length > 0) {
            invoke<LibraryState>('import_paths', { paths: payload.paths }).catch((err) => {
              console.error('Failed to import dropped paths:', err);
            });
          }
        }
      }),
    ]);

    try {
      const state = await invoke<LibraryState>('load_library');
      if (state && Array.isArray(state.albums)) {
        handlers.onLibraryUpdated(
          {
            albums: state.albums,
            sources: Array.isArray(state.sources) ? state.sources : [],
            selectAlbumId: null,
            importedTracks: 0,
          },
          false
        );
      }
    } catch (err) {
      console.error('Failed to load persisted library:', err);
    }
  }

  async pickAndImport(kind: ImportKind): Promise<LibraryState | null> {
    if (!this.isDesktop) return null;
    try {
      return await invoke<LibraryState | null>('pick_and_import', { kind });
    } catch (err) {
      console.error('Dialog import failed:', err);
      return null;
    }
  }

  async rescanLibrary(): Promise<void> {
    if (!this.isDesktop) return;
    try {
      await invoke<LibraryState>('rescan_library');
    } catch (err) {
      console.error('Failed to rescan library:', err);
    }
  }

  async toggleSource(sourceId: string): Promise<void> {
    if (!this.isDesktop || !sourceId) return;
    try {
      await invoke<LibraryState>('toggle_library_source', { sourceId });
    } catch (err) {
      console.error('Failed to toggle library source:', err);
    }
  }

  async reimportSource(sourceId: string): Promise<void> {
    if (!this.isDesktop || !sourceId) return;
    try {
      await invoke<LibraryState | null>('reimport_library_source', { sourceId });
    } catch (err) {
      console.error('Failed to reimport library source:', err);
    }
  }

  async revealSource(sourceId: string): Promise<void> {
    if (!this.isDesktop || !sourceId) return;
    try {
      await invoke('reveal_library_source', { sourceId });
    } catch (err) {
      console.error('Failed to reveal library source:', err);
    }
  }

  async removeSource(sourceId: string): Promise<void> {
    if (!this.isDesktop || !sourceId) return;
    try {
      await invoke<LibraryState>('remove_library_source', { sourceId });
    } catch (err) {
      console.error('Failed to remove library source:', err);
    }
  }

  async clearLibrary(): Promise<void> {
    if (!this.isDesktop) return;
    try {
      await invoke<LibraryState>('clear_library');
    } catch (err) {
      console.error('Failed to clear library:', err);
    }
  }

  bindChassisShell(frameEl: HTMLElement | null, grilleEl: HTMLElement | null): void {
    if (!this.isDesktop) return;
    const win = getCurrentWindow();

    const startDragIfPrimary = (e: PointerEvent) => {
      if (e.button !== 0 || e.ctrlKey) return;
      e.preventDefault();
      win.startDragging().catch(() => {});
    };

    // Outer pale maple wood frame drags window when grabbed directly
    frameEl?.addEventListener('pointerdown', (e) => {
      if (e.target === frameEl) {
        startDragIfPrimary(e);
      }
    });

    // Acoustic speaker grille also acts as a natural thumb-hold for moving the radio
    grilleEl?.addEventListener('pointerdown', startDragIfPrimary);

    // Right-clicking anywhere on the chassis opens the native OS placement/library menu
    frameEl?.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      invoke('popup_chassis_menu').catch(() => {});
    });
  }

  resolveMediaUrl(pathOrUrl: string): string {
    if (!pathOrUrl) return '';
    if (pathOrUrl.startsWith('komapack://')) {
      return this.isDesktop ? convertFileSrc(pathOrUrl.slice('komapack://'.length), 'komapack') : '';
    }
    if (
      !this.isDesktop ||
      pathOrUrl.startsWith('http://') ||
      pathOrUrl.startsWith('https://') ||
      pathOrUrl.startsWith('blob:') ||
      pathOrUrl.startsWith('data:') ||
      pathOrUrl.startsWith('asset:')
    ) {
      return pathOrUrl;
    }
    return convertFileSrc(pathOrUrl);
  }
}

export const desktop = new DesktopStageBridge();
