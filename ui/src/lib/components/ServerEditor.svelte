<script lang="ts">
  import { onMount } from 'svelte';
  import { connect, deleteServer } from '../actions';
  import { api, errorMessage } from '../api';
  import { app, type EditorState } from '../state/app.svelte';
  import { SUGGESTED_GROUPS, servers } from '../state/servers.svelte';
  import type { AuthKind, ForwardKind, ForwardSpec, KeyInfo, Server } from '../types';
  import Icon from './Icon.svelte';
  import Modal from './Modal.svelte';

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
  let jumpHost = $state(jumpDisplay(initial.jumpHost));
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

  const AUTH: { id: AuthKind; label: string; hint: string }[] = [
    { id: 'auto', label: 'Auto', hint: 'SSH agent, your keys, then password — like the ssh command.' },
    { id: 'password', label: 'Password', hint: 'Stored only in the system keychain, or asked on connect.' },
    { id: 'key', label: 'Key', hint: 'A private key file. Its passphrase is asked once and can be saved in the keychain.' },
    { id: 'agent', label: 'Agent', hint: 'Only keys from ssh-agent, Pageant or another SSH agent.' },
  ];

  const groupOptions = $derived([...new Set([...servers.groupNames, ...SUGGESTED_GROUPS])]);
  const jumpOptions = $derived(servers.data.servers.filter((s) => s.id !== initial.id));
  const selectedKey = $derived(keyList.find((k) => k.path === identityFile.trim()));
  const portValid = $derived(/^\d+$/.test(port) && Number(port) >= 1 && Number(port) <= 65535);
  const canSave = $derived(host.trim().length > 0 && portValid && !saving && (auth !== 'key' || identityFile.trim().length > 0));

  function jumpDisplay(value?: string): string {
    if (!value) return '';
    return servers.byId.get(value)?.name ?? value;
  }

  onMount(() => {
    api.keys().then((k) => (keyList = k)).catch(() => {});
    if (existing) api.hasPassword(existing.id).then((h) => (hasPassword = h)).catch(() => {});
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
    const byName = servers.data.servers.find((s) => s.name === t);
    return byName ? byName.id : t;
  }

  function addForward(kind: ForwardKind = 'local') {
    forwards.push({ kind, bindHost: kind === 'remote' ? 'localhost' : '127.0.0.1', bindPort: 0, targetHost: 'localhost', targetPort: 0 });
  }

  async function save(connectAfter: boolean) {
    if (!canSave) return;
    error = null;
    saving = true;
    const server: Server = {
      ...initial,
      name: name.trim() || host.trim(),
      host: host.trim(),
      port: Number(port),
      user: user.trim(),
      group: group.trim(),
      auth,
      identityFile: auth === 'key' || auth === 'auto' ? identityFile.trim() || undefined : undefined,
      jumpHost: resolveJump(jumpHost),
      keepaliveSecs: toNumber(keepalive),
      connectTimeoutSecs: toNumber(timeout),
      forwards: forwards
        .filter((f) => f.bindPort > 0 || f.kind === 'remote')
        .map((f) => ({ ...f, bindPort: Number(f.bindPort), targetPort: Number(f.targetPort) })),
    };
    try {
      const saved = await servers.save(server, auth === 'password' ? password : undefined, forgetPassword);
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

<Modal title={existing ? 'Edit server' : 'New server'} width={540} onclose={() => (app.editor = null)}>
  <form id="server-form" {onsubmit} autocomplete="off">
    <div class="grid">
      <label class="field c9">
        <span>Host</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input" bind:value={host} placeholder="192.168.1.10 or example.com" spellcheck="false" autofocus={!existing} />
      </label>
      <label class="field c3">
        <span>Port</span>
        <input class="input" class:invalid={!portValid} bind:value={port} inputmode="numeric" />
      </label>

      <label class="field c4">
        <span>Name</span>
        <input class="input" bind:value={name} placeholder={host.trim() || 'web-01'} spellcheck="false" />
      </label>
      <label class="field c4">
        <span>Username</span>
        <input class="input" bind:value={user} placeholder="root" spellcheck="false" />
      </label>
      <label class="field c4">
        <span>Group</span>
        <input class="input" bind:value={group} list="nexssh-groups" placeholder="Production" spellcheck="false" />
      </label>
    </div>
    <datalist id="nexssh-groups">
      {#each groupOptions as g (g)}<option value={g}></option>{/each}
    </datalist>

    <div class="section">
      <span class="label">Authentication</span>
      <div class="segmented" role="radiogroup">
        {#each AUTH as a (a.id)}
          <button type="button" role="radio" aria-checked={auth === a.id} class:on={auth === a.id} onclick={() => (auth = a.id)}>
            {a.label}
          </button>
        {/each}
      </div>
      <p class="hint">{AUTH.find((a) => a.id === auth)?.hint}</p>

      {#if auth === 'password'}
        <label class="field">
          <span>Password</span>
          <input
            class="input"
            type="password"
            bind:value={password}
            placeholder={hasPassword && !forgetPassword ? 'Saved in keychain — type to replace' : 'Leave empty to be asked when connecting'}
            disabled={forgetPassword}
          />
        </label>
        {#if hasPassword}
          <label class="check small"><input type="checkbox" bind:checked={forgetPassword} /> Forget the saved password</label>
        {/if}
        {#if app.keychainIssue}
          <p class="warn"><Icon name="alert" size={13} /> System keychain unavailable: passwords cannot be saved.</p>
        {/if}
      {/if}

      {#if auth === 'key' || auth === 'auto'}
        <label class="field">
          <span>{auth === 'key' ? 'Private key' : 'Private key (optional)'}</span>
          <input class="input mono" bind:value={identityFile} list="nexssh-keys" placeholder="~/.ssh/id_ed25519" spellcheck="false" />
        </label>
        <datalist id="nexssh-keys">
          {#each keyList as k (k.path)}<option value={k.path}>{k.keyType}{k.encrypted ? ' · encrypted' : ''}</option>{/each}
        </datalist>
        {#if selectedKey}
          <p class="hint">
            <Icon name="key" size={12} />
            {selectedKey.keyType}{selectedKey.encrypted ? ' · passphrase protected' : ''}{selectedKey.fingerprint ? ` · ${selectedKey.fingerprint}` : ''}
          </p>
        {:else if keyList.length && !identityFile}
          <p class="hint">Found {keyList.length} key{keyList.length === 1 ? '' : 's'} in ~/.ssh — pick one from the list.</p>
        {/if}
      {/if}
    </div>

    <button type="button" class="disclosure" onclick={() => (advanced = !advanced)} aria-expanded={advanced}>
      <span class="chev" class:open={advanced}><Icon name="chevronRight" size={13} stroke={2} /></span>
      More options
    </button>

    {#if advanced}
      <div class="grid">
        <label class="field c6">
          <span>Jump host</span>
          <input class="input" bind:value={jumpHost} list="nexssh-jumps" placeholder="A saved server, or user@bastion:22" spellcheck="false" />
        </label>
        <label class="field c3">
          <span>Keepalive, s</span>
          <input class="input" bind:value={keepalive} inputmode="numeric" placeholder="30" />
        </label>
        <label class="field c3">
          <span>Timeout, s</span>
          <input class="input" bind:value={timeout} inputmode="numeric" placeholder="15" />
        </label>
      </div>
      <datalist id="nexssh-jumps">
        {#each jumpOptions as s (s.id)}<option value={s.name}>{s.host}</option>{/each}
      </datalist>

      <div class="section">
        <span class="label">Port forwarding <span class="muted">— started on every connection</span></span>
        {#each forwards as f, i (i)}
          <div class="fwd">
            <select class="input kind" bind:value={f.kind}>
              <option value="local">Local</option>
              <option value="remote">Remote</option>
              <option value="dynamic">SOCKS</option>
            </select>
            <input class="input port" type="number" min="0" max="65535" bind:value={f.bindPort} title={f.kind === 'remote' ? 'Port on the server' : 'Local port'} />
            {#if f.kind !== 'dynamic'}
              <span class="arrow">→</span>
              <input class="input" bind:value={f.targetHost} placeholder="localhost" spellcheck="false" />
              <input class="input port" type="number" min="1" max="65535" bind:value={f.targetPort} />
            {:else}
              <span class="muted grow">SOCKS5 proxy on 127.0.0.1:{f.bindPort || '…'}</span>
            {/if}
            <button type="button" class="icon-btn small" aria-label="Remove" onclick={() => forwards.splice(i, 1)}>
              <Icon name="x" size={13} />
            </button>
          </div>
        {/each}
        <button type="button" class="btn ghost add-fwd" onclick={() => addForward()}>
          <Icon name="plus" size={13} /> Add forwarding rule
        </button>
      </div>
    {/if}

    {#if error}<p class="error">{error}</p>{/if}
  </form>

  {#snippet footer()}
    {#if existing}
      <button type="button" class="btn danger ghost" onclick={remove}><Icon name="trash" size={14} /> Delete</button>
    {/if}
    <span class="spacer"></span>
    <button type="button" class="btn" onclick={() => (app.editor = null)}>Cancel</button>
    <button type="submit" form="server-form" class="btn" disabled={!canSave}>Save</button>
    <button type="button" class="btn primary" disabled={!canSave} onclick={() => save(true)}>Save & Connect</button>
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
  .c6 {
    grid-column: span 6;
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
  .fwd .kind {
    width: 100px;
    flex: none;
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
