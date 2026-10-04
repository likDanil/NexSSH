<script lang="ts">
  import logo from '../../assets/logo.png';
  import {
    addServer,
    connect,
    importSshConfig,
    keys,
    openLocalTerminal,
    openSettings,
    quickConnect,
    showHome,
    toggleSidebar,
  } from '../actions';
  import { errorMessage } from '../api';
  import { fuzzyBest } from '../fuzzy';
  import { t } from '../i18n.svelte';
  import { app } from '../state/app.svelte';
  import { servers, type ServerGroup } from '../state/servers.svelte';
  import { sessions } from '../state/sessions.svelte';
  import { toasts } from '../state/toasts.svelte';
  import Icon from './Icon.svelte';
  import ServerRow from './ServerRow.svelte';
  import UpdateCard from './UpdateCard.svelte';

  let query = $state('');
  let searchEl: HTMLInputElement;

  const collapsed = $derived(new Set(app.settings.collapsedGroups));
  const dragRegion = $derived(app.fullscreen ? 'false' : '');

  const visible: ServerGroup[] = $derived.by(() => {
    const q = query.trim();
    if (!q) return servers.groups;
    return servers.groups
      .map((g) => ({
        name: g.name,
        servers: g.servers
          .map((s) => ({ s, score: fuzzyBest(q, s.name, s.host, s.user, s.alias, g.name) }))
          .filter((x) => x.score > 0)
          .sort((a, b) => b.score - a.score)
          .map((x) => x.s),
      }))
      .filter((g) => g.servers.length);
  });

  const empty = $derived(servers.loaded && servers.data.servers.length === 0);

  export function focusSearch() {
    searchEl?.focus();
    searchEl?.select();
  }

  function toggleGroup(name: string) {
    const next = new Set(collapsed);
    if (next.has(name)) next.delete(name);
    else next.add(name);
    app.update({ collapsedGroups: [...next] });
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      query = '';
      searchEl.blur();
      sessions.focusActive();
    } else if (e.key === 'Enter') {
      const first = visible[0]?.servers[0];
      if (first) {
        connect(first, e.ctrlKey || e.metaKey);
        query = '';
      }
    }
  }

  function groupMenu(e: MouseEvent, name: string) {
    e.preventDefault();
    if (!name) return;
    app.showMenu(e.clientX, e.clientY, [
      { label: t('group.addServer'), icon: 'plus', action: () => addServer({ group: name }) },
      {
        label: t('group.rename'),
        icon: 'edit',
        action: async () => {
          const next = await app.prompt(t('group.renameTitle'), '', name);
          if (next && next !== name) {
            servers.renameGroup(name, next).catch((err) => toasts.error(errorMessage(err)));
          }
        },
      },
      'separator',
      {
        label: t('group.delete'),
        icon: 'trash',
        danger: true,
        action: async () => {
          const ok = await app.confirm(
            t('group.deleteTitle', { name }),
            t('group.deleteMessage'),
            t('group.delete'),
            true,
          );
          if (ok) servers.deleteGroup(name).catch((err) => toasts.error(errorMessage(err)));
        },
      },
    ]);
  }

  // ---- width -------------------------------------------------------------------
  let width = $derived(app.settings.sidebarWidth);
  let dragging = $state(false);

  function startResize(e: PointerEvent) {
    e.preventDefault();
    dragging = true;
    const startX = e.clientX;
    const startWidth = app.settings.sidebarWidth;
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      width = Math.round(Math.min(380, Math.max(190, startWidth + ev.clientX - startX)));
    };
    const up = () => {
      dragging = false;
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', up);
      app.update({ sidebarWidth: width });
    };
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', up);
  }
</script>

