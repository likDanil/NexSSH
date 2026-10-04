<script lang="ts">
  import { onMount } from 'svelte';
  import { connect, deleteServer } from '../actions';
  import { api, errorMessage } from '../api';
  import { t, tn, type MessageKey } from '../i18n.svelte';
  import { app, type EditorState } from '../state/app.svelte';
  import { servers, suggestedGroups } from '../state/servers.svelte';
  import type { AuthKind, ForwardKind, ForwardSpec, KeyInfo, Server } from '../types';
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
  let password = $state('');
  let hasPassword = $state(false);
  let forgetPassword = $state(false);
  let keyList = $state<KeyInfo[]>([]);
  let advanced = $state(!!(initial.jumpHost || initial.forwards?.length || initial.keepaliveSecs != null || initial.connectTimeoutSecs != null));
  let error = $state<string | null>(null);
  let saving = $state(false);

  const AUTH: { id: AuthKind; label: MessageKey; hint: MessageKey }[] = [
    { id: 'auto', label: 'auth.auto', hint: 'auth.auto.hint' },
    { id: 'password', label: 'auth.password', hint: 'auth.password.hint' },
    { id: 'key', label: 'auth.key', hint: 'auth.key.hint' },
    { id: 'agent', label: 'auth.agent', hint: 'auth.agent.hint' },
  ];
  const authHint = $derived(AUTH.find((a) => a.id === auth)?.hint);

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
  const canSave = $derived(host.trim().length > 0 && portValid && !saving && (auth !== 'key' || identityFile.trim().length > 0));

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
    if (!canSave) return;
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

