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
    openLocalTerminal,
    openSettings,
    presetFromDestination,
    showHome,
    toggleFiles,
    toggleFullscreen,
    toggleSidebar,
  } from '../actions';
  import { fuzzyBest, fuzzyScore } from '../fuzzy';
  import { i18n, LANGUAGES, t, tEn, type LanguageSetting, type MessageKey, type Params } from '../i18n.svelte';
  import { isMac } from '../platform';
  import { app } from '../state/app.svelte';
  import { destination, servers } from '../state/servers.svelte';
  import { sessions } from '../state/sessions.svelte';
  import { shellName, shells } from '../state/shells.svelte';
  import { updates } from '../state/updates.svelte';
  import { THEMES, themeLabel } from '../themes';
  import Icon from './Icon.svelte';
  import Rich from './Rich.svelte';

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

  /** An action labelled by `key`; in a translated interface it is also found by its English words. */
  function action(
    key: MessageKey,
    rest: Omit<Action, 'label' | 'keywords'>,
    keywords?: MessageKey,
    params?: Params,
  ): Action {
    const words = keywords ? [t(keywords)] : [];
    if (i18n.lang !== 'en') words.push(tEn(key, params), keywords ? tEn(keywords) : '');
    return { label: t(key, params), keywords: words.join(' '), ...rest };
  }

  function actionList(): Action[] {
    const languages: { id: LanguageSetting; name: string; english: string }[] = [
      { id: 'system', name: t('settings.languageSystem'), english: tEn('settings.languageSystem') },
      ...LANGUAGES,
    ];
    const out: Action[] = [
      action(
        'palette.newSession',
        { icon: 'zap', hint: keys.newSession(), stay: true, run: () => app.openPalette('connect') },
        'palette.newSession.keywords',
      ),
      action(
        'palette.localTerminal',
        { icon: 'terminal', hint: keys.localTerminal(), run: () => openLocalTerminal() },
        'palette.localTerminal.keywords',
      ),
      // Every shell when there is a choice (PowerShell, cmd, WSL…).
      ...(shells.list.length > 1
        ? shells.list.map((shell) =>
            action('palette.localShell', { icon: 'terminal', run: () => openLocalTerminal(shell) }, undefined, {
              name: shellName(shell),
            }),
          )
        : []),
      action('palette.addServer', { icon: 'plus', run: () => addServer() }, 'palette.addServer.keywords'),
      action('palette.import', { icon: 'import', run: importSshConfig }, 'palette.import.keywords'),
      action('palette.showServers', { icon: 'server', run: showHome }, 'palette.showServers.keywords'),
      action(
        'palette.settings',
        { icon: 'settings', hint: keys.settings(), run: () => openSettings() },
        'palette.settings.keywords',
      ),
      action('palette.toggleSidebar', { icon: 'sidebar', hint: keys.sidebar(), run: toggleSidebar }),
      action('palette.zoomIn', { icon: 'plus', hint: keys.zoomIn(), run: () => app.zoom(1) }, 'palette.zoom.keywords'),
      action('palette.zoomOut', { icon: 'minus', hint: keys.zoomOut(), run: () => app.zoom(-1) }, 'palette.zoom.keywords'),
      action('palette.zoomReset', { icon: 'text', hint: keys.zoomReset(), run: () => app.zoom(0) }, 'palette.zoom.keywords'),
      action(
        'palette.toggleFullscreen',
        { icon: 'fullscreen', hint: keys.fullscreen(), run: toggleFullscreen },
        'palette.toggleFullscreen.keywords',
      ),
      ...THEMES.map((th) => ({
        label: t('palette.theme', { name: themeLabel(th.id) }),
        icon: 'theme',
        keywords: `${t('palette.theme.keywords')} ${i18n.lang !== 'en' ? tEn('palette.theme', { name: th.id }) : ''}`,
        run: () => app.update({ theme: th.id }),
      })),
      ...languages.map((l) => ({
        label: t('palette.language', { name: l.name }),
        icon: 'globe',
        keywords: `${t('palette.language.keywords')} ${tEn('palette.language', { name: l.english })}`,
        run: () => app.update({ language: l.id }),
      })),
    ];
    if (updates.supported) {
      const version = updates.info?.version ?? '';
      if (updates.status === 'available') {
        const run = () => void updates.download();
        out.unshift(action('update.downloadVersion', { icon: 'update', run }, 'palette.update.keywords', { version }));
      } else if (updates.status === 'ready') {
        const run = () => void updates.install();
        out.unshift(action('update.installVersion', { icon: 'update', run }, 'palette.update.keywords', { version }));
      }
      out.push(action('update.check', { icon: 'update', run: () => void updates.check(true) }, 'palette.update.keywords'));
    }
    const tab = sessions.active;
    if (tab?.kind === 'local') {
      out.unshift(
        tab.status === 'disconnected'
          ? action('session.restart', { icon: 'refresh', hint: keys.reconnect(), run: () => sessions.reconnect(tab) }, 'palette.restart.keywords')
          : action('session.stop', { icon: 'power', run: () => sessions.disconnect(tab) }, 'palette.stop.keywords'),
      );
      out.push(
        action('session.duplicate', { icon: 'duplicate', run: () => sessions.duplicate(tab) }, 'palette.duplicate.keywords'),
        action('session.clear', { icon: 'eraser', run: () => sessions.clear(tab) }, 'palette.clear.keywords'),
        action('session.closeTab', { icon: 'x', hint: keys.closeTab(), run: () => closeTab(tab) }),
      );
    } else if (tab) {
      const server = tab.serverId ? servers.byId.get(tab.serverId) : undefined;
      out.unshift(
        tab.status === 'disconnected'
          ? action('session.reconnect', { icon: 'refresh', hint: keys.reconnect(), run: () => sessions.reconnect(tab) })
          : action('session.disconnect', { icon: 'power', run: () => sessions.disconnect(tab) }, 'palette.disconnect.keywords'),
      );
      if (tab.status === 'connected') {
        out.push(action('session.forwarding', { icon: 'forward', run: openForwards }, 'palette.forwarding.keywords'));
      }
      out.push(
        action('session.files', { icon: 'folder', hint: keys.files(), run: toggleFiles }, 'palette.files.keywords'),
        action('session.duplicate', { icon: 'duplicate', run: () => sessions.duplicate(tab) }, 'palette.duplicate.keywords'),
        action('session.clear', { icon: 'eraser', run: () => sessions.clear(tab) }, 'palette.clear.keywords'),
        action('session.closeTab', { icon: 'x', hint: keys.closeTab(), run: () => closeTab(tab) }),
      );
      if (server) out.push(action('palette.editConnection', { icon: 'edit', run: () => editServer(server) }));
    }
    return out;
  }

  const items: Item[] = $derived.by(() => {
    const q = query.trim();
    const out: Item[] = [];

    if (q && (mode === 'connect' || looksLikeDestination(q))) {
      out.push({
        section: t('palette.sectionConnect'),
        label: t('palette.connectTo', { dest: q }),
        icon: 'zap',
        hint: '↵',
        score: 3e6,
        run: () => sessions.openDestination(q),
      });
      out.push({
        section: t('palette.sectionConnect'),
        label: t('palette.saveAs', { dest: q }),
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
        section: t('palette.sectionServers'),
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
      for (const tb of sessions.tabs) {
        const score = q ? fuzzyBest(q, tb.title, tb.subtitle) : 1;
        if (score <= 0 || tb.key === sessions.activeKey) continue;
        tabs.push({
          section: t('palette.sectionTabs'),
          label: tb.title,
          detail: tb.subtitle,
          icon: 'terminal',
          dot: tb.status,
          score: 1e6 + score,
          run: () => {
            sessions.activeKey = tb.key;
          },
        });
      }
      tabs.sort((a, b) => b.score - a.score);
      out.push(...tabs.slice(0, 5));

      const acts: Item[] = [];
      for (const a of actionList()) {
        const score = q ? Math.max(fuzzyScore(q, a.label), fuzzyScore(q, a.keywords ?? '') * 0.6) : 1;
        if (score <= 0) continue;
        acts.push({
          section: t('palette.sectionActions'),
          label: a.label,
          icon: a.icon,
          hint: a.hint,
          stay: a.stay,
          score,
          run: a.run,
        });
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
  <div class="palette" role="dialog" aria-label={t('palette.label')}>
    <div class="searchbar">
      {#if mode === 'connect'}
        <span class="chip"><Icon name="zap" size={13} /> {t('palette.chip')}</span>
      {:else}
        <Icon name="search" size={16} />
      {/if}
      <input
        bind:this={input}
        bind:value={app.palette.query}
        placeholder={t(mode === 'connect' ? 'palette.placeholderConnect' : 'palette.placeholderCommands')}
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
          {#if mode === 'connect'}
            <Rich key="palette.typeDestination" params={{ example: 'deploy@10.0.0.5:2222' }} tags={{ example: 'code' }} />
          {:else}
            {t('palette.noMatches')}
          {/if}
        </div>
      {/each}
    </div>

    <div class="foot">
      <span><kbd>↑</kbd><kbd>↓</kbd> {t('palette.navigate')}</span>
      <span><kbd>↵</kbd> {t('palette.open')}</span>
      <span><kbd>{isMac() ? '⌘↵' : 'Ctrl+↵'}</kbd> {t('palette.newTab')}</span>
      <span><kbd>esc</kbd> {t('palette.close')}</span>
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
  .none :global(code) {
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
