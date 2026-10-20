// Numbers of the status bar (state/metrics.svelte.ts reads them from the server): no Svelte
// and no messages here, so the tests run it in Node.

import type { CpuTimes, ServerMetrics, Usage } from './types';

/** The share of the time between two readings the processor was busy, 0..1; `null` when the
 * counters did not move (or went back: the server restarted between the readings). */
export function cpuBusy(prev: CpuTimes | undefined, next: CpuTimes): number | null {
  if (!prev || next.total <= prev.total || next.idle < prev.idle) return null;
  const busy = 1 - (next.idle - prev.idle) / (next.total - prev.total);
  return Math.min(1, Math.max(0, busy));
}

/** Nothing was read: not a Linux server. */
export function isEmpty(m: ServerMetrics): boolean {
  return !m.cpu && !m.memory && !m.disk;
}

/** The share in use, 0..1, as `df` counts it for a disk: of what is used plus what is left for
 * use (blocks kept for root are neither). */
export function usedShare(u: Usage): number {
  const of = u.used + u.free;
  return of > 0 ? Math.min(1, u.used / of) : 0;
}

/** How alarming a share is, for the bar's color. */
export function level(share: number): 'ok' | 'high' | 'full' {
  return share >= 0.9 ? 'full' : share >= 0.75 ? 'high' : 'ok';
}

/** Index into B, KB, MB, GB, TB: the largest unit `bytes` has at least one of, or almost (a
 * swap of 1023.9 MB is 1.0 GB). */
export function unitOf(bytes: number): number {
  let unit = 0;
  while (bytes >= 1000 * 1024 ** unit && unit < 4) unit++;
  return unit;
}

/** `bytes` in `unit`, with one decimal below 10 (`1.2`, `12`, `850`). */
export function inUnit(bytes: number, unit: number, lang: string): string {
  const value = bytes / 1024 ** unit;
  const digits = unit > 0 && value < 10 ? 1 : 0;
  return value.toLocaleString(lang, { maximumFractionDigits: digits, minimumFractionDigits: digits });
}

/** Whole days, hours and minutes of `seconds`: the two largest that are not zero (minutes
 * alone below an hour). */
export function uptimeParts(seconds: number): { unit: 'days' | 'hours' | 'minutes'; count: number }[] {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return hours > 0 ? [{ unit: 'days', count: days }, { unit: 'hours', count: hours }] : [{ unit: 'days', count: days }];
  if (hours > 0) return [{ unit: 'hours', count: hours }, ...(minutes > 0 ? [{ unit: 'minutes' as const, count: minutes }] : [])];
  return [{ unit: 'minutes', count: minutes }];
}

/** How far back the status bar's graphs go. */
export const HISTORY_MS = 60_000;

/** One reading for the graphs: shares 0..1, `null` where the server gave none. */
export interface Sample {
  at: number;
  cpu: number | null;
  memory: number | null;
  disk: number | null;
}

export type SampleKey = 'cpu' | 'memory' | 'disk';

/** `history` with `sample` added and what is older than the graphs show dropped. */
export function remember(history: Sample[], sample: Sample): Sample[] {
  return [...history.filter((s) => sample.at - s.at <= HISTORY_MS), sample];
}

/** Lowest, highest and average share over the history; `null` with no values. */
export function stats(history: Sample[], key: SampleKey): { min: number; max: number; avg: number } | null {
  const values = history.flatMap((s) => (s[key] != null ? [s[key]] : []));
  if (!values.length) return null;
  return {
    min: Math.min(...values),
    max: Math.max(...values),
    avg: values.reduce((a, b) => a + b, 0) / values.length,
  };
}

/**
 * SVG paths of a graph `width` × `height` of the last minute up to the newest sample (at the
 * right edge): the line, and the area under it. A gap longer than `gap` ms (the tab was not
 * shown, the window was hidden) breaks the line instead of joining over it.
 */
export function sparkline(
  history: Sample[],
  key: SampleKey,
  width: number,
  height: number,
  gap: number,
): { line: string; area: string } {
  const end = history.at(-1)?.at ?? 0;
  const runs: [number, number][][] = [];
  let run: [number, number][] = [];
  let last = -Infinity;
  for (const s of history) {
    const value = s[key];
    if (value == null || s.at - last > gap) {
      if (run.length) runs.push(run);
      run = [];
    }
    last = s.at;
    if (value == null) continue;
    const x = width * (1 - (end - s.at) / HISTORY_MS);
    // Half the stroke stays inside at 0 and 100 %.
    const y = 1 + (height - 2) * (1 - value);
    run.push([round(x), round(y)]);
  }
  if (run.length) runs.push(run);
  let line = '';
  let area = '';
  for (const points of runs) {
    // A lone reading still shows: a short level stroke ending at it.
    if (points.length === 1) points.unshift([round(Math.max(0, points[0][0] - 2)), points[0][1]]);
    const path = points.map(([x, y], i) => `${i ? 'L' : 'M'}${x} ${y}`).join(' ');
    line += `${line ? ' ' : ''}${path}`;
    const [x0] = points[0];
    const [xn] = points[points.length - 1];
    area += `${area ? ' ' : ''}M${x0} ${height} ${path.replace(/^M/, 'L')} L${xn} ${height} Z`;
  }
  return { line, area };
}

function round(n: number): number {
  return Math.round(n * 10) / 10;
}
