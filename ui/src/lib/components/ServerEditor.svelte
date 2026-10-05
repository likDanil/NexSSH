<script lang="ts">
  // Adding or editing a server, in tabs like the settings: general, sign-in, connection, ports,
  // commands on login, AI agents. A tab with something to fix shows a dot, and saving goes to it.
  import { onMount } from 'svelte';
  import { connect, deleteServer } from '../actions';
  import { api, errorMessage } from '../api';
  import { t, tn, type MessageKey } from '../i18n.svelte';
  import { app, type EditorState } from '../state/app.svelte';
  import { servers, suggestedGroups } from '../state/servers.svelte';
  import type { AgentAccess, AuthKind, ForwardKind, ForwardSpec, KeyInfo, Server } from '../types';
  import Icon from './Icon.svelte';
  import Modal from './Modal.svelte';
  import Select from './Select.svelte';
  import Suggest from './Suggest.svelte';

  interface Props {
    editor: EditorState;
  }
  let { editor }: Props = $props();

  // svelte-ignore state_referenced_locally
  const existing = editor.server;
  // svelte-ignore state_referenced_locally
  const initial: Server = existing
    ? structuredClone($state.snapshot(existing))
    : {
        id: '',
        name: '',
        host: '',
        port: 22,
        user: '',
        group: '',
        auth: 'auto',
        ...editor.preset,
      };

  type Section = 'general' | 'login' | 'connection' | 'forwarding' | 'commands' | 'agents';
  const SECTIONS: { id: Section; label: MessageKey }[] = [
    { id: 'general', label: 'editor.tabGeneral' },
    { id: 'login', label: 'editor.tabLogin' },
    { id: 'connection', label: 'editor.tabConnection' },
    { id: 'forwarding', label: 'editor.tabForwarding' },
    { id: 'commands', label: 'editor.tabCommands' },
    { id: 'agents', label: 'editor.tabAgents' },
  ];
  let section = $state<Section>('general');

  let name = $state(initial.name === initial.host && !existing ? '' : initial.name);
  let host = $state(initial.host);
  let port = $state(String(initial.port || 22));
  let user = $state(initial.user);
  let group = $state(initial.group);
  let auth = $state<AuthKind>(initial.auth);
  let identityFile = $state(initial.identityFile ?? '');
  // A login typed with the jump host (`ops@bastion`, from older versions or ~/.ssh/config)
  // moves to its own field.
  // svelte-ignore state_referenced_locally
  const initialJump = splitLogin(jumpDisplay(initial.jumpHost), initial.jumpUser ?? '');
  let jumpHost = $state(initialJump.host);
  let jumpUser = $state(initialJump.user);
  let jumpPassword = $state('');
  let hasJumpPassword = $state(false);
  let forgetJumpPassword = $state(false);
  let keepalive = $state(initial.keepaliveSecs != null ? String(initial.keepaliveSecs) : '');
  let timeout = $state(initial.connectTimeoutSecs != null ? String(initial.connectTimeoutSecs) : '');
  let forwards = $state<ForwardSpec[]>(initial.forwards ? [...initial.forwards] : []);
  let startupCommands = $state(initial.startupCommands ?? '');
  let agentAccess = $state<AgentAccess>(initial.agents ?? 'off');
  let password = $state('');
  let hasPassword = $state(false);
  let forgetPassword = $state(false);
  let keyList = $state<KeyInfo[]>([]);
  let error = $state<string | null>(null);
  let saving = $state(false);
  /** Saving was tried with something to fix: the fields say what. */
  let tried = $state(false);

  const AUTH: { id: AuthKind; label: MessageKey; hint: MessageKey }[] = [
    { id: 'auto', label: 'auth.auto', hint: 'auth.auto.hint' },
    { id: 'password', label: 'auth.password', hint: 'auth.password.hint' },
    { id: 'key', label: 'auth.key', hint: 'auth.key.hint' },
    { id: 'agent', label: 'auth.agent', hint: 'auth.agent.hint' },
  ];
  const authHint = $derived(AUTH.find((a) => a.id === auth)?.hint);

  const AGENT_ACCESS: { id: AgentAccess; label: MessageKey; hint: MessageKey }[] = [
    { id: 'off', label: 'editor.agentsOff', hint: 'editor.agentsOff.hint' },
    { id: 'ask', label: 'editor.agentsAsk', hint: 'editor.agentsAsk.hint' },
    { id: 'allow', label: 'editor.agentsAllow', hint: 'editor.agentsAllow.hint' },
  ];
  const agentHint = $derived(AGENT_ACCESS.find((a) => a.id === agentAccess)?.hint);

  const groupOptions = $derived([...new Set([...servers.groupNames, ...suggestedGroups()])].map((value) => ({ value })));
  const jumpOptions = $derived(
    servers.data.servers.filter((s) => s.id !== initial.id).map((s) => ({ value: s.name, hint: s.host })),
  );
  const keyOptions = $derived(
    keyList.map((k) => ({ value: k.path, hint: k.encrypted ? `${k.keyType} · ${t('editor.keyEncrypted')}` : k.keyType })),
  );
  const forwardKinds = $derived<{ value: ForwardKind; label: string }[]>([
    { value: 'local', label: t('forward.local') },
    { value: 'remote', label: t('forward.remote') },
    { value: 'dynamic', label: t('forward.dynamic') },
  ]);
  const selectedKey = $derived(keyList.find((k) => k.path === identityFile.trim()));
  const portValid = $derived(/^\d+$/.test(port) && Number(port) >= 1 && Number(port) <= 65535);
  const keyMissing = $derived(auth === 'key' && !identityFile.trim());
  /** Tabs with something to fix before saving. */
  const problems = $derived<Record<Section, boolean>>({
    general: !host.trim() || !portValid,
    login: keyMissing,
    connection: false,
    forwarding: false,
    commands: false,
    agents: false,
  });
  const commandCount = $derived(startupCommands.split('\n').filter((l) => l.trim()).length);

  function jumpDisplay(value?: string): string {
    if (!value) return '';
    return servers.byId.get(value)?.name ?? value;
  }

  /** A saved server by id, alias or name, the way the backend resolves jump hosts. */
  function findServer(value: string): Server | undefined {
    const key = value.toLowerCase();
    const list = servers.data.servers.filter((s) => s.id !== initial.id);
    return (
      list.find((s) => s.id === value) ?? list.find((s) => s.alias === value) ?? list.find((s) => s.name.toLowerCase() === key)
    );
  }

  /** What the jump host field names: one host typed in place gets its own login and password. */
  function jumpKind(value: string): { kind: 'none' | 'typed' | 'chain' } | { kind: 'saved'; server: Server } {
    const v = value.trim();
    if (!v || v.toLowerCase() === 'none') return { kind: 'none' };
    if (v.includes(',')) return { kind: 'chain' };
    const server = findServer(v);
    return server ? { kind: 'saved', server } : { kind: 'typed' };
  }

  /** `ops@bastion:22` → host `bastion:22`, login `ops` (unless a login is given already). */
  function splitLogin(host: string, user: string): { host: string; user: string } {
    const at = host.lastIndexOf('@');
    if (at < 0 || jumpKind(host).kind !== 'typed') return { host, user };
    return { host: host.slice(at + 1), user: user || host.slice(0, at) };
  }

  const jump = $derived(jumpKind(jumpHost));

  onMount(() => {
    api.keys().then((k) => (keyList = k)).catch(() => {});
    if (existing) {
      api.hasPassword(existing.id).then((h) => (hasPassword = h)).catch(() => {});
      api.hasPassword(existing.id, true).then((h) => (hasJumpPassword = h)).catch(() => {});
    }
  });

  function toNumber(v: string): number | undefined {
    const t = v.trim();
    if (!t) return undefined;
    const n = Number(t);
    return Number.isFinite(n) && n >= 0 ? Math.round(n) : undefined;
  }

  function resolveJump(v: string): string | undefined {
    const t = v.trim();
    if (!t) return undefined;
    // Store saved servers by id so renames do not break the reference.
    const target = jumpKind(t);
    return target.kind === 'saved' ? target.server.id : t;
  }

  /** The system file dialog, opened in ~/.ssh; the chosen path goes into the field. */
  async function browseKey() {
    try {
      const path = await api.pickKeyFile(t('editor.pickKey'));
      if (path) identityFile = path;
    } catch (e) {
      error = errorMessage(e);
    }
  }

  function addForward(kind: ForwardKind = 'local') {
    forwards.push({ kind, bindHost: kind === 'remote' ? 'localhost' : '127.0.0.1', bindPort: 0, targetHost: 'localhost', targetPort: 0 });
  }

  async function save(connectAfter: boolean) {
    if (saving) return;
    const broken = SECTIONS.find((s) => problems[s.id]);
    if (broken) {
      tried = true;
      section = broken.id;
      return;
    }
    error = null;
    saving = true;
    const typed = jump.kind === 'typed';
    const jumpParts = typed ? splitLogin(jumpHost.trim(), jumpUser.trim()) : { host: jumpHost, user: '' };
    const server: Server = {
      ...initial,
      name: name.trim() || host.trim(),
      host: host.trim(),
      port: Number(port),
      user: user.trim(),
      group: group.trim(),
      auth,
      identityFile: auth === 'key' || auth === 'auto' ? identityFile.trim() || undefined : undefined,
      jumpHost: resolveJump(jumpParts.host),
      jumpUser: jumpParts.user || undefined,
      keepaliveSecs: toNumber(keepalive),
      connectTimeoutSecs: toNumber(timeout),
      forwards: forwards
        .filter((f) => f.bindPort > 0 || f.kind === 'remote')
        .map((f) => ({ ...f, bindPort: Number(f.bindPort), targetPort: Number(f.targetPort) })),
      startupCommands: startupCommands.trim() ? startupCommands : undefined,
      agents: agentAccess === 'off' ? undefined : agentAccess,
    };
    try {
      const saved = await servers.save(server, {
        password: auth === 'password' ? password : undefined,
        clearPassword: forgetPassword,
        jumpPassword: typed ? jumpPassword : undefined,
        // Only a jump host typed in place has a password of its own.
        clearJumpPassword: hasJumpPassword && (forgetJumpPassword || !typed),
      });
      app.editor = null;
      if (connectAfter) connect(saved, true);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      saving = false;
    }
  }

  function onsubmit(e: SubmitEvent) {
    e.preventDefault();
    void save(false);
  }

  async function remove() {
    if (!existing) return;
    app.editor = null;
    await deleteServer(existing);
  }