<aside class="sidebar" style:width="{width}px">
  <div class="brand" data-tauri-drag-region={dragRegion}>
    <img src={logo} alt="" class="logo" draggable="false" />
    <span class="title" data-tauri-drag-region={dragRegion}>NexSSH</span>
    <button
      class="icon-btn hide"
      title="{t('sidebar.hide')} ({keys.sidebar()})"
      aria-label={t('sidebar.hide')}
      onclick={toggleSidebar}
    >
      <Icon name="sidebar" size={16} />
    </button>
  </div>

  <label class="search">
    <Icon name="search" size={14} />
    <input
      bind:this={searchEl}
      bind:value={query}
      placeholder={t('sidebar.search')}
      spellcheck="false"
      autocomplete="off"
      onkeydown={onSearchKey}
    />
    {#if query}
      <button class="icon-btn small" aria-label={t('sidebar.clearSearch')} onclick={() => (query = '')}>
        <Icon name="x" size={12} />
      </button>
    {/if}
  </label>

  <nav>
    <button class:on={!sessions.active} onclick={showHome}>
      <Icon name="server" size={15} /> <span>{t('nav.servers')}</span>
    </button>
    <button onclick={quickConnect}>
      <Icon name="zap" size={15} /> <span>{t('nav.quickConnect')}</span>
      <kbd>{keys.palette()}</kbd>
    </button>
    <!-- The shortcut is in the tooltip: next to this label it would not fit. -->
    <button onclick={() => openLocalTerminal()} title="{t('nav.localTerminal')} ({keys.localTerminal()})">
      <Icon name="terminal" size={15} /> <span>{t('nav.localTerminal')}</span>
    </button>
    <button onclick={() => openSettings()}>
      <Icon name="settings" size={15} /> <span>{t('nav.settings')}</span>
    </button>
  </nav>

  <div class="list">
    {#each visible as group (group.name)}
      {@const isCollapsed = !query && collapsed.has(group.name)}
      <div class="group">
        {#if group.name || visible.length > 1}
          <button
            class="group-head"
            onclick={() => toggleGroup(group.name)}
            oncontextmenu={(e) => groupMenu(e, group.name)}
            aria-expanded={!isCollapsed}
          >
            <span class="chev" class:closed={isCollapsed}><Icon name="chevronDown" size={12} stroke={2} /></span>
            <span class="gname">{group.name || t('group.other')}</span>
            <span class="count">{group.servers.length}</span>
          </button>
        {/if}
        {#if !isCollapsed}
          {#each group.servers as server (server.id)}
            <ServerRow {server} />
          {/each}
        {/if}
      </div>
    {:else}
      {#if query}
        <p class="note">{t('sidebar.noMatches', { query })}</p>
      {:else if empty}
        <p class="note">{t('sidebar.empty')}</p>
      {/if}
    {/each}
  </div>

  <UpdateCard />

  <div class="footer">
    <button class="add" onclick={() => addServer()}>
      <Icon name="plus" size={15} /> {t('sidebar.addServer')}
    </button>
    <button class="icon-btn" title={t('sidebar.import')} aria-label={t('sidebar.import')} onclick={importSshConfig}>
      <Icon name="import" size={15} />
    </button>
  </div>

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="resizer" class:dragging onpointerdown={startResize} ondblclick={() => app.update({ sidebarWidth: 240 })}></div>
</aside>

<style>
  .sidebar {
    position: relative;
    flex: none;
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--frame);
    min-width: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    height: var(--titlebar);
    padding: 0 16px;
    flex: none;
  }
  :global(:root[data-os='macos']) .brand {
    padding-left: 80px;
  }
  .logo {
    width: 20px;
    height: auto;
    filter: var(--logo-filter);
    pointer-events: none;
  }
  .title {
    flex: 1;
    font-weight: 600;
    font-size: 13.5px;
    letter-spacing: -0.01em;
  }
  .brand .hide {
    margin-right: -6px;
    color: var(--text-2);
  }
  .brand .hide:hover {
    color: var(--text);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 4px 10px 8px;
    height: 30px;
    padding: 0 6px 0 10px;
    border-radius: var(--radius-sm);
    border: 1px solid transparent;
    background: var(--hover);
    color: var(--text-2);
    transition:
      border-color 0.12s,
      background 0.12s;
  }
  .search:focus-within {
    background: var(--input);
    border-color: var(--border);
  }
  .search input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    background: transparent;
    color: var(--text);
  }
  .search input::placeholder {
    color: var(--text-2);
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 0 10px 10px;
    border-bottom: 1px solid var(--border-soft);
  }
  nav button {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 30px;
    padding: 0 8px 0 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text);
    text-align: left;
  }
  nav button span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  nav button :global(svg) {
    color: var(--text-2);
  }
  nav button:hover {
    background: var(--hover);
  }
  nav button.on {
    background: var(--active);
  }
  nav kbd {
    opacity: 0;
    transition: opacity 0.12s;
  }
  nav button:hover kbd {
    opacity: 1;
  }
  .list {
    flex: 1;
    overflow-y: auto;
    padding: 6px 10px 10px;
  }
  .group {
    margin-bottom: 4px;
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    height: 28px;
    margin-top: 4px;
    padding: 0 6px 0 4px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text-2);
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-align: left;
  }
  .group-head:hover {
    color: var(--text);
  }
  .chev {
    display: grid;
    place-items: center;
    width: 14px;
    transition: transform 0.14s var(--ease);
  }
  .chev.closed {
    transform: rotate(-90deg);
  }
  .gname {
    flex: 1;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .count {
    font-weight: 500;
    color: var(--text-3);
  }
  .note {
    margin: 12px 8px;
    color: var(--text-2);
  }
  .footer {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 8px 10px 10px;
    border-top: 1px solid var(--border-soft);
  }
  .add {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 9px;
    height: 32px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-2);
    font-weight: 500;
  }
  .add:hover {
    background: var(--hover);
    color: var(--text);
  }
  .resizer {
    position: absolute;
    top: 0;
    right: -3px;
    width: 6px;
    height: 100%;
    cursor: col-resize;
    z-index: 5;
  }
  .resizer:hover,
  .resizer.dragging {
    background: linear-gradient(to right, transparent 2px, var(--border) 2px, var(--border) 4px, transparent 4px);
  }
</style>
