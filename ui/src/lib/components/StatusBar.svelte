<script lang="ts">
  import { formatNumber, i18n, t } from '../i18n.svelte';
  import { inUnit, level, sparkline, stats, unitOf, uptimeParts, usedShare, type SampleKey } from '../metrics';
  import { app } from '../state/app.svelte';
  import { INTERVAL, metrics } from '../state/metrics.svelte';
  import { sessions } from '../state/sessions.svelte';
  import type { Usage } from '../types';

  const UNITS = ['size.b', 'size.kb', 'size.mb', 'size.gb', 'size.tb'] as const;
  const TIME = { days: 'time.days', hours: 'time.hours', minutes: 'time.minutes' } as const;
  /** The graphs' size; a minute across. */
  const GRAPH_W = 60;
  const GRAPH_H = 16;

  let visible = $state(!document.hidden);

  const tab = $derived(sessions.active);
  const live = $derived(tab?.kind === 'ssh' && tab.status === 'connected' ? tab.sessionId : null);
  const reading = $derived(live != null ? metrics.readings[live] : undefined);

  // Only the shown tab's server is asked, and not while the window is minimized or in the tray.
  $effect(() => {
    metrics.watch(visible ? live : null);
  });
  $effect(() => {
    metrics.prune(new Set(sessions.tabs.flatMap((t) => (t.sessionId != null ? [t.sessionId] : []))));
  });
  $effect(() => () => metrics.watch(null));

  function size(bytes: number): string {
    const unit = unitOf(bytes);
    return `${inUnit(bytes, unit, i18n.lang)} ${t(UNITS[unit])}`;
  }

  /** `1.2 / 3.8 GB`: both in the total's unit. */
  function pair(u: Usage): string {
    const unit = unitOf(u.total);
    return `${inUnit(u.used, unit, i18n.lang)} / ${inUnit(u.total, unit, i18n.lang)} ${t(UNITS[unit])}`;
  }

  function percent(share: number): string {
    return share.toLocaleString(i18n.lang, { style: 'percent', maximumFractionDigits: 0 });
  }

  function uptime(seconds: number): string {
    return uptimeParts(seconds)
      .map((p) => t(TIME[p.unit], { count: p.count }))
      .join(' ');
  }

  const cpuTip = $derived.by(() => {
    const m = reading?.metrics;
    const lines = [];
    if (reading?.cpu != null) lines.push(t('status.cpuTip', { value: percent(reading.cpu) }));
    if (m?.load) lines.push(t('status.loadTip', { load: m.load.map((l) => formatNumber(l)).join('  ') }));
    return lines.join('\n');
  });

  const memoryTip = $derived.by(() => {
    const m = reading?.metrics;
    if (!m?.memory) return '';
    const lines = [t('status.memoryTip', { used: size(m.memory.used), total: size(m.memory.total), free: size(m.memory.free) })];
    if (m.swap) lines.push(t('status.swapTip', { used: size(m.swap.used), total: size(m.swap.total) }));
    return lines.join('\n');
  });

  const diskTip = $derived.by(() => {
    const d = reading?.metrics.disk;
    return d ? t('status.diskTip', { used: size(d.used), total: size(d.total), free: size(d.free) }) : '';
  });

  /** The minute's lowest, highest and average under a metric's own tooltip. */
  function withMinute(tip: string, key: SampleKey): string {
    const s = reading && reading.history.length > 1 ? stats(reading.history, key) : null;
    if (!s) return tip;
    const minute = t('status.minuteTip', { min: percent(s.min), max: percent(s.max), avg: percent(s.avg) });
    return [tip, minute].filter(Boolean).join('\n');
  }

  function menu(e: MouseEvent) {
    e.preventDefault();
    app.showMenu(e.clientX, e.clientY, [
      { label: t('status.hide'), icon: 'x', action: () => app.update({ statusBar: false }) },
    ]);
  }
</script>

