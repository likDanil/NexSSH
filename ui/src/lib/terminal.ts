// xterm.js wrapper. Loaded lazily (dynamic import) when the first session opens, so the
// app starts without parsing the terminal emulator.

import { Terminal, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import '@fontsource-variable/jetbrains-mono/wght.css';
import { isMac } from './platform';
import type { Settings } from './types';

export const BUNDLED_FONT = 'JetBrains Mono Variable';
const FALLBACK_FONTS = `'JetBrains Mono', 'SF Mono', 'Geist Mono', 'Cascadia Mono', Menlo, Consolas, 'DejaVu Sans Mono', monospace`;

export type TermSettings = Pick<
  Settings,
  'fontFamily' | 'fontSize' | 'lineHeight' | 'cursorStyle' | 'cursorBlink' | 'scrollback' | 'copyOnSelect' | 'gpuAcceleration'
>;

function fontStack(custom: string): string {
  const user = custom.trim();
  const quoted = user && !/[,'"]/.test(user) ? `'${user}'` : user;
  return [quoted, `'${BUNDLED_FONT}'`, FALLBACK_FONTS].filter(Boolean).join(', ');
}

export interface TermCallbacks {
  onData(data: string): void;
  onBinary(data: string): void;
  onResize(cols: number, rows: number): void;
  onContextMenu(e: MouseEvent, view: TermView): void;
}

export class TermView {
  readonly term: Terminal;
  #fit = new FitAddon();
  #webgl: WebglAddon | null = null;
  #copyTimer: ReturnType<typeof setTimeout> | undefined;
  #settings: TermSettings;

  constructor(host: HTMLElement, settings: TermSettings, theme: ITheme, cb: TermCallbacks) {
    this.#settings = settings;
    this.term = new Terminal({
      fontFamily: fontStack(settings.fontFamily),
      fontSize: settings.fontSize,
      lineHeight: settings.lineHeight,
      fontWeight: 400,
      fontWeightBold: 650,
      cursorStyle: settings.cursorStyle,
      cursorInactiveStyle: 'outline',
      cursorBlink: settings.cursorBlink,
      scrollback: settings.scrollback,
      theme,
      drawBoldTextInBrightColors: true,
      minimumContrastRatio: 3,
      smoothScrollDuration: 0,
      macOptionClickForcesSelection: true,
      rightClickSelectsWord: false,
      allowProposedApi: false,
    });
    this.term.loadAddon(this.#fit);
    this.term.open(host);
    this.setGpu(settings.gpuAcceleration);

    this.term.onData(cb.onData);
    this.term.onBinary(cb.onBinary);
    this.term.onResize(({ cols, rows }) => cb.onResize(cols, rows));
    this.term.onSelectionChange(() => {
      if (!this.#settings.copyOnSelect) return;
      clearTimeout(this.#copyTimer);
      this.#copyTimer = setTimeout(() => {
        if (this.term.hasSelection()) void this.copy();
      }, 120);
    });
    this.term.attachCustomKeyEventHandler((e) => this.#onKey(e));
    host.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      cb.onContextMenu(e, this);
    });
  }

  /** Clipboard shortcuts; everything else goes to the shell. */
  #onKey(e: KeyboardEvent): boolean {
    if (e.type !== 'keydown') return true;
    const key = e.key.toLowerCase();
    if (isMac()) {
      if (e.metaKey && key === 'c' && this.term.hasSelection()) {
        void this.copy();
        return false;
      }
      // Cmd+V: the native paste event reaches xterm without a permission prompt.
      return !(e.metaKey && (key === 'v' || key === 'a'));
    }
    const copy =
      (e.ctrlKey && e.shiftKey && key === 'c') ||
      (e.ctrlKey && !e.shiftKey && key === 'insert') ||
      // Like Windows Terminal: Ctrl+C copies when there is a selection, otherwise it is ^C.
      (e.ctrlKey && !e.shiftKey && !e.altKey && key === 'c' && this.term.hasSelection());
    if (copy) {
      e.preventDefault();
      void this.copy().then(() => this.term.clearSelection());
      return false;
    }
    const paste = (e.ctrlKey && e.shiftKey && key === 'v') || (e.shiftKey && !e.ctrlKey && key === 'insert');
    if (paste) {
      e.preventDefault();
      void this.paste();
      return false;
    }
    return true;
  }

  async copy() {
    const text = this.term.getSelection();
    if (text) await navigator.clipboard.writeText(text).catch(() => {});
  }

  async paste() {
    const text = await navigator.clipboard.readText().catch(() => '');
    if (text) this.term.paste(text);
  }

  write(data: Uint8Array | string) {
    this.term.write(data);
  }

  focus() {
    this.term.focus();
  }

  clear() {
    this.term.clear();
  }

  /** Resizes to the container; returns true if the grid size changed. */
  fit(): boolean {
    const { cols, rows } = this.term;
    const dims = this.#fit.proposeDimensions();
    if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows) || dims.cols < 2 || dims.rows < 1) {
      return false;
    }
    if (dims.cols !== cols || dims.rows !== rows) {
      this.term.resize(dims.cols, dims.rows);
      return true;
    }
    return false;
  }

  setGpu(on: boolean) {
    if (on && !this.#webgl) {
      try {
        const addon = new WebglAddon();
        addon.onContextLoss(() => {
          addon.dispose();
          if (this.#webgl === addon) this.#webgl = null;
        });
        this.term.loadAddon(addon);
        this.#webgl = addon;
      } catch {
        // No WebGL2 available: the DOM renderer stays in use.
        this.#webgl = null;
      }
    } else if (!on && this.#webgl) {
      this.#webgl.dispose();
      this.#webgl = null;
    }
  }

  apply(settings: TermSettings, theme: ITheme) {
    const prev = this.#settings;
    this.#settings = settings;
    const o = this.term.options;
    if (prev.fontFamily !== settings.fontFamily) o.fontFamily = fontStack(settings.fontFamily);
    if (o.fontSize !== settings.fontSize) o.fontSize = settings.fontSize;
    if (o.lineHeight !== settings.lineHeight) o.lineHeight = settings.lineHeight;
    if (o.cursorStyle !== settings.cursorStyle) o.cursorStyle = settings.cursorStyle;
    if (o.cursorBlink !== settings.cursorBlink) o.cursorBlink = settings.cursorBlink;
    if (o.scrollback !== settings.scrollback) o.scrollback = settings.scrollback;
    if (o.theme !== theme) o.theme = theme;
    if (prev.gpuAcceleration !== settings.gpuAcceleration) this.setGpu(settings.gpuAcceleration);
  }

  dispose() {
    clearTimeout(this.#copyTimer);
    this.#webgl?.dispose();
    this.term.dispose();
  }
}

let fontReady: Promise<void> | null = null;

/** Glyph metrics must be measured with the real font, so wait for it once. */
export function ensureFont(size: number): Promise<void> {
  fontReady ??= document.fonts
    .load(`${size}px "${BUNDLED_FONT}"`)
    .then(() => undefined)
    .catch(() => undefined);
  return fontReady;
}
