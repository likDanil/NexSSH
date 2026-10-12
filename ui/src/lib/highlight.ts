// Keyword highlighting: words like error and warning, IP and MAC addresses get their own color
// in a terminal's output. Apart from xterm, so that it is tested without a terminal
// (ui/tests/highlight.test.ts); `TermView` puts the colors on the screen.

import type { HighlightKind } from './types';

/** In the order the settings show them. */
export const HIGHLIGHT_KINDS: readonly HighlightKind[] = ['error', 'warning', 'ok', 'info', 'debug', 'address'];

/** Whole words in any case: `[ERROR]`, `Error:` and `error` alike, but not `errorCount`. */
const WORDS: Record<Exclude<HighlightKind, 'address'>, string> = {
  error: 'errors?|fatal|fail(?:s|ed|ure|ures)?|denied|refused|exceptions?|critical|panic',
  warning: 'warn(?:s|ing|ings)?|deprecated',
  ok: 'ok|okay|success(?:ful|fully)?|succeeded|passed|done',
  info: 'info|information|notice',
  debug: 'debug|trace',
};
// Next to a letter (of any script), a digit or `_` a keyword is a part of another word: `error2`.
const WORD_START = '(?<![\\p{L}\\p{N}_])';
const WORD_END = '(?![\\p{L}\\p{N}_])';

const OCTET = '(?:25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)';
/** 192.168.0.1 (also before a port or a prefix length), not a part of a version like 1.2.3.4.5. */
const IPV4 = `(?<![\\w.])(?:${OCTET}\\.){3}${OCTET}(?!\\w|\\.\\d)`;
/** 00:1a:2b:3c:4d:5e, 00-1A-2B-3C-4D-5E. */
const MAC = '(?<![\\w:-])(?:[0-9a-f]{2}[:-]){5}[0-9a-f]{2}(?![\\w:-])';
const GROUP = '[0-9a-f]{1,4}';
/** In full (2001:db8:0:0:0:0:0:1) or shortened (fe80::1c2:3ff:fe4:5, ::1), never `::` alone;
 * a time like 12:30:45 has no `::`. */
const IPV6 =
  `(?<![\\w:.])(?:(?:${GROUP}:){7}${GROUP}|(?:${GROUP}:){1,7}:(?:${GROUP}(?::${GROUP}){0,6})?` +
  `|::${GROUP}(?::${GROUP}){0,6})(?![\\w:]|\\.\\d)`;

/** A group per kind; at the same place the first of them wins. */
const PATTERN = new RegExp(
  [
    ...Object.entries(WORDS).map(([kind, words]) => `(?<${kind}>${WORD_START}(?:${words})${WORD_END})`),
    `(?<address>${IPV4}|${MAC}|${IPV6})`,
  ].join('|'),
  'giu',
);

/** Something to color in a text: from index `start` to `end` (UTF-16 units). */
export interface Highlight {
  kind: HighlightKind;
  start: number;
  end: number;
}

/** What to color in `text`, from its start; the highlights never overlap. */
export function findHighlights(text: string): Highlight[] {
  const found: Highlight[] = [];
  for (const m of text.matchAll(PATTERN)) {
    const kind = HIGHLIGHT_KINDS.find((k) => m.groups?.[k] !== undefined);
    if (kind) found.push({ kind, start: m.index, end: m.index + m[0].length });
  }
  return found;
}

/** A cell of a terminal's row (xterm's `IBufferCell`). */
export interface Cell {
  /** 2 for a wide character (CJK, most emoji), 0 for the cell after it. */
  getWidth(): number;
  getChars(): string;
  isFgDefault(): boolean;
  isBgDefault(): boolean;
  isInverse(): number;
}

/** A row of a terminal (xterm's `IBufferLine`): `cell`, when given, is filled and returned. */
export interface CellRow {
  getCell(x: number, cell?: Cell): Cell | undefined;
}

/** Cells to color: `width` columns from column `x`. */
export interface Span {
  kind: HighlightKind;
  x: number;
  width: number;
}

/** What to color on a row `cols` columns wide, in columns, which are not the text's indexes:
 * a wide character takes two columns, an emoji two UTF-16 units. Text the program colored
 * itself (`git diff`, `grep --color`, a prompt) keeps its colors. */
export function highlightRow(row: CellRow, cols: number, cell?: Cell): Span[] {
  let text = '';
  // For each UTF-16 unit of `text`: the column it starts at, the one after it, and whether the
  // program colored it.
  const starts: number[] = [];
  const ends: number[] = [];
  const colored: boolean[] = [];
  for (let x = 0; x < cols; x++) {
    const c = row.getCell(x, cell);
    if (!c) break;
    const width = c.getWidth();
    if (!width) continue;
    // An empty cell shows as a space.
    const chars = c.getChars() || ' ';
    const own = !c.isFgDefault() || !c.isBgDefault() || c.isInverse() !== 0;
    text += chars;
    for (let i = 0; i < chars.length; i++) {
      starts.push(x);
      ends.push(x + width);
      colored.push(own);
    }
  }
  return findHighlights(text)
    .filter(({ start, end }) => !colored.slice(start, end).includes(true))
    .map(({ kind, start, end }) => ({ kind, x: starts[start], width: ends[end - 1] - starts[start] }));
}

/** Whether `value` is a color the terminal's decorations take (`#rrggbb`); settings.json can be
 * edited by hand. */
export function isHighlightColor(value: unknown): value is string {
  return typeof value === 'string' && /^#[0-9a-f]{6}$/i.test(value);
}
