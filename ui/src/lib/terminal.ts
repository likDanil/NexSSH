// xterm.js wrapper. Loaded lazily (dynamic import) when the first session opens, so the
// app starts without parsing the terminal emulator.

import { Terminal, type ITheme, type IWindowsPty } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import '@fontsource-variable/jetbrains-mono/wght.css';
import { isMac, mod, shortcutKey } from './platform';
import type { Settings } from './types';

export const BUNDLED_FONT = 'JetBrains Mono Variable';
const FALLBACK_FONTS = `'JetBrains Mono', 'SF Mono', 'Geist Mono', 'Cascadia Mono', Menlo, Consolas, 'DejaVu Sans Mono', monospace`;

export type TermSettings = Pick<
  Settings,
  'fontFamily' | 'fontSize' | 'lineHeight' | 'cursorStyle' | 'cursorBlink' | 'scrollback' | 'copyOnSelect' | 'gpuAcceleration'
>;

/** Control characters but tab and line breaks, which pastes leave out (see `pasteText`). */
const PASTE_CONTROLS = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/g;

function fontStack(custom: string): string {
  const user = custom.trim();
  const quoted = user && !/[,'"]/.test(user) ? `'${user}'` : user;
  return [quoted, `'${BUNDLED_FONT}'`, FALLBACK_FONTS].filter(Boolean).join(', ');
}

/** A link under the mouse: a web address in the output, or a hyperlink (OSC 8), whose text
 * may say something else than where it points. */
export interface HoveredLink {
  uri: string;
  hyperlink: boolean;
  /** Where the mouse came onto it (client coordinates). */
  x: number;
  y: number;
}

export interface TermCallbacks {
  onData(data: string): void;
  onBinary(data: string): void;
  onResize(cols: number, rows: number): void;
  onContextMenu(e: MouseEvent, view: TermView): void;
  /** Ctrl+click (Cmd+click on macOS) on a link. */
  onLinkOpen(uri: string): void;
  /** The mouse came onto a link, or left it (`null`). */
  onLinkHover(link: HoveredLink | null): void;
  /** Ctrl+wheel: the font a step bigger (1) or smaller (-1). */
  onZoom(step: 1 | -1): void;
  /** Whether `text` may be pasted (`bracketed`: the program takes pastes as text). */
  allowPaste(text: string, bracketed: boolean): Promise<boolean>;
}

export class TermView {
  readonly term: Terminal;
  #fit = new FitAddon();
  #webgl: WebglAddon | null = null;
  #copyTimer: ReturnType<typeof setTimeout> | undefined;
  #settings: TermSettings;
  #link: string | null = null;
  #cb: TermCallbacks;

  /** `windowsPty`: the session is a local terminal on Windows (ConPTY), which xterm.js adapts to. */
  constructor(
    host: HTMLElement,
    settings: TermSettings,
    theme: ITheme,
    cb: TermCallbacks,
    windowsPty?: IWindowsPty,
  ) {
    this.#settings = settings;
    this.#cb = cb;
    // Links open with Ctrl+click (Cmd+click on macOS), as in most terminals: a plain click
    // stays a click, for selecting text and for programs that use the mouse.
    const activate = (e: MouseEvent, uri: string) => {
      if (mod(e)) cb.onLinkOpen(uri);
    };
    const hover = (hyperlink: boolean) => (e: MouseEvent, uri: string) => {
      this.#link = uri;
      cb.onLinkHover({ uri, hyperlink, x: e.clientX, y: e.clientY });
    };
    const leave = () => {
      this.#link = null;
      cb.onLinkHover(null);
    };
    this.term = new Terminal({
      windowsPty,
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
      // Hyperlinks programs print (OSC 8, e.g. `ls --hyperlink`); only web ones.
      linkHandler: { activate, hover: hover(true), leave },
    });
    this.term.loadAddon(this.#fit);
    // Web addresses in the output.
    this.term.loadAddon(new WebLinksAddon(activate, { hover: hover(false), leave }));
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
    // Pastes xterm would take by itself (Cmd+V on macOS, a middle click on Linux) go the same
    // way as the app's own.
    host.addEventListener(
      'paste',
      (e) => {
        e.preventDefault();
        e.stopPropagation();
        const text = e.clipboardData?.getData('text/plain');
        if (text) void this.pasteText(text);
      },
      { capture: true },
    );
    // Ctrl+wheel (a touchpad's pinch too) zooms instead of scrolling, before xterm sees it: a
    // step per notch of a mouse wheel, or per bit of a touchpad's movement.
    let wheel = 0;
    host.addEventListener(
      'wheel',
      (e) => {
        if (!e.ctrlKey && !(isMac() && e.metaKey)) return;
        e.preventDefault();
        e.stopPropagation();
        wheel += e.deltaMode === WheelEvent.DOM_DELTA_PIXEL ? e.deltaY : e.deltaY * 50;
        if (Math.abs(wheel) >= 50) {
          cb.onZoom(wheel < 0 ? 1 : -1);
          wheel = 0;
        }
      },
      { capture: true, passive: false },
    );
  }

  /** Clipboard shortcuts; everything else goes to the shell. */
  #onKey(e: KeyboardEvent): boolean {
    if (e.type !== 'keydown') return true;
    // Also with a Cyrillic (or other non-Latin) layout active.
    const key = shortcutKey(e).toLowerCase();
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

  /** The link under the mouse, if any. */
  get link(): string | null {
    return this.#link;
  }

  async copy() {
    const text = this.term.getSelection();
    if (text) await navigator.clipboard.writeText(text).catch(() => {});
  }

  async paste() {
    const text = await navigator.clipboard.readText().catch(() => '');
    if (text) await this.pasteText(text);
  }

  /** Pastes `text` if the user agrees, when asking is due (see `TermCallbacks.allowPaste`). */
  async pasteText(text: string) {
    // Without control characters, as in Windows Terminal: they would reach the program as keys
    // (Ctrl+O runs the line in bash, with no line break), and an escape sequence could end a
    // bracketed paste early, so that the rest runs as if typed.
    const clean = text.replace(PASTE_CONTROLS, '');
    if (!clean || !(await this.#cb.allowPaste(clean, this.term.modes.bracketedPasteMode))) return;
    this.term.paste(clean);
    this.term.focus();
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
