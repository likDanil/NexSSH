// Application-wide state: backend info, settings (persisted), theme and overlays.

import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from '../api';
import { i18n, resolveLanguage, systemLanguage, t, type Lang } from '../i18n.svelte';
import { setOs } from '../platform';
import { isDarkTheme, resolveTheme } from '../themes';
import type { AppInfo, ExplorerMenuState, ResolvedTheme, Server, Settings } from '../types';

export const DEFAULT_SETTINGS: Settings = {
  language: 'system',
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
  filesShowHidden: false,
  filesWidth: 380,
  filesSort: 'name',
  filesSortDesc: false,
  localShell: '',
  localShellCommand: '',
  explorerMenu: true,
  autoUpdateCheck: true,
  lastVersion: '',
};

/** The terminal's font size goes from 9 to 28 (settings and Ctrl+=/Ctrl+-). */
export function clampFontSize(size: number): number {
  return Math.min(28, Math.max(9, size));
}

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

export type SettingsSection = 'appearance' | 'terminal' | 'keyboard' | 'about';

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
  /** The field holds a file name: it opens with the name selected, not the extension. */
  fileName?: boolean;
  resolve: (value: string | null) => void;
}

export interface PermissionsChoice {
  mode: number;
  /** Also change everything inside the chosen folders. */
  recursive: boolean;
}

export interface PermissionsState {
  title: string;
  /** The mode to start from (the first item's when several are chosen). */
  mode: number;
  /** Folders are among the items: offer to apply the mode inside them too. */
  folders: boolean;
  resolve: (value: PermissionsChoice | null) => void;
}

const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');

class AppState {
  info = $state<AppInfo | null>(null);
  settings = $state<Settings>({ ...DEFAULT_SETTINGS });
  systemDark = $state(darkQuery.matches);
  theme: ResolvedTheme = $derived(resolveTheme(this.settings.theme, this.systemDark));
  systemLanguage = $state<Lang>(systemLanguage());
  language: Lang = $derived(resolveLanguage(this.settings.language, this.systemLanguage));
  keychainIssue = $state<string | null>(null);
  /** Windows: where Explorer shows "Open with NexSSH" (`null` until known). */
  explorerMenuState = $state<ExplorerMenuState | null>(null);
  fullscreen = $state(false);

  palette = $state<{ open: boolean; mode: 'commands' | 'connect'; query: string }>({
    open: false,
    mode: 'commands',
    query: '',
  });
  editor = $state<EditorState | null>(null);
  settingsOpen = $state(false);
  /** The section the settings dialog opens at. */
  settingsSection = $state<SettingsSection>('appearance');
  forwardsOpen = $state(false);
  menu = $state<MenuState | null>(null);
  confirmation = $state<ConfirmState | null>(null);
  permissions = $state<PermissionsState | null>(null);
  /** The terminal's font size, shown for a moment after a zoom shortcut changed it. */
  zoomBadge = $state<number | null>(null);

  #saveTimer: ReturnType<typeof setTimeout> | undefined;
  #zoomTimer: ReturnType<typeof setTimeout> | undefined;

  get overlayOpen(): boolean {
    return (
      this.palette.open ||
      !!this.editor ||
      this.settingsOpen ||
      this.forwardsOpen ||
      !!this.confirmation ||
      !!this.permissions
    );
  }

  async init() {
    darkQuery.addEventListener('change', (e) => (this.systemDark = e.matches));
    window.addEventListener('languagechange', () => (this.systemLanguage = systemLanguage()));
    this.#watchFullscreen();
    try {
      const info = await api.appInfo();
      setOs(info.os);
      this.info = info;
      this.settings = { ...DEFAULT_SETTINGS, ...(info.settings ?? {}) };
    } catch (e) {
      console.error('app_info failed', e);
    }
    // Before anything renders or asks the backend for messages.
    await this.#applyLanguage(this.language);
    $effect.root(() => {
      $effect(() => applyTheme(this.theme));
      $effect(() => void this.#applyLanguage(this.language));
    });
    this.refreshKeychainStatus();
  }

  refreshKeychainStatus() {
    api
      .keychainStatus()
      .then((issue) => (this.keychainIssue = issue))
      .catch(() => {});
  }

  #appliedLanguage: Lang | null = null;

  /** Switches the interface and the backend's messages to `lang`. */
  async #applyLanguage(lang: Lang) {
    i18n.lang = lang;
    document.documentElement.lang = lang;
    if (this.#appliedLanguage === lang) return;
    const first = this.#appliedLanguage === null;
    this.#appliedLanguage = lang;
    await api.setLanguage(lang).catch(() => {});
    // The keychain problem (if any) is described by the backend in its language.
    if (!first && this.keychainIssue) this.refreshKeychainStatus();
    // Also at start-up: the entry then points at this copy of NexSSH.
    this.syncExplorerMenu();
  }

  /** Windows: puts "Open with NexSSH" into Explorer's menu for folders (labelled in the
   * current language), or takes it out. */
  syncExplorerMenu() {
    if (this.info?.os !== 'windows') return;
    api
      .setExplorerMenu(this.settings.explorerMenu)
      .then((state) => (this.explorerMenuState = state))
      .catch((e) => console.error('explorer menu', e));
  }

  /** Windows 11: moves "Open with NexSSH" into Explorer's compact menu, once the user allowed
   * it with administrator rights (Windows asks). */
  async trustExplorerMenu() {
    const state = await api.trustExplorerMenu();
    this.explorerMenuState = state;
    return state;
  }

  /** The terminal's font size, like a browser's zoom: a step bigger (1), smaller (-1), or
   * back to the default (0). It is the setting, so it stays. */
  zoom(step: -1 | 0 | 1) {
    const size = step === 0 ? DEFAULT_SETTINGS.fontSize : clampFontSize(this.settings.fontSize + step);
    if (size !== this.settings.fontSize) this.update({ fontSize: size });
    this.zoomBadge = size;
    clearTimeout(this.#zoomTimer);
    this.#zoomTimer = setTimeout(() => (this.zoomBadge = null), 1200);
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
  async confirm(title: string, message: string, confirmLabel = t('common.ok'), danger = false): Promise<boolean> {
    return (await this.#ask({ title, message, confirmLabel, danger })) !== null;
  }

  /** Asks for a line of text; `null` when cancelled. */
  prompt(
    title: string,
    message: string,
    initial: string,
    confirmLabel = t('common.save'),
    fileName = false,
  ): Promise<string | null> {
    return this.#ask({ title, message, confirmLabel, danger: false, input: initial, fileName });
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

  /** The permissions dialog (chmod); `null` when cancelled. */
  askPermissions(title: string, mode: number, folders: boolean): Promise<PermissionsChoice | null> {
    this.menu = null;
    return new Promise((resolve) => {
      this.permissions = {
        title,
        mode,
        folders,
        resolve: (value) => {
          this.permissions = null;
          resolve(value);
        },
      };
    });
  }

  async toggleFullscreen() {
    const next = !(await getCurrentWindow().isFullscreen());
    await api.setFullscreen(next);
    this.fullscreen = next;
  }

  #fullscreenTimer: ReturnType<typeof setTimeout> | undefined;

  /** Full screen can also end outside the app (e.g. a system shortcut). */
  #watchFullscreen() {
    const win = getCurrentWindow();
    void win.onResized(() => {
      clearTimeout(this.#fullscreenTimer);
      this.#fullscreenTimer = setTimeout(() => {
        win
          .isFullscreen()
          .then((f) => (this.fullscreen = f))
          .catch(() => {});
      }, 150);
    });
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
