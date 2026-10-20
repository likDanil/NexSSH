// The load of the server the active tab is on, for the status bar: read every few seconds
// with a short command on the session's connection (`session_metrics`), only while the bar
// shows a connected SSH tab and the window can be seen.

import { api } from '../api';
import { cpuBusy, isEmpty, remember, usedShare, type Sample } from '../metrics';
import type { CpuTimes, ServerMetrics } from '../types';

/** Thirty readings make the status bar's one-minute graphs. */
export const INTERVAL = 2000;
/** The processor's use needs a second reading: it comes sooner after the first. */
const FIRST_CPU = 1000;
/** After an error (the command refused, timed out). */
const RETRY = 15000;
/** A server without `/proc`: asked again now and then (it would not change, but costs little). */
const UNSUPPORTED = 60000;
/** An older reading is not used for the processor's use: it would be the average since then
 * (back on a tab after a while). */
const STALE = 10000;

export interface Reading {
  metrics: ServerMetrics;
  /** The processor's use between the last two readings, 0..1. */
  cpu: number | null;
  /** The last minute's readings, oldest first, for the graphs. */
  history: Sample[];
}

class MetricsState {
  /** By session id: the last reading, `null` when the server gives none (not Linux). Kept for
   * the other tabs, so going back to one shows its numbers at once. */
  readings = $state<Record<number, Reading | null>>({});
  #cpu = new Map<number, { times: CpuTimes; at: number }>();
  #session: number | null = null;
  #round = 0;
  #timer: ReturnType<typeof setTimeout> | undefined;

  /** Reads the server of session `id` now and every few seconds after; `null` stops. */
  watch(id: number | null) {
    if (id === this.#session) return;
    this.#session = id;
    clearTimeout(this.#timer);
    const round = ++this.#round;
    if (id != null) void this.#poll(id, round);
  }

  /** Drops what is kept for sessions that are gone. */
  prune(live: Set<number>) {
    for (const key of Object.keys(this.readings)) {
      const id = Number(key);
      if (!live.has(id)) {
        delete this.readings[id];
        this.#cpu.delete(id);
      }
    }
  }

  async #poll(id: number, round: number) {
    let delay = INTERVAL;
    try {
      const metrics = await api.metrics(id);
      if (isEmpty(metrics)) {
        this.readings[id] = null;
        delay = UNSUPPORTED;
      } else {
        const now = Date.now();
        const last = this.#cpu.get(id);
        const prev = last && now - last.at < STALE ? last.times : undefined;
        let cpu: number | null = null;
        if (metrics.cpu) {
          cpu = cpuBusy(prev, metrics.cpu);
          this.#cpu.set(id, { times: metrics.cpu, at: now });
          if (!prev) delay = FIRST_CPU;
        }
        const sample: Sample = {
          at: now,
          cpu,
          memory: metrics.memory ? usedShare(metrics.memory) : null,
          disk: metrics.disk ? usedShare(metrics.disk) : null,
        };
        this.readings[id] = {
          metrics,
          cpu: cpu ?? this.readings[id]?.cpu ?? null,
          history: remember(this.readings[id]?.history ?? [], sample),
        };
      }
    } catch {
      delay = RETRY;
    }
    if (round === this.#round) this.#timer = setTimeout(() => void this.#poll(id, round), delay);
  }
}

export const metrics = new MetricsState();
