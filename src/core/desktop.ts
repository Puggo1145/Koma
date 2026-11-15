// DEEP MODULE 2: NATIVE DESKTOP SHELL BRIDGE
// Hides Tauri v2 IPC, macOS frameless window drag, context menu invocation,
// tray command listeners, scale synchronization, and local asset:// path resolution.

import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Album } from './player';

export type ChassisScale = 'standard' | 'compact';

export interface DesktopHandlers {
  onPlayToggle: () => void;
  onStepTrack: (delta: 1 | -1) => void;
  onLocalAlbumsDiscovered: (albums: Album[]) => void;
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

    // Subscribe to native macOS Menu Bar Tray & Context Menu commands
    await Promise.all([
      listen('tray://play-toggle', () => handlers.onPlayToggle()),
      listen('tray://next-track', () => handlers.onStepTrack(1)),
      listen('tray://prev-track', () => handlers.onStepTrack(-1)),
      listen<ChassisScale>('stage://scale-changed', (ev) => {
        document.documentElement.classList.toggle('scale-compact', ev.payload === 'compact');
      }),
      listen<Album[]>('library://updated', (ev) => {
        if (Array.isArray(ev.payload) && ev.payload.length > 0) {
          handlers.onLocalAlbumsDiscovered(ev.payload);
        }
      }),
    ]);

    // Auto-discover any lossless albums placed in ~/Music/Yorushika
    try {
      const localAlbums = await invoke<Album[]>('scan_local_yorushika');
      if (Array.isArray(localAlbums) && localAlbums.length > 0) {
        handlers.onLocalAlbumsDiscovered(localAlbums);
      }
    } catch {
      // Built-in library continues seamlessly if no external folder exists
    }
  }

  bindChassisShell(frameEl: HTMLElement | null, grilleEl: HTMLElement | null): void {
    if (!this.isDesktop) return;
    const win = getCurrentWindow();

    const startDragIfPrimary = (e: PointerEvent) => {
      if (e.button !== 0) return;
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

    // Right-clicking anywhere on the chassis opens the native macOS placement/size menu
    frameEl?.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      invoke('popup_chassis_menu').catch(() => {});
    });
  }

  resolveMediaUrl(pathOrUrl: string): string {
    if (!pathOrUrl) return '';
    if (
      !this.isDesktop ||
      pathOrUrl.startsWith('/') && pathOrUrl.startsWith('/library/') ||
      pathOrUrl.startsWith('http://') ||
      pathOrUrl.startsWith('https://') ||
      pathOrUrl.startsWith('blob:') ||
      pathOrUrl.startsWith('data:')
    ) {
      return pathOrUrl;
    }
    // Absolute filesystem path from Rust local discography scanner -> zero-copy asset:// URL
    return convertFileSrc(pathOrUrl);
  }
}

export const desktop = new DesktopStageBridge();