<svelte:document onvisibilitychange={() => (visible = !document.hidden)} />

{#if tab}
  <footer class="status" oncontextmenu={menu}>
    <div class="where">
      <span class="dot {tab.status}" aria-hidden="true"></span>
      <span class="host" title={tab.kind === 'local' ? t('status.local') : tab.subtitle}>{tab.subtitle || tab.title}</span>
      {#if live != null && reading?.metrics.uptime != null}
        <span class="uptime" title={t('status.uptimeTip', { time: uptime(reading.metrics.uptime) })}>
          {t('status.uptime', { time: uptime(reading.metrics.uptime) })}
        </span>
      {/if}
    </div>

    {#if live != null}
      {#if reading === null}
        <span class="none" title={t('status.unavailableTip')}>{t('status.unavailable')}</span>
      {:else if reading}
        <div class="metrics">
          {#if reading.metrics.cpu}
            {@render meter(t('status.cpu'), reading.cpu, reading.cpu != null ? percent(reading.cpu) : '—', cpuTip, 'cpu')}
          {/if}
          {#if reading.metrics.memory}
            {@const m = reading.metrics.memory}
            {@render meter(t('status.memory'), usedShare(m), pair(m), memoryTip, 'memory')}
          {/if}
          {#if reading.metrics.disk}
            {@const d = reading.metrics.disk}
            {@render meter(t('status.disk'), usedShare(d), pair(d), diskTip, 'disk')}
          {/if}
        </div>
      {/if}
    {/if}
  </footer>
{/if}

{#snippet meter(label: string, share: number | null, value: string, tip: string, kind: SampleKey)}
  {@const graph = sparkline(reading?.history ?? [], kind, GRAPH_W, GRAPH_H, INTERVAL * 3)}
  <div class="metric {kind} {share != null ? level(share) : ''}" title={withMinute(tip, kind)}>
    <span class="label">{label}</span>
    <svg class="graph" width={GRAPH_W} height={GRAPH_H} viewBox="0 0 {GRAPH_W} {GRAPH_H}" aria-hidden="true">
      <path class="area" d={graph.area} />
      <path class="line" d={graph.line} />
    </svg>
    <span class="value">{value}</span>
  </div>
{/snippet}

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 16px;
    height: 26px;
    flex: none;
    min-width: 0;
    padding: 0 14px 0 12px;
    color: var(--text-2);
    font-size: 11.5px;
    user-select: none;
    cursor: default;
  }
  .where {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    white-space: nowrap;
  }
  .dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-3);
  }
  .dot.connected {
    background: var(--green);
  }
  .dot.connecting {
    background: var(--amber);
  }
  .host {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .uptime {
    flex: none;
    color: var(--text-3);
  }
  .none {
    color: var(--text-3);
    white-space: nowrap;
  }
  .metrics {
    display: flex;
    align-items: center;
    gap: 16px;
    flex: none;
  }
  .metric {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  .label {
    color: var(--text-3);
  }
  /* The last minute, newest on the right; the color is the current value's. */
  .graph {
    --graph: var(--text-2);
    flex: none;
    border-radius: 3px;
    background: color-mix(in srgb, var(--active) 70%, transparent);
    overflow: hidden;
  }
  .graph .line {
    fill: none;
    stroke: var(--graph);
    stroke-width: 1.25;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .graph .area {
    fill: var(--graph);
    opacity: 0.18;
  }
  .value {
    font-variant-numeric: tabular-nums;
  }
  /* Room for the widest value, so the bar does not jump as the numbers change. */
  .cpu .value {
    min-width: 2.8em;
  }
  .high .graph {
    --graph: var(--amber);
  }
  .full .graph {
    --graph: var(--red);
  }
  .full .value {
    color: var(--red);
  }
  /* A narrow window keeps the numbers and drops the bars, then the uptime. */
  @media (max-width: 760px) {
    .graph,
    .uptime {
      display: none;
    }
  }
</style>
