<script lang="ts">
  import { closeTab, keys, quickConnect, sessionMenu, toggleSidebar } from '../actions';
  import { t } from '../i18n.svelte';
  import { currentOs } from '../platform';
  import { app } from '../state/app.svelte';
  import { sessions, type Tab } from '../state/sessions.svelte';
  import Icon from './Icon.svelte';
  import WindowControls from './WindowControls.svelte';

  const windows = $derived(app.info?.os === 'windows' || (!app.info && currentOs() === 'windows'));

  let dragFrom = $state<number | null>(null);
  let dropAt = $state<number | null>(null);

  function activate(tab: Tab) {
    sessions.activeKey = tab.key;
    requestAnimationFrame(() => sessions.focusActive());
  }

  function onauxclick(e: MouseEvent, tab: Tab) {
    if (e.button === 1) {
      e.preventDefault();
      closeTab(tab);
    }
  }

  function menu(e: MouseEvent, tab: Tab) {
    e.preventDefault();
    sessionMenu(tab, e.clientX, e.clientY);
  }

  function moreMenu(e: MouseEvent) {
    const tab = sessions.active;
    if (!tab) return;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    sessionMenu(tab, r.right - 210, r.bottom + 4);
  }

  function ondragstart(e: DragEvent, index: number) {
    dragFrom = index;
    e.dataTransfer?.setData('text/plain', String(index));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }

  function ondragover(e: DragEvent, index: number) {
    if (dragFrom === null) return;
    e.preventDefault();
    dropAt = index;
  }

  function ondrop(e: DragEvent) {
    e.preventDefault();
    if (dragFrom !== null && dropAt !== null) sessions.move(dragFrom, dropAt);
    dragFrom = dropAt = null;
  }
</script>

<header class="bar" data-tauri-drag-region>
  {#if app.settings.sidebarHidden}
    <button
      class="icon-btn"
      title="{t('tabs.showSidebar')} ({keys.sidebar()})"
      aria-label={t('tabs.showSidebar')}
      onclick={toggleSidebar}
    >
      <Icon name="sidebar" size={16} />
    </button>
  {/if}

  <div class="tabs" role="tablist" data-tauri-drag-region>
    {#each sessions.tabs as tab, i (tab.key)}
      <div
        class="tab"
        class:active={tab.key === sessions.activeKey}
        class:drop={dropAt === i && dragFrom !== i}
        role="tab"
        tabindex="0"
        aria-selected={tab.key === sessions.activeKey}
        title={tab.message ? `${tab.subtitle} — ${tab.message}` : tab.subtitle}
        draggable="true"
        onclick={() => activate(tab)}
        onkeydown={(e) => e.key === 'Enter' && activate(tab)}
        onauxclick={(e) => onauxclick(e, tab)}
        oncontextmenu={(e) => menu(e, tab)}
        ondragstart={(e) => ondragstart(e, i)}
        ondragover={(e) => ondragover(e, i)}
        {ondrop}
        ondragend={() => (dragFrom = dropAt = null)}
      >
        <span
          class="dot {tab.status === 'disconnected' ? (tab.failed ? 'error' : 'disconnected') : tab.status}"
        ></span>
        <span class="name">{tab.title}</span>
        <button
          class="close"
          aria-label={t('session.closeTab')}
          onclick={(e) => {
            e.stopPropagation();
            closeTab(tab);
          }}
        >
          <Icon name="x" size={12} stroke={2} />
        </button>
      </div>
    {/each}
    <button
      class="icon-btn new"
      title="{t('tabs.newSession')} ({keys.newSession()})"
      aria-label={t('tabs.newSession')}
      onclick={quickConnect}
    >
      <Icon name="plus" size={15} />
    </button>
  </div>

  <div class="actions">
    {#if sessions.active}
      {#if sessions.active.status === 'disconnected'}
        <button
          class="icon-btn"
          title="{t('session.reconnect')} ({keys.reconnect()})"
          aria-label={t('session.reconnect')}
          onclick={() => sessions.active && sessions.reconnect(sessions.active)}
        >
          <Icon name="refresh" size={15} />
        </button>
      {/if}
      <button class="icon-btn" title={t('tabs.sessionActions')} aria-label={t('tabs.sessionActions')} onclick={moreMenu}>
        <Icon name="more" size={16} />
      </button>
    {/if}
  </div>

  {#if windows}
    <WindowControls />
  {/if}
</header>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: var(--titlebar);
    padding-left: 4px;
    padding-right: 8px;
    flex: none;
    min-width: 0;
  }
  :global(:root[data-os='windows']) .bar {
    padding-right: 0;
  }
  .tabs {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    height: 100%;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabs::-webkit-scrollbar {
    display: none;
  }
  .tab {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    min-width: 96px;
    max-width: 200px;
    padding: 0 6px 0 11px;
    border-radius: 8px;
    color: var(--text-2);
    transition:
      background 0.1s var(--ease),
      color 0.1s var(--ease);
    flex: 0 1 auto;
  }
  .tab:hover {
    background: var(--hover);
    color: var(--text);
  }
  .tab.active {
    background: var(--surface);
    color: var(--text);
    box-shadow:
      0 0 0 1px var(--border-soft),
      var(--shadow-sm);
  }
  .tab.drop::before {
    content: '';
    position: absolute;
    left: -2px;
    top: 6px;
    bottom: 6px;
    width: 2px;
    border-radius: 2px;
    background: var(--text-3);
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-weight: 500;
  }
  .close {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    padding: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--text-2);
    opacity: 0;
  }
  .tab:hover .close,
  .tab.active .close {
    opacity: 1;
  }
  .close:hover {
    background: var(--hover);
    color: var(--text);
  }
  .new {
    flex: none;
    margin-left: 2px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
  }
</style>