</script>

<Modal title={t(existing ? 'editor.titleEdit' : 'editor.titleNew')} width={600} onclose={() => (app.editor = null)}>
  {#snippet toolbar()}
    <div class="tabs segmented" role="tablist">
      {#each SECTIONS as sec (sec.id)}
        <button
          type="button"
          role="tab"
          aria-selected={section === sec.id}
          class:on={section === sec.id}
          onclick={() => (section = sec.id)}
        >
          {t(sec.label)}
          {#if problems[sec.id] && (tried || sec.id !== 'general')}
            <span class="problem" aria-hidden="true"></span>
          {:else if sec.id === 'forwarding' && forwards.length}
            <span class="count">{forwards.length}</span>
          {:else if sec.id === 'commands' && commandCount}
            <span class="count">{commandCount}</span>
          {/if}
        </button>
      {/each}
    </div>
  {/snippet}

  <form id="server-form" class="panel" {onsubmit} autocomplete="off">
    {#if section === 'general'}
      <div class="rows">
        <div class="row">
          <label for="server-host">{t('editor.host')}</label>
          <div class="control address">
            <!-- svelte-ignore a11y_autofocus -->
            <input
              id="server-host"
              class="input"
              class:invalid={tried && !host.trim()}
              bind:value={host}
              placeholder={t('editor.hostPlaceholder')}
              spellcheck="false"
              autofocus={!existing}
            />
            <span class="colon">:</span>
            <input
              class="input port"
              class:invalid={!portValid}
              bind:value={port}
              inputmode="numeric"
              aria-label={t('editor.port')}
              title={t('editor.port')}
            />
          </div>
        </div>
        <div class="row">
          <label for="server-name">
            {t('editor.name')}
            <small>{t('editor.nameHint')}</small>
          </label>
          <input id="server-name" class="input control" bind:value={name} placeholder={host.trim() || 'web-01'} spellcheck="false" />
        </div>
        <div class="row">
          <label for="server-user">
            {t('editor.user')}
            <small>{t('editor.userHint')}</small>
          </label>
          <input
            id="server-user"
            class="input control"
            bind:value={user}
            placeholder={t('editor.userPlaceholder')}
            spellcheck="false"
          />
        </div>
        <div class="row">
          <label for="server-group">
            {t('editor.group')}
            <small>{t('editor.groupHint')}</small>
          </label>
          <div class="control">
            <Suggest id="server-group" bind:value={group} suggestions={groupOptions} placeholder={t('group.production')} />
          </div>
        </div>
      </div>
    {:else if section === 'login'}
      <div class="rows">
        <div class="row top">
          <span>
            {t('editor.method')}
            <small>{authHint ? t(authHint) : ''}</small>
          </span>
          <div class="segmented" role="radiogroup" aria-label={t('editor.method')}>
            {#each AUTH as a (a.id)}
              <button type="button" role="radio" aria-checked={auth === a.id} class:on={auth === a.id} onclick={() => (auth = a.id)}>
                {t(a.label)}
              </button>
            {/each}
          </div>
        </div>

        {#if auth === 'password'}
          <div class="row top">
            <label for="server-password">{t('editor.password')}</label>
            <div class="control stack">
              <input
                id="server-password"
                class="input"
                type="password"
                bind:value={password}
                placeholder={t(hasPassword && !forgetPassword ? 'editor.passwordSaved' : 'editor.passwordEmpty')}
                disabled={forgetPassword}
                autocomplete="new-password"
              />
              {#if hasPassword}
                <label class="check small">
                  <input type="checkbox" bind:checked={forgetPassword} />
                  {t('editor.forgetPassword')}
                </label>
              {/if}
            </div>
          </div>
          {#if app.keychainIssue}
            <p class="warn"><Icon name="alert" size={13} /> {t('editor.keychainUnavailable')}</p>
          {/if}
        {/if}

        {#if auth === 'key' || auth === 'auto'}
          <div class="row top">
            <label for="identity-file">
              {t(auth === 'key' ? 'editor.privateKey' : 'editor.privateKeyOptional')}
              {#if selectedKey}
                <small class="key">
                  <Icon name="key" size={11} />
                  {selectedKey.keyType}{selectedKey.encrypted ? ` · ${t('editor.keyProtected')}` : ''}
                </small>
              {:else if keyList.length && !identityFile}
                <small>{tn('editor.keysFound', keyList.length)}</small>
              {/if}
            </label>
            <div class="control picker" class:invalid={tried && keyMissing}>
              <Suggest id="identity-file" bind:value={identityFile} suggestions={keyOptions} placeholder="~/.ssh/id_ed25519" mono />
              <button type="button" class="btn" title={t('editor.pickKey')} onclick={browseKey}>
                <Icon name="folder" size={14} />
                {t('editor.browse')}
              </button>
            </div>
          </div>
          {#if selectedKey?.fingerprint}
            <p class="note mono">{selectedKey.fingerprint}</p>
          {/if}
        {/if}
      </div>
    {:else if section === 'connection'}
      <div class="rows">
        <div class="row top">
          <label for="jump-host">
            {t('editor.jumpHost')}
            <small>{t('editor.jumpHint')}</small>
          </label>
          <div class="control">
            <Suggest id="jump-host" bind:value={jumpHost} suggestions={jumpOptions} placeholder={t('editor.jumpPlaceholder')} />
          </div>
        </div>
        {#if jump.kind === 'typed'}
          <div class="row">
            <label for="jump-user">{t('editor.jumpUser')}</label>
            <input
              id="jump-user"
              class="input control"
              bind:value={jumpUser}
              placeholder={t('editor.userPlaceholder')}
              spellcheck="false"
            />
          </div>
          <div class="row top">
            <label for="jump-password">{t('editor.jumpPassword')}</label>
            <div class="control stack">
              <input
                id="jump-password"
                class="input"
                type="password"
                bind:value={jumpPassword}
                placeholder={t(hasJumpPassword && !forgetJumpPassword ? 'editor.jumpPasswordSaved' : 'editor.jumpPasswordEmpty')}
                disabled={forgetJumpPassword}
                autocomplete="new-password"
              />
              {#if hasJumpPassword}
                <label class="check small">
                  <input type="checkbox" bind:checked={forgetJumpPassword} />
                  {t('editor.forgetPassword')}
                </label>
              {/if}
            </div>
          </div>
          {#if app.keychainIssue}
            <p class="warn"><Icon name="alert" size={13} /> {t('editor.keychainUnavailable')}</p>
          {/if}
        {:else if jump.kind === 'saved'}
          <p class="note">{t('editor.jumpSaved', { name: jump.server.name })}</p>
        {:else if jump.kind === 'chain'}
          <p class="note">{t('editor.jumpChain')}</p>
        {/if}
        <div class="row">
          <label for="keepalive">
            {t('editor.keepalive')}
            <small>{t('editor.keepaliveHint')}</small>
          </label>
          <input id="keepalive" class="input number" bind:value={keepalive} inputmode="numeric" placeholder="30" />
        </div>
        <div class="row">
          <label for="connect-timeout">
            {t('editor.timeout')}
            <small>{t('editor.timeoutHint')}</small>
          </label>
          <input id="connect-timeout" class="input number" bind:value={timeout} inputmode="numeric" placeholder="15" />
        </div>
      </div>
    {:else if section === 'forwarding'}
      <p class="lead">{t('editor.forwardingHint')}</p>
      <div class="forwards">
        {#each forwards as f, i (i)}
          <div class="fwd">
            <Select bind:value={f.kind} options={forwardKinds} label={t('editor.forwarding')} width={124} />
            <input
              class="input port"
              type="number"
              min="0"
              max="65535"
              bind:value={f.bindPort}
              title={t(f.kind === 'remote' ? 'editor.serverPort' : 'editor.localPort')}
            />
            {#if f.kind !== 'dynamic'}
              <span class="arrow">→</span>
              <input class="input" bind:value={f.targetHost} placeholder="localhost" spellcheck="false" />
              <input class="input port" type="number" min="1" max="65535" bind:value={f.targetPort} />
            {:else}
              <span class="muted grow">{t('editor.socksOn', { port: f.bindPort || '…' })}</span>
            {/if}
            <button type="button" class="icon-btn small" aria-label={t('common.remove')} onclick={() => forwards.splice(i, 1)}>
              <Icon name="x" size={13} />
            </button>
          </div>
        {:else}
          <p class="empty">{t('editor.forwardingEmpty')}</p>
        {/each}
        <button type="button" class="btn ghost add-fwd" onclick={() => addForward()}>
          <Icon name="plus" size={13} /> {t('editor.addForward')}
        </button>
      </div>
      <dl class="legend">
        <dt>{t('forward.local')}</dt>
        <dd>{t('forward.local.hint')}</dd>
        <dt>{t('forward.remote')}</dt>
        <dd>{t('forward.remote.hint')}</dd>
        <dt>{t('forward.dynamic')}</dt>
        <dd>{t('forward.dynamic.hint')}</dd>
      </dl>
    {:else if section === 'commands'}
      <label class="label block" for="startup-commands">{t('editor.commands')}</label>
      <p class="lead">{t('editor.commandsHint')}</p>
      <textarea
        id="startup-commands"
        class="input commands"
        bind:value={startupCommands}
        placeholder={t('editor.commandsPlaceholder')}
        spellcheck="false"
        autocomplete="off"
        rows="7"
      ></textarea>
      <p class="note">{t('editor.commandsNote')}</p>
    {:else}
      <div class="rows">
        <div class="row top">
          <span>
            {t('editor.agentsAccess')}
            <small>{agentHint ? t(agentHint) : ''}</small>
          </span>
          <div class="segmented" role="radiogroup" aria-label={t('editor.agents')}>
            {#each AGENT_ACCESS as a (a.id)}
              <button
                type="button"
                role="radio"
                aria-checked={agentAccess === a.id}
                class:on={agentAccess === a.id}
                onclick={() => (agentAccess = a.id)}
              >
                {t(a.label)}
              </button>
            {/each}
          </div>
        </div>
      </div>
      {#if agentAccess !== 'off' && !app.settings.agentsEnabled}
        <p class="warn"><Icon name="alert" size={13} /> {t('editor.agentsDisabled')}</p>
      {/if}
    {/if}

    {#if error}<p class="error">{error}</p>{/if}
  </form>

  {#snippet footer()}
    {#if existing}
      <button type="button" class="btn danger ghost" onclick={remove}>
        <Icon name="trash" size={14} /> {t('common.delete')}
      </button>
    {/if}
    <span class="spacer"></span>
    <button type="button" class="btn" onclick={() => (app.editor = null)}>{t('common.cancel')}</button>
    <button type="submit" form="server-form" class="btn" disabled={saving}>{t('common.save')}</button>
    <button type="button" class="btn primary" disabled={saving} onclick={() => save(true)}>{t('editor.saveConnect')}</button>
  {/snippet}
</Modal>

<style>
  /* One line whatever the labels: a strip that would not fit scrolls rather than wraps. */
  .tabs {
    display: flex;
    width: fit-content;
    max-width: 100%;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabs::-webkit-scrollbar {
    display: none;
  }
  .tabs button {
    flex: none;
    white-space: nowrap;
  }
  .tabs button:focus-visible {
    outline-offset: -2px;
  }
  .tabs button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .problem {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--red);
  }
  .count {
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--active);
    color: var(--text-2);
    font-size: 10.5px;
    line-height: 16px;
    text-align: center;
  }
  /* About as tall as the tallest tab, so the dialog does not jump between them. */
  .panel {
    min-height: 268px;
  }
  .rows {
    display: flex;
    flex-direction: column;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    min-height: 46px;
    padding: 7px 0;
    border-bottom: 1px solid var(--border-soft);
  }
  .row.top {
    align-items: flex-start;
  }
  .row > span:first-child,
  .row > label:first-child {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .row.top > span:first-child,
  .row.top > label:first-child {
    padding-top: 6px;
  }
  .row small {
    color: var(--text-2);
    font-size: 11.5px;
    line-height: 1.4;
    max-width: 260px;
  }
  .row > .segmented {
    flex: none;
  }
  .row .segmented button {
    white-space: nowrap;
  }
  .row small.key {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .control {
    width: 300px;
    flex: none;
  }
  .stack {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .address {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .address .input:first-child {
    flex: 1;
    min-width: 0;
  }
  .colon {
    color: var(--text-3);
  }
  .port {
    width: 76px;
    flex: none;
  }
  .number {
    width: 90px;
    flex: none;
    text-align: right;
  }
  .picker {
    display: flex;
    gap: 6px;
  }
  .picker :global(.input) {
    flex: 1;
    min-width: 0;
  }
  .picker .btn {
    flex: none;
    height: 32px;
  }
  .picker.invalid :global(.input) {
    border-color: var(--red);
  }
  .check.small {
    font-size: 12px;
    color: var(--text-2);
  }
  .note {
    margin: 8px 0 0;
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .note.mono {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .lead {
    margin: 0 0 12px;
    color: var(--text-2);
    font-size: 12.5px;
    line-height: 1.45;
  }
  .block {
    display: block;
    margin-bottom: 4px;
  }
  .warn {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 8px 0 0;
    color: var(--amber);
    font-size: 12px;
  }
  .forwards {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .fwd {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .arrow {
    color: var(--text-3);
  }
  .muted {
    color: var(--text-3);
    font-weight: 400;
  }
  .grow {
    flex: 1;
    font-size: 12px;
  }
  .empty {
    margin: 0;
    color: var(--text-3);
  }
  .add-fwd {
    align-self: flex-start;
    color: var(--text-2);
    height: 28px;
    padding: 0 8px;
  }
  .legend {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 10px;
    margin: 16px 0 0;
    padding-top: 12px;
    border-top: 1px solid var(--border-soft);
    font-size: 11.5px;
    line-height: 1.4;
  }
  .legend dt {
    color: var(--text);
    font-weight: 500;
  }
  .legend dd {
    margin: 0;
    color: var(--text-2);
  }
  .commands {
    height: auto;
    min-height: 150px;
    padding: 8px 10px;
    resize: vertical;
    font-family: var(--font-mono);
    font-size: 12.5px;
    line-height: 1.55;
    white-space: pre;
  }
  .error {
    margin: 14px 0 0;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    background: var(--red-soft);
    color: var(--red);
  }
  .spacer {
    flex: 1;
  }
</style>
