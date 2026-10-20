// Numbers of the status bar (src/lib/metrics.ts).
// Run with `npm test` (Node's test runner; Node 22.18+ reads TypeScript itself).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { cpuBusy, inUnit, isEmpty, level, remember, sparkline, stats, unitOf, uptimeParts, usedShare } from '../src/lib/metrics.ts';

test('processor use between two readings', () => {
  assert.equal(cpuBusy(undefined, { total: 100, idle: 50 }), null);
  assert.equal(cpuBusy({ total: 100, idle: 50 }, { total: 200, idle: 125 }), 0.25);
  assert.equal(cpuBusy({ total: 100, idle: 50 }, { total: 200, idle: 50 }), 1);
  // Nothing moved, or the server restarted in between.
  assert.equal(cpuBusy({ total: 100, idle: 50 }, { total: 100, idle: 50 }), null);
  assert.equal(cpuBusy({ total: 900, idle: 800 }, { total: 100, idle: 90 }), null);
});

test('a server without /proc', () => {
  const none = { cpu: null, memory: null, swap: null, disk: null, load: null, uptime: null };
  assert.equal(isEmpty(none), true);
  assert.equal(isEmpty({ ...none, disk: { total: 10, used: 5, free: 5 } }), false);
});

test('shares and how alarming they are', () => {
  // As df counts it: the blocks kept for root are left out.
  assert.equal(usedShare({ total: 100, used: 45, free: 45 }), 0.5);
  assert.equal(usedShare({ total: 0, used: 0, free: 0 }), 0);
  assert.equal(level(0.5), 'ok');
  assert.equal(level(0.8), 'high');
  assert.equal(level(0.95), 'full');
});

test('sizes', () => {
  const gib = 1024 ** 3;
  assert.equal(unitOf(500), 0);
  assert.equal(unitOf(3.8 * gib), 3);
  assert.equal(unitOf(1048572 * 1024), 3);
  assert.equal(inUnit(1048572 * 1024, 3, 'en'), '1.0');
  assert.equal(inUnit(1.234 * gib, 3, 'en'), '1.2');
  assert.equal(inUnit(1.234 * gib, 3, 'ru'), '1,2');
  assert.equal(inUnit(23.6 * gib, 3, 'en'), '24');
  assert.equal(inUnit(0.4 * gib, 3, 'en'), '0.4');
  assert.equal(inUnit(2 * gib, 3, 'en'), '2.0');
});

test('uptime', () => {
  assert.deepEqual(uptimeParts(59), [{ unit: 'minutes', count: 0 }]);
  assert.deepEqual(uptimeParts(3 * 3600 + 12 * 60), [
    { unit: 'hours', count: 3 },
    { unit: 'minutes', count: 12 },
  ]);
  assert.deepEqual(uptimeParts(12 * 86400 + 4 * 3600 + 59), [
    { unit: 'days', count: 12 },
    { unit: 'hours', count: 4 },
  ]);
  assert.deepEqual(uptimeParts(2 * 86400 + 30), [{ unit: 'days', count: 2 }]);
});

test('a minute of history', () => {
  const at = (s: number, cpu: number | null) => ({ at: s * 1000, cpu, memory: 0.5, disk: 0.2 });
  let h = [at(0, 0.1), at(30, 0.2)];
  h = remember(h, at(61, 0.3));
  assert.deepEqual(
    h.map((s) => s.at),
    [30_000, 61_000],
  );
  assert.deepEqual(stats([at(0, 0.1), at(2, null), at(4, 0.5)], 'cpu'), { min: 0.1, max: 0.5, avg: 0.3 });
  assert.equal(stats([at(0, null)], 'cpu'), null);
});

test('graph paths', () => {
  const at = (s: number, cpu: number | null) => ({ at: s * 1000, cpu, memory: null, disk: null });
  // 60 wide, 12 high: the newest at the right edge, 0 % at the bottom (y 11), 100 % at the top (y 1).
  const { line, area } = sparkline([at(30, 0), at(60, 1)], 'cpu', 60, 12, 40_000);
  assert.equal(line, 'M30 11 L60 1');
  assert.equal(area, 'M30 12 L30 11 L60 1 L60 12 Z');
  // A gap breaks the line; a lone reading is a short stroke.
  assert.equal(sparkline([at(0, 0), at(2, 0), at(50, 1)], 'cpu', 60, 12, 5000).line, 'M10 11 L12 11 M58 1 L60 1');
  assert.equal(sparkline([at(0, 0.5), at(2, null), at(4, 0.5)], 'cpu', 60, 12, 5000).line, 'M54 6 L56 6 M58 6 L60 6');
  assert.deepEqual(sparkline([], 'cpu', 60, 12, 5000), { line: '', area: '' });
});
