<script lang="ts">
  import { api, errorMessage } from '../api';
  import { app } from '../state/app.svelte';
  import { servers } from '../state/servers.svelte';
  import { sessions } from '../state/sessions.svelte';
  import type { ForwardKind, ForwardSpec } from '../types';
  import Icon from './Icon.svelte';
  import Modal from './Modal.svelte';

  // svelte-ignore state_referenced_locally
  const tabKey = sessions.activeKey;
  const tab = $derived(tabKey ? sessions.get(tabKey) : undefined);

  let kind = $state<ForwardKind>('local');
  let bindPort = $state('');
  let targetHost = $state('localhost');
  let targetPort = $state('');
  let save = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);

  const KINDS: { id: ForwardKind; label: string; hint: string }[] = [
    { id: 'local', label: 'Local', hint: 'Opens a port on this computer that leads to a host reachable from the server (ssh -L).' },
    { id: 'remote', label: 'Remote', hint: 'Opens a port on the server that leads back to a host reachable from this computer (ssh -R).' },
    { id: 'dynamic', label: 'SOCKS', hint: 'A local SOCKS5 proxy: point a browser or tool at it to browse through the server (ssh -D).' },
  ];

  const port = (v: string) => (/^\d+$/.test(v.trim()) ? Number(v.trim()) : NaN);
  const valid = $derived(
    port(bindPort) >= (kind === 'remote' ? 0 : 1) &&
      port(bindPort) <= 65535 &&
      (kind === 'dynamic' || (targetHost.trim() !== '' && port(targetPort) >= 1 && port(targetPort) <= 65535)),
  );
  const serverName = $derived(tab?.serverId ? servers.byId.get(tab.serverId)?.name : undefined);

  $effect(() => {
    // The session went away: nothing to manage.
    if (!tab || tab.status !== 'connected') app.forwardsOpen = false;
  });

  async function add(e: SubmitEvent) {
    e.preventDefault();
    if (!tab?.sessionId || !valid) return;
    error = null;
    busy = true;
    const spec: ForwardSpec = {
      kind,
      bindHost: kind === 'remote' ? 'localhost' : '127.0.0.1',
      bindPort: port(bindPort),
      targetHost: kind === 'dynamic' ? '' : targetHost.trim(),
      targetPort: kind === 'dynamic' ? 0 : port(targetPort),
    };
    try {
      await api.addForward(tab.sessionId, spec, save && tab.serverId ? tab.serverId : undefined);
      if (save) await servers.load();
      bindPort = '';
      targetPort = '';
    } catch (err) {
      error = errorMessage(err);
    } finally {
      busy = false;
    }
  }

  function remove(id: number) {
    if (tab?.sessionId) void api.removeForward(tab.sessionId, id).catch((e) => (error = errorMessage(e)));
  }
</script>

<Modal title="Port forwarding" subtitle={tab ? `${tab.title} · ${tab.subtitle}` : ''} width={540} onclose={() => (app.forwardsOpen = false)}>
  {#if tab}
    {#if tab.forwards.length}
      <div class="list">
        {#each tab.forwards as f (f.id)}
          <div class="item">
            <span class="kind">{f.spec.kind === 'dynamic' ? 'SOCKS' : f.spec.kind === 'local' ? 'L' : 'R'}</span>
            <span class="desc mono">{f.description}</span>
            {#if f.saved}<span class="saved">saved</span>{/if}
            <button class="icon-btn small" title="Stop" aria-label="Stop" onclick={() => remove(f.id)}>
              <Icon name="x" size={13} />
            </button>
          </div>
        {/each}
      </div>
    {:else}
      <p class="empty">No active forwards in this session.</p>
    {/if}

    <form class="add" onsubmit={add}>
      <div class="segmented">
        {#each KINDS as k (k.id)}
          <button type="button" class:on={kind === k.id} onclick={() => (kind = k.id)}>{k.label}</button>
        {/each}
      </div>
      <p class="hint">{KINDS.find((k) => k.id === kind)?.hint}</p>
      <div class="inputs">
        <label class="field">
          <span>{kind === 'remote' ? 'Server port' : 'Local port'}</span>
          <input class="input" bind:value={bindPort} inputmode="numeric" placeholder={kind === 'dynamic' ? '1080' : '8080'} />
        </label>
        {#if kind !== 'dynamic'}
          <span class="arrow">→</span>
          <label class="field grow">
            <span>{kind === 'local' ? 'Destination (from the server)' : 'Destination (from this computer)'}</span>
            <input class="input" bind:value={targetHost} spellcheck="false" />
          </label>
          <label class="field">
            <span>Port</span>
            <input class="input" bind:value={targetPort} inputmode="numeric" placeholder="5432" />
          </label>
        {/if}
      </div>
      <div class="bottom">
        {#if serverName}
          <label class="check"><input type="checkbox" bind:checked={save} /> Also start on every connection to {serverName}</label>
        {/if}
        <button class="btn primary" type="submit" disabled={!valid || busy}>Start</button>
      </div>
      {#if error}<p class="error">{error}</p>{/if}
    </form>
  {/if}
</Modal>

<style>
  .list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 16px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 6px 0 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-soft);
    background: var(--surface);
  }
  .kind {
    min-width: 42px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-2);
  }
  .desc {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .saved {
    font-size: 11px;
    color: var(--green);
  }
  .empty {
    margin: 0 0 16px;
    color: var(--text-2);
  }
  .add {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-top: 14px;
    border-top: 1px solid var(--border-soft);
  }
  .add .segmented {
    align-self: flex-start;
  }
  .hint {
    margin: 0;
    color: var(--text-2);
    font-size: 12px;
  }
  .inputs {
    display: flex;
    align-items: flex-end;
    gap: 8px;
  }
  .inputs .field {
    width: 100px;
  }
  .inputs .field.grow {
    flex: 1;
    width: auto;
  }
  .arrow {
    padding-bottom: 7px;
    color: var(--text-3);
  }
  .bottom {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .bottom .btn {
    margin-left: auto;
  }
  .error {
    margin: 0;
    color: var(--red);
  }
</style>
