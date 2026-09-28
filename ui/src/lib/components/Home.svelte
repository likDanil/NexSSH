<script lang="ts">
  import logo from '../../assets/logo.png';
  import { addServer, connect, editServer, importSshConfig, keys, quickConnect, serverMenu } from '../actions';
  import { destination, servers } from '../state/servers.svelte';
  import { sessions } from '../state/sessions.svelte';
  import type { AuthKind, Server } from '../types';
  import Icon from './Icon.svelte';

  const AUTH_LABEL: Record<AuthKind, string> = { auto: 'Auto', password: 'Password', key: 'Key', agent: 'Agent' };

  const empty = $derived(servers.loaded && servers.data.servers.length === 0);

  function ago(ts?: number): string {
    if (!ts) return '';
    const s = Math.max(0, Date.now() / 1000 - ts);
    if (s < 60) return 'just now';
    if (s < 3600) return `${Math.floor(s / 60)} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    if (s < 86400 * 30) return `${Math.floor(s / 86400)} d ago`;
    return new Date(ts * 1000).toLocaleDateString();
  }

  function menu(e: MouseEvent, s: Server) {
    e.preventDefault();
    e.stopPropagation();
    serverMenu(s, e.clientX, e.clientY);
  }
</script>

{#snippet row(s: Server)}
  {@const status = sessions.serverStatus(s.id)}
  <div
    class="row"
    role="button"
    tabindex="0"
    onclick={(e) => connect(s, e.ctrlKey || e.metaKey)}
    onkeydown={(e) => e.key === 'Enter' && connect(s)}
    oncontextmenu={(e) => menu(e, s)}
  >
    <span class="dot {status === 'idle' ? '' : status}"></span>
    <span class="name">{s.name}</span>
    <span class="addr">{destination(s)}</span>
    <span class="meta">
      {#if s.jumpHost}<span class="tag" title="Jump host: {s.jumpHost}">via {s.jumpHost}</span>{/if}
      <span class="tag">{AUTH_LABEL[s.auth]}</span>
    </span>
    <span class="when">{ago(s.lastUsed)}</span>
    <span class="acts">
      <button class="icon-btn small" title="Edit" aria-label="Edit" onclick={(e) => (e.stopPropagation(), editServer(s))}>
        <Icon name="edit" size={14} />
      </button>
      <button class="icon-btn small" title="More" aria-label="More" onclick={(e) => menu(e, s)}>
        <Icon name="more" size={14} />
      </button>
    </span>
  </div>
{/snippet}

<div class="home">
  {#if empty}
    <div class="welcome">
      <img src={logo} alt="" class="logo" />
      <h1>Welcome to NexSSH</h1>
      <p>Add your first server, or import the hosts you already have in <code>~/.ssh/config</code>.</p>
      <div class="cta">
        <button class="btn primary" onclick={() => addServer()}><Icon name="plus" size={14} /> Add server</button>
        <button class="btn" onclick={importSshConfig}><Icon name="import" size={14} /> Import ~/.ssh/config</button>
        <button class="btn ghost" onclick={quickConnect}><Icon name="zap" size={14} /> Quick connect</button>
      </div>
      <p class="hint">Press <kbd>{keys.palette()}</kbd> anytime to search servers and run commands.</p>
    </div>
  {:else if servers.loaded}
    <div class="page">
      <header>
        <div>
          <h1>Servers</h1>
          <p>{servers.data.servers.length} saved · click to connect, right-click for more</p>
        </div>
        <div class="head-actions">
          <button class="btn ghost" onclick={importSshConfig}><Icon name="import" size={14} /> Import</button>
          <button class="btn" onclick={() => addServer()}><Icon name="plus" size={14} /> Add Server</button>
        </div>
      </header>

      {#if servers.recent.length}
        <section>
          <h2>Recent</h2>
          {#each servers.recent as s (s.id)}{@render row(s)}{/each}
        </section>
      {/if}

      {#each servers.groups as group (group.name)}
        <section>
          <h2>{group.name || 'Other'}</h2>
          {#each group.servers as s (s.id)}{@render row(s)}{/each}
        </section>
      {/each}

      <p class="hint">
        <kbd>{keys.palette()}</kbd> command palette · <kbd>{keys.newSession()}</kbd> new session ·
        <kbd>{keys.settings()}</kbd> settings
      </p>
    </div>
  {/if}
</div>

<style>
  .home {
    position: absolute;
    inset: 0;
    overflow: auto;
    background: var(--surface);
  }
  .welcome {
    min-height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 40px 24px;
    text-align: center;
  }
  .logo {
    width: 64px;
    margin-bottom: 18px;
    filter: var(--logo-filter);
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .welcome p {
    max-width: 400px;
    margin: 8px 0 0;
    color: var(--text-2);
    line-height: 1.5;
  }
  .cta {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
    margin: 22px 0 10px;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  .hint kbd {
    margin: 0 2px;
  }
  code {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .page {
    max-width: 880px;
    margin: 0 auto;
    padding: 28px 28px 20px;
  }
  header {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 18px;
  }
  header p {
    margin: 4px 0 0;
    color: var(--text-2);
  }
  .head-actions {
    display: flex;
    gap: 6px;
  }
  section {
    margin-bottom: 18px;
  }
  h2 {
    margin: 0 0 4px 10px;
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--text-2);
  }
  .row {
    display: grid;
    grid-template-columns: 8px minmax(120px, 1fr) minmax(150px, 1.4fr) 160px 80px 56px;
    align-items: center;
    gap: 12px;
    height: 38px;
    padding: 0 8px 0 12px;
    border-radius: 8px;
  }
  .row:hover {
    background: var(--hover);
  }
  .name {
    font-weight: 500;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .addr {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-2);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .meta {
    display: flex;
    justify-content: flex-end;
    gap: 4px;
    min-width: 0;
    overflow: hidden;
  }
  .tag {
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid var(--border-soft);
    color: var(--text-2);
    font-size: 11px;
    white-space: nowrap;
  }
  .when {
    color: var(--text-3);
    font-size: 11.5px;
    text-align: right;
  }
  .acts {
    display: flex;
    justify-content: flex-end;
    gap: 2px;
    opacity: 0;
  }
  .row:hover .acts,
  .row:focus-within .acts {
    opacity: 1;
  }
  @media (max-width: 820px) {
    .row {
      grid-template-columns: 8px 1fr 1.2fr 56px;
    }
    .meta,
    .when {
      display: none;
    }
  }
</style>
