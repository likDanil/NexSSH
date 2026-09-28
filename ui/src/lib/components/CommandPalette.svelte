<script lang="ts">
  import { onMount } from 'svelte';
  import {
    addServer,
    closeTab,
    connect,
    editServer,
    importSshConfig,
    keys,
    looksLikeDestination,
    openForwards,
    openSettings,
    presetFromDestination,
    showHome,
    toggleFullscreen,
    toggleSidebar,
  } from '../actions';
  import { fuzzyBest, fuzzyScore } from '../fuzzy';
  import { isMac } from '../platform';
  import { app } from '../state/app.svelte';
  import { destination, servers } from '../state/servers.svelte';
  import { sessions } from '../state/sessions.svelte';
  import { THEMES } from '../themes';
  import Icon from './Icon.svelte';

  interface Item {
    section: string;
    label: string;
    detail?: string;
    icon: string;
    hint?: string;
    dot?: string;
    score: number;
    /** Keeps the palette open (e.g. switching to connect mode). */
    stay?: boolean;
    run: (newTab: boolean) => void;
  }

  let input: HTMLInputElement;
  let list = $state<HTMLDivElement>();
  let selected = $state(0);

  const mode = $derived(app.palette.mode);
  const query = $derived(app.palette.query);

  interface Action {
    label: string;
    icon: string;
    hint?: string;
    keywords?: string;
    stay?: boolean;
    run: () => void;
  }

  function actionList(): Action[] {
    const out: Action[] = [
      {
        label: 'New session…',
        icon: 'zap',
        hint: keys.newSession(),
        keywords: 'quick connect ssh open',
        stay: true,
        run: () => app.openPalette('connect'),
      },
      { label: 'Add server…', icon: 'plus', keywords: 'new create host', run: () => addServer() },
      { label: 'Import ~/.ssh/config', icon: 'import', keywords: 'openssh hosts', run: importSshConfig },
      { label: 'Show servers', icon: 'server', keywords: 'home list', run: showHome },
      { label: 'Settings', icon: 'settings', hint: keys.settings(), keywords: 'preferences options font', run: openSettings },
      { label: 'Toggle sidebar', icon: 'sidebar', hint: keys.sidebar(), run: toggleSidebar },
      { label: 'Toggle full screen', icon: 'fullscreen', hint: keys.fullscreen(), keywords: 'focus', run: toggleFullscreen },
      ...THEMES.map((t) => ({
        label: `Theme: ${t.label}`,
        icon: 'theme',
        keywords: 'appearance color dark light',
        run: () => app.update({ theme: t.id }),
      })),
    ];
    const tab = sessions.active;
    if (tab) {
      const server = tab.serverId ? servers.byId.get(tab.serverId) : undefined;
      out.unshift(
        tab.status === 'disconnected'
          ? { label: 'Reconnect', icon: 'refresh', hint: keys.reconnect(), run: () => sessions.reconnect(tab) }
          : { label: 'Disconnect', icon: 'power', keywords: 'close connection', run: () => sessions.disconnect(tab) },
      );
      if (tab.status === 'connected') {
        out.push({ label: 'Port forwarding…', icon: 'forward', keywords: 'tunnel socks proxy -L -R -D', run: openForwards });
      }
      out.push(
        { label: 'Duplicate session', icon: 'duplicate', keywords: 'clone new tab', run: () => sessions.duplicate(tab) },
        { label: 'Clear terminal', icon: 'eraser', keywords: 'reset screen', run: () => sessions.clear(tab) },
        { label: 'Close tab', icon: 'x', hint: keys.closeTab(), run: () => closeTab(tab) },
      );
      if (server) out.push({ label: 'Edit connection settings…', icon: 'edit', run: () => editServer(server) });
    }
    return out;
  }

  const items: Item[] = $derived.by(() => {
    const q = query.trim();
    const out: Item[] = [];

    if (q && (mode === 'connect' || looksLikeDestination(q))) {
      out.push({
        section: 'Connect',
        label: `Connect to ${q}`,
        icon: 'zap',
        hint: '↵',
        score: 3e6,
        run: () => sessions.openDestination(q),
      });
      out.push({
        section: 'Connect',
        label: `Save ${q} as a server…`,
        icon: 'plus',
        score: 3e6 - 1,
        run: () => addServer(presetFromDestination(q)),
      });
    }

    const recentRank = new Map(servers.recent.map((s, i) => [s.id, i]));
    const serverItems: Item[] = [];
    for (const s of servers.data.servers) {
      const score = q
        ? fuzzyBest(q, s.name, s.host, destination(s), s.group, s.alias)
        : 1000 - (recentRank.get(s.id) ?? 100);
      if (score <= 0) continue;
      const status = sessions.serverStatus(s.id);
      serverItems.push({
        section: 'Servers',
        label: s.name,
        detail: destination(s),
        icon: 'server',
        dot: status === 'idle' ? '' : status,
        score: 2e6 + score,
        run: (newTab) => connect(s, newTab),
      });
    }
    serverItems.sort((a, b) => b.score - a.score);
    out.push(...serverItems.slice(0, q ? 8 : mode === 'connect' ? 12 : 5));

    if (mode === 'commands') {
      const tabs: Item[] = [];
      for (const t of sessions.tabs) {
        const score = q ? fuzzyBest(q, t.title, t.subtitle) : 1;
        if (score <= 0 || t.key === sessions.activeKey) continue;
        tabs.push({
          section: 'Open tabs',
          label: t.title,
          detail: t.subtitle,
          icon: 'terminal',
          dot: t.status,
          score: 1e6 + score,
          run: () => {
            sessions.activeKey = t.key;
          },
        });
      }
      tabs.sort((a, b) => b.score - a.score);
      out.push(...tabs.slice(0, 5));

      const acts: Item[] = [];
      for (const a of actionList()) {
        const score = q ? Math.max(fuzzyScore(q, a.label), fuzzyScore(q, a.keywords ?? '') * 0.6) : 1;
        if (score <= 0) continue;
        acts.push({ section: 'Actions', label: a.label, icon: a.icon, hint: a.hint, stay: a.stay, score, run: a.run });
      }
      if (q) acts.sort((a, b) => b.score - a.score);
      out.push(...acts);
    }
    return out;
  });

  $effect(() => {
    void items;
    selected = 0;
  });

  $effect(() => {
    const el = list?.querySelector<HTMLElement>(`[data-index="${selected}"]`);
    el?.scrollIntoView({ block: 'nearest' });
  });

  onMount(() => {
    input.focus();
    input.select();
  });

  function close() {
    app.closePalette();
    requestAnimationFrame(() => sessions.focusActive());
  }

  function run(item: Item, newTab = false) {
    if (item.stay) {
      item.run(newTab);
      requestAnimationFrame(() => input.focus());
      return;
    }
    app.closePalette();
    item.run(newTab);
    requestAnimationFrame(() => sessions.focusActive());
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (mode === 'connect' && !query) app.palette.mode = 'commands';
      else close();
    } else if (e.key === 'ArrowDown' || (e.key === 'Tab' && !e.shiftKey)) {
      e.preventDefault();
      if (items.length) selected = (selected + 1) % items.length;
    } else if (e.key === 'ArrowUp' || (e.key === 'Tab' && e.shiftKey)) {
      e.preventDefault();
      if (items.length) selected = (selected - 1 + items.length) % items.length;
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const item = items[selected];
      if (item) run(item, isMac() ? e.metaKey : e.ctrlKey);
    } else if (e.key === 'Backspace' && !query && mode === 'connect') {
      app.palette.mode = 'commands';
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="palette" role="dialog" aria-label="Command palette">
    <div class="searchbar">
      {#if mode === 'connect'}
        <span class="chip"><Icon name="zap" size={13} /> Connect</span>
      {:else}
        <Icon name="search" size={16} />
      {/if}
      <input
        bind:this={input}
        bind:value={app.palette.query}
        placeholder={mode === 'connect' ? 'user@host:port or a server name' : 'Search servers and commands…'}
        spellcheck="false"
        autocomplete="off"
        {onkeydown}
      />
    </div>

    <div class="results" bind:this={list}>
      {#each items as item, i (item.section + item.label + i)}
        {#if i === 0 || items[i - 1].section !== item.section}
          <div class="section">{item.section}</div>
        {/if}
        <button
          class="item"
          class:selected={i === selected}
          data-index={i}
          onmousemove={() => (selected = i)}
          onclick={(e) => run(item, isMac() ? e.metaKey : e.ctrlKey)}
        >
          <span class="icon">
            {#if item.dot !== undefined}
              <span class="dot {item.dot}"></span>
            {:else}
              <Icon name={item.icon} size={15} />
            {/if}
          </span>
          <span class="title">{item.label}</span>
          {#if item.detail}<span class="detail">{item.detail}</span>{/if}
          {#if item.hint}<span class="hint">{item.hint}</span>{/if}
        </button>
      {:else}
        <div class="none">
          {#if mode === 'connect'}Type a destination such as <code>deploy@10.0.0.5:2222</code>{:else}No matches{/if}
        </div>
      {/each}
    </div>

    <div class="foot">
      <span><kbd>↑</kbd><kbd>↓</kbd> navigate</span>
      <span><kbd>↵</kbd> open</span>
      <span><kbd>{isMac() ? '⌘↵' : 'Ctrl+↵'}</kbd> new tab</span>
      <span><kbd>esc</kbd> close</span>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 55;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 12vh 16px 16px;
    background: var(--overlay);
    animation: fade 0.1s var(--ease);
  }
  .palette {
    width: 580px;
    max-width: 100%;
    max-height: 62vh;
    display: flex;
    flex-direction: column;
    border-radius: 14px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    overflow: hidden;
    animation: rise 0.14s var(--ease);
  }
  .searchbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 16px;
    height: 50px;
    border-bottom: 1px solid var(--border-soft);
    color: var(--text-2);
  }
  .searchbar input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    background: transparent;
    font-size: 14.5px;
    color: var(--text);
  }
  .searchbar input::placeholder {
    color: var(--text-3);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 24px;
    padding: 0 8px;
    border-radius: 6px;
    background: var(--hover);
    color: var(--text);
    font-size: 12px;
    font-weight: 500;
  }
  .results {
    flex: 1;
    overflow-y: auto;
    padding: 6px;
  }
  .section {
    padding: 8px 10px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    color: var(--text-3);
    text-transform: uppercase;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 36px;
    padding: 0 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    text-align: left;
  }
  .item.selected {
    background: var(--hover);
  }
  .icon {
    display: grid;
    place-items: center;
    width: 18px;
    color: var(--text-2);
  }
  .title {
    flex: none;
    max-width: 55%;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .detail {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .hint {
    margin-left: auto;
    color: var(--text-3);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .none {
    padding: 22px;
    text-align: center;
    color: var(--text-2);
  }
  .none code {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text);
  }
  .foot {
    display: flex;
    gap: 14px;
    padding: 8px 14px;
    border-top: 1px solid var(--border-soft);
    background: var(--frame);
    color: var(--text-3);
    font-size: 11.5px;
  }
  .foot kbd {
    margin-right: 3px;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(-4px) scale(0.99);
    }
  }
</style>
