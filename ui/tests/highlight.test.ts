// What keyword highlighting colors in a terminal's output (src/lib/highlight.ts).
// Run with `npm test` (Node's test runner; Node 22.18+ reads TypeScript itself).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { findHighlights, highlightRow, isHighlightColor, type Cell, type CellRow } from '../src/lib/highlight.ts';

/** The highlights of `text` as [kind, the text colored]. */
function found(text: string): [string, string][] {
  return findHighlights(text).map(({ kind, start, end }) => [kind, text.slice(start, end)]);
}

type Colored = { from: number; to: number; by: 'fg' | 'bg' | 'inverse' };

function cell(chars: string, width: number, by?: Colored['by']): Cell {
  return {
    getWidth: () => width,
    getChars: () => chars,
    isFgDefault: () => by !== 'fg',
    isBgDefault: () => by !== 'bg',
    isInverse: () => (by === 'inverse' ? 1 : 0),
  };
}

/** A terminal row `cols` wide showing `text`, where the characters in `wide` take two columns, as
 * in xterm: the character, then an empty cell of width 0; the program colored the characters
 * `colored` says. Columns past the text are empty cells. */
function row(text: string, cols: number, { wide = '', colored }: { wide?: string; colored?: Colored } = {}): CellRow {
  const cells: Cell[] = [];
  [...text].forEach((ch, i) => {
    const width = wide.includes(ch) ? 2 : 1;
    const by = colored && i >= colored.from && i < colored.to ? colored.by : undefined;
    cells.push(cell(ch, width, by));
    if (width === 2) cells.push(cell('', 0, by));
  });
  while (cells.length < cols) cells.push(cell('', 1));
  return { getCell: (x) => cells[x] };
}

test('the log of the example', () => {
  const stamp = '[00-00-0000 00:00:00.000]';
  assert.deepEqual(found(stamp), []);
  assert.deepEqual(found(`${stamp} [INFO] something`), [['info', 'INFO']]);
  assert.deepEqual(found(`${stamp} [DEBUG] 1.1.1.1 ok`), [
    ['debug', 'DEBUG'],
    ['address', '1.1.1.1'],
    ['ok', 'ok'],
  ]);
  assert.deepEqual(found(`${stamp} [ERROR] ERROR`), [
    ['error', 'ERROR'],
    ['error', 'ERROR'],
  ]);
  assert.deepEqual(found(`${stamp} [WARN] 00:00:00:00:00:00`), [
    ['warning', 'WARN'],
    ['address', '00:00:00:00:00:00'],
  ]);
});

test('keywords are whole words in any case', () => {
  assert.deepEqual(found('Error: connect failed, Permission denied'), [
    ['error', 'Error'],
    ['error', 'failed'],
    ['error', 'denied'],
  ]);
  assert.deepEqual(found('warning: deprecated; Success; 3 passed, done.'), [
    ['warning', 'warning'],
    ['warning', 'deprecated'],
    ['ok', 'Success'],
    ['ok', 'passed'],
    ['ok', 'done'],
  ]);
  assert.deepEqual(found('<notice> trace'), [
    ['info', 'notice'],
    ['debug', 'trace'],
  ]);
  // Parts of other words.
  assert.deepEqual(found('errorCount error_code error2 terror failover okay2 Traceback information_schema'), []);
  assert.deepEqual(found('errorы ошибка: error'), [['error', 'error']]);
  assert.deepEqual(found(''), []);
});

test('IPv4 addresses', () => {
  assert.deepEqual(found('from 192.168.0.1:22 to 10.0.0.0/8.'), [
    ['address', '192.168.0.1'],
    ['address', '10.0.0.0'],
  ]);
  assert.deepEqual(found('255.255.255.255 0.0.0.0'), [
    ['address', '255.255.255.255'],
    ['address', '0.0.0.0'],
  ]);
  // Versions, numbers out of range, three parts.
  assert.deepEqual(found('1.2.3.4.5 v1.2.3.4 256.1.1.1 1.2.3 3.14'), []);
});

