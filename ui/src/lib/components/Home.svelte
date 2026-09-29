<script lang="ts">
  import logo from '../../assets/logo.png';
  import {
    addServer,
    connect,
    editServer,
    importSshConfig,
    keys,
    openLocalTerminal,
    quickConnect,
    serverMenu,
  } from '../actions';
  import { t, timeAgo, tn, type MessageKey } from '../i18n.svelte';
  import { destination, servers } from '../state/servers.svelte';
  import { sessions } from '../state/sessions.svelte';
  import type { AuthKind, Server } from '../types';
  import Icon from './Icon.svelte';
  import Rich from './Rich.svelte';

  const AUTH_LABEL: Record<AuthKind, MessageKey> = {
    auto: 'auth.auto',
    password: 'auth.password',
    key: 'auth.key',
    agent: 'auth.agent',
  };

  const empty = $derived(servers.loaded && servers.data.servers.length === 0);

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
      {#if s.jumpHost}
        {@const jump = servers.byId.get(s.jumpHost)?.name ?? s.jumpHost}
        <span class="tag via" title={t('home.jumpHost', { name: jump })}>{t('home.via', { name: jump })}</span>
      {/if}
      <span class="tag">{t(AUTH_LABEL[s.auth])}</span>
    </span>
    <span class="when">{timeAgo(s.lastUsed)}</span>
    <span class="acts">
      <button
        class="icon-btn small"
        title={t('common.edit')}
        aria-label={t('common.edit')}
        onclick={(e) => (e.stopPropagation(), editServer(s))}
      >
        <Icon name="edit" size={14} />
      </button>
      <button class="icon-btn small" title={t('common.more')} aria-label={t('common.more')} onclick={(e) => menu(e, s)}>
        <Icon name="more" size={14} />
      </button>
    </span>
  </div>
{/snippet}

<div class="home">
  {#if empty}
    <div class="welcome">
      <img src={logo} alt="" class="logo" />
      <h1>{t('home.welcome')}</h1>
      <p><Rich key="home.welcomeBody" params={{ file: '~/.ssh/config' }} tags={{ file: 'code' }} /></p>
      <div class="cta">
        <button class="btn primary" onclick={() => addServer()}><Icon name="plus" size={14} /> {t('home.addServer')}</button>
        <button class="btn" onclick={importSshConfig}><Icon name="import" size={14} /> {t('home.importConfig')}</button>
        <button class="btn ghost" onclick={quickConnect}><Icon name="zap" size={14} /> {t('home.quickConnect')}</button>
        <button class="btn ghost" onclick={() => openLocalTerminal()}>
          <Icon name="terminal" size={14} />
          {t('home.localTerminal')}
        </button>
      </div>
      <p class="hint"><Rich key="home.paletteHint" params={{ keys: keys.palette() }} tags={{ keys: 'kbd' }} /></p>
    </div>
  {:else if servers.loaded}
    <div class="page">
      <header>
        <div>
          <h1>{t('home.title')}</h1>
          <p>{tn('home.summary', servers.data.servers.length)}</p>
        </div>
        <div class="head-actions">
          <button class="btn ghost" onclick={() => openLocalTerminal()} title={keys.localTerminal()}>
            <Icon name="terminal" size={14} />
            {t('home.localTerminal')}
          </button>
          <button class="btn ghost" onclick={importSshConfig}><Icon name="import" size={14} /> {t('home.import')}</button>
          <button class="btn" onclick={() => addServer()}><Icon name="plus" size={14} /> {t('sidebar.addServer')}</button>
        </div>
      </header>

      {#if servers.recent.length}
        <section>
          <h2>{t('home.recent')}</h2>
          {#each servers.recent as s (s.id)}{@render row(s)}{/each}
        </section>
      {/if}

      {#each servers.groups as group (group.name)}
        <section>
          <h2>{group.name || t('group.other')}</h2>
          {#each group.servers as s (s.id)}{@render row(s)}{/each}
        </section>
      {/each}

      <p class="hint">
        <Rich
          key="home.shortcuts"
          params={{
            palette: keys.palette(),
            newSession: keys.newSession(),
            local: keys.localTerminal(),
            settings: keys.settings(),
          }}
          tags={{ palette: 'kbd', newSession: 'kbd', local: 'kbd', settings: 'kbd' }}
        />
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
  .hint :global(kbd) {
    margin: 0 2px;
  }
  .home :global(code) {
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
    flex: none;
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid var(--border-soft);
    color: var(--text-2);
    font-size: 11px;
    white-space: nowrap;
  }
  /* A long jump host (typed as an address) shrinks; the full one is in the tooltip. */
  .tag.via {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
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
