import type { ITheme } from '@xterm/xterm';
import type { ResolvedTheme, ThemeId } from './types';

export interface ThemeMeta {
  id: ThemeId;
  label: string;
  /** Colors for the preview card in Settings: frame, surface, text, accent. */
  preview: [string, string, string, string];
}

export const THEMES: ThemeMeta[] = [
  { id: 'system', label: 'System', preview: ['#eceef1', '#2a2d31', '#8a919c', '#5cb887'] },
  { id: 'light', label: 'Light', preview: ['#eceef1', '#f7f8f9', '#3f4650', '#5cb887'] },
  { id: 'graphite', label: 'Graphite', preview: ['#222428', '#2a2d31', '#d6d9de', '#5cb887'] },
  { id: 'black', label: 'Black', preview: ['#0c0d0f', '#141517', '#d8dbe0', '#5cb887'] },
  { id: 'navy', label: 'Navy', preview: ['#171c29', '#1d2434', '#d5dce8', '#62c292'] },
];

/** `system` follows the OS: Light by day, Graphite at night. */
export function resolveTheme(id: ThemeId, systemDark: boolean): ResolvedTheme {
  if (id === 'system') return systemDark ? 'graphite' : 'light';
  return id;
}

export const isDarkTheme = (t: ResolvedTheme) => t !== 'light';

const light: ITheme = {
  background: '#f7f8f9',
  foreground: '#3f4650',
  cursor: '#3f4650',
  cursorAccent: '#f7f8f9',
  selectionBackground: 'rgba(58, 111, 192, 0.2)',
  selectionInactiveBackground: 'rgba(63, 70, 80, 0.1)',
  scrollbarSliderBackground: 'rgba(63, 70, 80, 0.16)',
  scrollbarSliderHoverBackground: 'rgba(63, 70, 80, 0.28)',
  scrollbarSliderActiveBackground: 'rgba(63, 70, 80, 0.36)',
  black: '#3f4650',
  red: '#c2504c',
  green: '#2f8a5e',
  yellow: '#a57a1f',
  blue: '#3a6fc0',
  magenta: '#9150b0',
  cyan: '#2a8593',
  white: '#6f7682',
  brightBlack: '#8a919c',
  brightRed: '#d4625d',
  brightGreen: '#3d9d6d',
  brightYellow: '#b98c2c',
  brightBlue: '#4d84d2',
  brightMagenta: '#a464c2',
  brightCyan: '#3597a6',
  brightWhite: '#8a919c',
};

const darkAnsi = {
  red: '#e0706b',
  green: '#6cc494',
  yellow: '#dfb05c',
  blue: '#6fa0e3',
  magenta: '#c08ae0',
  cyan: '#5ebfc6',
  white: '#c9cdd3',
  brightRed: '#ec8782',
  brightGreen: '#82d4a7',
  brightYellow: '#eac074',
  brightBlue: '#88b3ee',
  brightMagenta: '#d0a2ec',
  brightCyan: '#79d0d6',
  brightWhite: '#eef0f3',
};

const graphite: ITheme = {
  ...darkAnsi,
  background: '#2a2d31',
  foreground: '#d6d9de',
  cursor: '#d6d9de',
  cursorAccent: '#2a2d31',
  selectionBackground: 'rgba(136, 179, 238, 0.24)',
  selectionInactiveBackground: 'rgba(214, 217, 222, 0.1)',
  scrollbarSliderBackground: 'rgba(214, 217, 222, 0.14)',
  scrollbarSliderHoverBackground: 'rgba(214, 217, 222, 0.26)',
  scrollbarSliderActiveBackground: 'rgba(214, 217, 222, 0.34)',
  black: '#3b3f46',
  brightBlack: '#6c727c',
};

const black: ITheme = {
  ...darkAnsi,
  background: '#141517',
  foreground: '#d8dbe0',
  cursor: '#d8dbe0',
  cursorAccent: '#141517',
  selectionBackground: 'rgba(136, 179, 238, 0.22)',
  selectionInactiveBackground: 'rgba(216, 219, 224, 0.08)',
  scrollbarSliderBackground: 'rgba(216, 219, 224, 0.12)',
  scrollbarSliderHoverBackground: 'rgba(216, 219, 224, 0.24)',
  scrollbarSliderActiveBackground: 'rgba(216, 219, 224, 0.32)',
  black: '#2b2e33',
  brightBlack: '#6a7079',
};

const navy: ITheme = {
  background: '#1d2434',
  foreground: '#d5dce8',
  cursor: '#d5dce8',
  cursorAccent: '#1d2434',
  selectionBackground: 'rgba(147, 186, 245, 0.24)',
  selectionInactiveBackground: 'rgba(213, 220, 232, 0.1)',
  scrollbarSliderBackground: 'rgba(170, 186, 214, 0.14)',
  scrollbarSliderHoverBackground: 'rgba(170, 186, 214, 0.26)',
  scrollbarSliderActiveBackground: 'rgba(170, 186, 214, 0.34)',
  black: '#2c3548',
  red: '#e5807c',
  green: '#6fcb98',
  yellow: '#e2b867',
  blue: '#79a6ee',
  magenta: '#c792ea',
  cyan: '#66c7d0',
  white: '#c3ccdb',
  brightBlack: '#5f6b84',
  brightRed: '#f09490',
  brightGreen: '#86d9ab',
  brightYellow: '#ecc77f',
  brightBlue: '#93baf5',
  brightMagenta: '#d5a8f0',
  brightCyan: '#82d5dc',
  brightWhite: '#eef2f8',
};

export const TERMINAL_THEMES: Record<ResolvedTheme, ITheme> = { light, graphite, black, navy };