<Modal title={t(existing ? 'editor.titleEdit' : 'editor.titleNew')} width={540} onclose={() => (app.editor = null)}>
  <form id="server-form" {onsubmit} autocomplete="off">
    <div class="grid">
      <label class="field c9">
        <span>{t('editor.host')}</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="input"
          bind:value={host}
          placeholder={t('editor.hostPlaceholder')}
          spellcheck="false"
          autofocus={!existing}
        />
      </label>
      <label class="field c3">
        <span>{t('editor.port')}</span>
        <input class="input" class:invalid={!portValid} bind:value={port} inputmode="numeric" />
      </label>

      <label class="field c4">
        <span>{t('editor.name')}</span>
        <input class="input" bind:value={name} placeholder={host.trim() || 'web-01'} spellcheck="false" />
      </label>
      <label class="field c4">
        <span>{t('editor.user')}</span>
        <input class="input" bind:value={user} placeholder={t('editor.userPlaceholder')} spellcheck="false" />
      </label>
      <label class="field c4">
        <span>{t('editor.group')}</span>
        <Suggest bind:value={group} suggestions={groupOptions} placeholder={t('group.production')} />
      </label>
    </div>

    <div class="section">
      <span class="label">{t('editor.auth')}</span>
      <div class="segmented" role="radiogroup">
        {#each AUTH as a (a.id)}
          <button type="button" role="radio" aria-checked={auth === a.id} class:on={auth === a.id} onclick={() => (auth = a.id)}>
            {t(a.label)}
          </button>
        {/each}
      </div>
      <p class="hint">{authHint ? t(authHint) : ''}</p>

      {#if auth === 'password'}
        <label class="field">
          <span>{t('editor.password')}</span>
          <input
            class="input"
            type="password"
            bind:value={password}
            placeholder={t(hasPassword && !forgetPassword ? 'editor.passwordSaved' : 'editor.passwordEmpty')}
            disabled={forgetPassword}
          />
        </label>
        {#if hasPassword}
          <label class="check small">
            <input type="checkbox" bind:checked={forgetPassword} />
            {t('editor.forgetPassword')}
          </label>
        {/if}
        {#if app.keychainIssue}
          <p class="warn"><Icon name="alert" size={13} /> {t('editor.keychainUnavailable')}</p>
        {/if}
      {/if}

      {#if auth === 'key' || auth === 'auto'}
        <div class="field">
          <label class="label" for="identity-file">{t(auth === 'key' ? 'editor.privateKey' : 'editor.privateKeyOptional')}</label>
          <div class="picker">
            <Suggest id="identity-file" bind:value={identityFile} suggestions={keyOptions} placeholder="~/.ssh/id_ed25519" mono />
            <button type="button" class="btn" title={t('editor.pickKey')} onclick={browseKey}>
              <Icon name="folder" size={14} />
              {t('editor.browse')}
            </button>
          </div>
        </div>
        {#if selectedKey}
          <p class="hint">
            <Icon name="key" size={12} />
            {selectedKey.keyType}{selectedKey.encrypted ? ` · ${t('editor.keyProtected')}` : ''}{selectedKey.fingerprint
              ? ` · ${selectedKey.fingerprint}`
              : ''}
          </p>
        {:else if keyList.length && !identityFile}
          <p class="hint">{tn('editor.keysFound', keyList.length)}</p>
        {/if}
      {/if}
    </div>

    <button type="button" class="disclosure" onclick={() => (advanced = !advanced)} aria-expanded={advanced}>
      <span class="chev" class:open={advanced}><Icon name="chevronRight" size={13} stroke={2} /></span>
      {t('editor.moreOptions')}
    </button>

    {#if advanced}
      <div class="grid">
        <label class="field c6">
          <span>{t('editor.jumpHost')}</span>
          <Suggest bind:value={jumpHost} suggestions={jumpOptions} placeholder={t('editor.jumpPlaceholder')} />
        </label>
        <label class="field c3">
          <span>{t('editor.keepalive')}</span>
          <input class="input" bind:value={keepalive} inputmode="numeric" placeholder="30" />
        </label>
        <label class="field c3">
          <span>{t('editor.timeout')}</span>
          <input class="input" bind:value={timeout} inputmode="numeric" placeholder="15" />
        </label>

        {#if jump.kind === 'typed'}
          <label class="field c5">
            <span>{t('editor.jumpUser')}</span>
            <input class="input" bind:value={jumpUser} placeholder={t('editor.userPlaceholder')} spellcheck="false" />
          </label>
          <label class="field c7">
            <span>{t('editor.jumpPassword')}</span>
            <input
              class="input"
              type="password"
              bind:value={jumpPassword}
              placeholder={t(hasJumpPassword && !forgetJumpPassword ? 'editor.jumpPasswordSaved' : 'editor.jumpPasswordEmpty')}
              disabled={forgetJumpPassword}
              autocomplete="new-password"
            />
          </label>
        {/if}
      </div>
      {#if (jump.kind === 'typed' && (hasJumpPassword || app.keychainIssue)) || jump.kind === 'saved' || jump.kind === 'chain'}
        <div class="jump-notes">
          {#if jump.kind === 'typed'}
            {#if hasJumpPassword}
              <label class="check small">
                <input type="checkbox" bind:checked={forgetJumpPassword} />
                {t('editor.forgetPassword')}
              </label>
            {/if}
            {#if app.keychainIssue}
              <p class="warn"><Icon name="alert" size={13} /> {t('editor.keychainUnavailable')}</p>
            {/if}
          {:else if jump.kind === 'saved'}
            <p class="hint">{t('editor.jumpSaved', { name: jump.server.name })}</p>
          {:else}
            <p class="hint">{t('editor.jumpChain')}</p>
          {/if}
        </div>
      {/if}

      <div class="section">
        <span class="label">{t('editor.forwarding')} <span class="muted">{t('editor.forwardingNote')}</span></span>
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
        {/each}
        <button type="button" class="btn ghost add-fwd" onclick={() => addForward()}>
          <Icon name="plus" size={13} /> {t('editor.addForward')}
        </button>
      </div>
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
    <button type="submit" form="server-form" class="btn" disabled={!canSave}>{t('common.save')}</button>
    <button type="button" class="btn primary" disabled={!canSave} onclick={() => save(true)}>{t('editor.saveConnect')}</button>
  {/snippet}
</Modal>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(12, minmax(0, 1fr));
    gap: 12px;
  }
  .c3 {
    grid-column: span 3;
  }
  .c4 {
    grid-column: span 4;
  }
  .c5 {
    grid-column: span 5;
  }
  .c6 {
    grid-column: span 6;
  }
  .c7 {
    grid-column: span 7;
  }
  .jump-notes {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 10px;
  }
  .jump-notes .hint {
    margin: 0;
  }
  .c9 {
    grid-column: span 9;
  }
  .section {
    display: flex;
    flex-direction: column;
    gap: 9px;
    margin-top: 18px;
  }
  .hint {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: -2px 0 0;
    color: var(--text-2);
    font-size: 12px;
  }
  .warn {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    color: var(--amber);
    font-size: 12px;
  }
  .check.small {
    font-size: 12px;
    color: var(--text-2);
  }
  .disclosure {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 18px 0 12px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--text-2);
    font-weight: 500;
  }
  .disclosure:hover {
    color: var(--text);
  }
  .chev {
    display: grid;
    transition: transform 0.14s var(--ease);
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .fwd {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .picker {
    display: flex;
    gap: 6px;
  }
  .picker .btn {
    flex: none;
    height: 32px;
  }
  .fwd .port {
    width: 84px;
    flex: none;
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
  .add-fwd {
    align-self: flex-start;
    color: var(--text-2);
    height: 28px;
    padding: 0 8px;
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
