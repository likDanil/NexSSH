// What `terminal_read` gives an agent (src/lib/screen.ts), on rows like a terminal's.
// Run with `npm test` (Node's test runner; Node 22.18+ reads TypeScript itself).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readRows, type Row, type Rows } from '../src/lib/screen.ts';

function row(cells: string, isWrapped: boolean, cols: number): Row {
  const full = cells.padEnd(cols);
  return { isWrapped, translateToString: (trimRight = false) => (trimRight ? full.trimEnd() : full) };
}

/** A terminal `cols` wide that printed `lines` (wrapping the long ones), then empty rows down to
 * `height` rows. */
function terminal(lines: string[], cols: number, height = 0): Rows {
  const rows: Row[] = [];
  for (const line of lines) {
    let rest = line;
    let wrapped = false;
    do {
      rows.push(row(rest.slice(0, cols), wrapped, cols));
      rest = rest.slice(cols);
      wrapped = true;
    } while (rest);
  }
  while (rows.length < height) rows.push(row('', false, cols));
  return { length: rows.length, getLine: (y) => rows[y] };
}

const TIME = ['$ time sleep 1', '', 'real\t0m1.003s', 'user\t0m0.001s', 'sys\t0m0.002s', '$'];

test('the last lines are counted back from the end of the output, not from the empty bottom', () => {
  // `clear`, then `time sleep 1`: six lines at the top of a 50-row screen.
  const rows = terminal(TIME, 80, 50);
  assert.deepEqual(readRows(rows, 0, 30), { text: TIME.join('\n'), from: 1, last: 6 });
  assert.deepEqual(readRows(rows, 0, 3), { text: TIME.slice(3).join('\n'), from: 4, last: 6 });
  // Empty lines inside the output count.
  assert.equal(readRows(rows, 0, 5).text, TIME.slice(1).join('\n'));
});

test('a wrapped line counts once and comes back whole, spaces at the wrap included', () => {
  const long = 'a'.repeat(25);
  const rows = terminal(['short', long, 'abcdefghi jklmn', 'end'], 10, 24);
  assert.deepEqual(readRows(rows, 0, 3), { text: `${long}\nabcdefghi jklmn\nend`, from: 2, last: 7 });
  assert.equal(readRows(rows, 0, 2).text, 'abcdefghi jklmn\nend');
});

test('more lines than there are gives all of them', () => {
  const rows = terminal(['one', 'two'], 20, 10);
  assert.deepEqual(readRows(rows, 0, 500), { text: 'one\ntwo', from: 1, last: 2 });
});

test('the screen runs from its first row to the end of the output', () => {
  const scrollback = Array.from({ length: 40 }, (_, i) => `old ${i}`);
  // 40 rows of scrollback above a 24-row screen holding the six lines.
  const rows = terminal([...scrollback, ...TIME], 80, 64);
  assert.deepEqual(readRows(rows, 40, null), { text: TIME.join('\n'), from: 41, last: 46 });
  // With lines, the scrollback too.
  assert.equal(readRows(rows, 40, 8).text, ['old 38', 'old 39', ...TIME].join('\n'));
  // An empty screen after `clear`: nothing on it, the output above it.
  const cleared = terminal(scrollback, 80, 64);
  assert.deepEqual(readRows(cleared, 40, null), { text: '', from: 41, last: 40 });
  assert.equal(readRows(cleared, 40, 2).text, 'old 38\nold 39');
});

test('a terminal that printed nothing gives nothing', () => {
  const rows = terminal([], 80, 24);
  assert.deepEqual(readRows(rows, 0, 30), { text: '', from: 1, last: 0 });
  assert.deepEqual(readRows(rows, 0, null), { text: '', from: 1, last: 0 });
});
