// Application-wide state: backend info, settings (persisted), theme and overlays.

import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from '../api';
import { setOs } from '../platform';
import { isDarkTheme, resolveTheme } from '../themes';
import type { AppInfo, ResolvedTheme, Server, Settings } from '../types';

export const DEFAULT_SETTINGS: Settings = {
  theme: 'light',
  fontFamily: '',
  fontSize: 13,
  lineHeight: 1.25,
  cursorStyle: 'bar',
  cursorBlink: false,
  scrollback: 10000,
  copyOnSelect: false,
  rightClickPaste: false,
  gpuAcceleration: true,
  ctrlKInTerminal: false,
  sidebarWidth: 240,
  sidebarHidden: false,
  collapsedGroups: [],
};

export interface MenuItem {
  label: string;
  icon?: string;
  hint?: string;
  danger?: boolean;
  disabled?: boolean;
  action: () => void;
}

export type MenuEntry = MenuItem | 'separator';

export interface MenuState {
  x: number;
  y: number;
  items: MenuEntry[];
}

export interface EditorState {
  server: Server | null;
  preset?: Partial<Server>;
}

export interface ConfirmState {
  title: string;
  message: string;
  confirmLabel: string;
  danger: boolean;
  /** When set, the dialog shows a text field with this initial value. */
  input?: string;
  resolve: (value: string | null) => void;
}

const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');

class AppState {
  info = $state<AppInfo | null>(null);
  settings = $state<Settings>({ ...DEFAULT_SETTINGS });
  systemDark = $state(darkQuery.matches);
  theme: ResolvedTheme = $derived(resolveTheme(this.settings.theme, this.systemDark));
  keychainIssue = $state<string | null>(null);
  fullscreen = $state(false);

  palette = $state<{ open: boolean; mode: 'commands' | 'connect'; query: string }>({
    open: false,
    mode: 'commands',
    query: '',
  });
  editor = $state<EditorState | null>(null);
  settingsOpen = $state(false);
  forwardsOpen = $state(false);
  menu = $state<MenuState | null>(null);
  confirmation = $state<ConfirmState | null>(null);

  #saveTimer: ReturnType<typeof setTimeout> | undefined;

  get overlayOpen(): boolean {
    return this.palette.open || !!this.editor || this.settingsOpen || this.forwardsOpen || !!this.confirmation;
  }

  async init() {
    darkQuery.addEventListener('change', (e) => (this.systemDark = e.matches));
    $effect.root(() => {
      $effect(() => applyTheme(this.theme));
    });
    try {
      const info = await api.appInfo();
      setOs(info.os);
      this.info = info;
      this.settings = { ...DEFAULT_SETTINGS, ...(info.settings ?? {}) };
    } catch (e) {
      console.error('app_info failed', e);
    }
    api
      .keychainStatus()
      .then((issue) => (this.keychainIssue = issue))
      .catch(() => {});
  }

  /** Updates settings and persists them (debounced). */
  update(patch: Partial<Settings>) {
    Object.assign(this.settings, patch);
    clearTimeout(this.#saveTimer);
    this.#saveTimer = setTimeout(() => {
      api.saveSettings($state.snapshot(this.settings)).catch((e) => console.error('settings', e));
    }, 300);
  }

  openPalette(mode: 'commands' | 'connect' = 'commands', query = '') {
    this.menu = null;
    this.palette = { open: true, mode, query };
  }

  closePalette() {
    this.palette.open = false;
  }

  openEditor(server: Server | null, preset?: Partial<Server>) {
    this.menu = null;
    this.palette.open = false;
    this.editor = { server, preset };
  }

  showMenu(x: number, y: number, items: MenuEntry[]) {
    this.menu = { x, y, items };
  }

  /** In-app confirmation (native confirm() is not available in every webview). */
  async confirm(title: string, message: string, confirmLabel = 'OK', danger = false): Promise<boolean> {
    return (await this.#ask({ title, message, confirmLabel, danger })) !== null;
  }

  /** Asks for a line of text; `null` when cancelled. */
  prompt(title: string, message: string, initial: string, confirmLabel = 'Save'): Promise<string | null> {
    return this.#ask({ title, message, confirmLabel, danger: false, input: initial });
  }

  #ask(state: Omit<ConfirmState, 'resolve'>): Promise<string | null> {
    this.menu = null;
    return new Promise((resolve) => {
      this.confirmation = {
        ...state,
        resolve: (value) => {
          this.confirmation = null;
          resolve(value);
        },
      };
    });
  }

  async toggleFullscreen() {
    const win = getCurrentWindow();
    const next = !(await win.isFullscreen());
    await win.setFullscreen(next);
    this.fullscreen = next;
  }
}

function applyTheme(theme: ResolvedTheme) {
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem('nexssh.theme', theme);
  } catch {
    // Storage may be unavailable; the theme still applies for this session.
  }
  getCurrentWindow()
    .setTheme(isDarkTheme(theme) ? 'dark' : 'light')
    .catch(() => {});
}

export const app = new AppState();
