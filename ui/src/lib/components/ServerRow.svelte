<script lang="ts">
  import { connect, serverMenu } from '../actions';
  import { t } from '../i18n.svelte';
  import { app } from '../state/app.svelte';
  import { sessions } from '../state/sessions.svelte';
  import { destination } from '../state/servers.svelte';
  import type { Server } from '../types';
  import Icon from './Icon.svelte';

  interface Props {
    server: Server;
  }
  let { server }: Props = $props();

  const status = $derived(sessions.serverStatus(server.id));
  const selected = $derived(!!sessions.active && sessions.active.serverId === server.id);
  /** Shared with AI agents (shown while agents may connect at all). */
  const shared = $derived(app.settings.agentsEnabled && !!server.agents && server.agents !== 'off');

  function onclick(e: MouseEvent) {
    // Ctrl/Cmd-click always opens another session.
    connect(server, e.ctrlKey || e.metaKey);
  }

  function onauxclick(e: MouseEvent) {
    if (e.button === 1) connect(server, true);
  }

  function menu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    serverMenu(server, e.clientX, e.clientY);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') connect(server, e.ctrlKey || e.metaKey);
  }
</script>

<div
  class="row"
  class:selected
  role="button"
  tabindex="0"
  title={destination(server)}
  {onclick}
  {onauxclick}
  {onkeydown}
  oncontextmenu={menu}
>
  <span class="dot {status === 'idle' ? '' : status}"></span>
  <span class="text">
    <span class="name">
      <span class="label">{server.name}</span>
      {#if shared}
        <span class="agents" title={t(server.agents === 'allow' ? 'server.agentsAllow' : 'server.agentsAsk')}>
          <Icon name="sparkle" size={12} stroke={1.9} />
        </span>
      {/if}
    </span>
    <span class="host">{server.host}</span>
  </span>
  <button class="icon-btn small more" aria-label={t('server.actions')} onclick={menu}>
    <Icon name="more" size={14} />
  </button>
</div>

<style>
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 40px;
    padding: 4px 6px 4px 12px;
    border-radius: 8px;
    transition: background 0.1s var(--ease);
  }
  .row:hover {
    background: var(--hover);
  }
  .row.selected {
    background: var(--active);
  }
  .row:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    line-height: 1.25;
  }
  .label,
  .host {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    font-weight: 500;
  }
  .agents {
    flex: none;
    display: grid;
    color: var(--text-3);
  }
  .host {
    font-size: 11.5px;
    color: var(--text-2);
  }
  .more {
    opacity: 0;
  }
  .row:hover .more,
  .row:focus-within .more {
    opacity: 1;
  }
</style>