test('MAC addresses', () => {
  assert.deepEqual(found('link/ether 52:54:00:12:34:56 brd ff:ff:ff:ff:ff:ff'), [
    ['address', '52:54:00:12:34:56'],
    ['address', 'ff:ff:ff:ff:ff:ff'],
  ]);
  assert.deepEqual(found('Physical Address. . . : 00-1A-2B-3C-4D-5E'), [['address', '00-1A-2B-3C-4D-5E']]);
  // Times and a longer run of pairs.
  assert.deepEqual(found('12:30:45 00:00:00.000 1:2:3 aa:bb:cc:dd:ee:ff:00'), []);
});

test('IPv6 addresses', () => {
  assert.deepEqual(found('inet6 fe80::1c2:3ff:fe4:5/64 scope link'), [['address', 'fe80::1c2:3ff:fe4:5']]);
  assert.deepEqual(found('[::1]:631 2001:db8:0:0:0:0:0:1 2001:db8::'), [
    ['address', '::1'],
    ['address', '2001:db8:0:0:0:0:0:1'],
    ['address', '2001:db8::'],
  ]);
  // Not addresses: code and `::` alone.
  assert.deepEqual(found('std::vector Foo::Bar x :: y [::]:22'), []);
});

test('rows are colored by columns, wide characters take two', () => {
  assert.deepEqual(highlightRow(row('[ERROR] ok', 20), 20), [
    { kind: 'error', x: 1, width: 5 },
    { kind: 'ok', x: 8, width: 2 },
  ]);
  // 日 and 本 take columns 0–3.
  assert.deepEqual(highlightRow(row('日本 error', 20, { wide: '日本' }), 20), [{ kind: 'error', x: 5, width: 5 }]);
  // An emoji: two UTF-16 units in two columns.
  assert.deepEqual(highlightRow(row('😀 ok 10.0.0.1', 20, { wide: '😀' }), 20), [
    { kind: 'ok', x: 3, width: 2 },
    { kind: 'address', x: 6, width: 8 },
  ]);
  // Empty cells between words are spaces; nothing is read past the row's width.
  const letters: Record<number, string> = { 0: 'o', 1: 'k', 9: 'i', 10: 'n', 11: 'f', 12: 'o' };
  const sparse: CellRow = { getCell: (x) => cell(letters[x] ?? '', 1) };
  assert.deepEqual(highlightRow(sparse, 13), [
    { kind: 'ok', x: 0, width: 2 },
    { kind: 'info', x: 9, width: 4 },
  ]);
  assert.deepEqual(highlightRow(sparse, 12), [{ kind: 'ok', x: 0, width: 2 }]);
});

test('text the program colored keeps its colors', () => {
  // `\e[32mok\e[0m error`, a red background, reverse video.
  assert.deepEqual(highlightRow(row('ok error', 20, { colored: { from: 0, to: 2, by: 'fg' } }), 20), [
    { kind: 'error', x: 3, width: 5 },
  ]);
  assert.deepEqual(highlightRow(row('ERROR info', 20, { colored: { from: 0, to: 5, by: 'bg' } }), 20), [
    { kind: 'info', x: 6, width: 4 },
  ]);
  assert.deepEqual(highlightRow(row('warn debug', 20, { colored: { from: 0, to: 4, by: 'inverse' } }), 20), [
    { kind: 'debug', x: 5, width: 5 },
  ]);
  // Partly colored, and a word that only looks like a keyword without the colored part.
  assert.deepEqual(highlightRow(row('error', 20, { colored: { from: 0, to: 2, by: 'fg' } }), 20), []);
  assert.deepEqual(highlightRow(row('fooerror', 20, { colored: { from: 0, to: 3, by: 'fg' } }), 20), []);
});

test('colors decorations take', () => {
  assert.equal(isHighlightColor('#f35e62'), true);
  assert.equal(isHighlightColor('#FFCA00'), true);
  for (const bad of ['#fff', 'red', 'f35e62', '#f35e6200', '', undefined, null, 0]) {
    assert.equal(isHighlightColor(bad), false, String(bad));
  }
});
