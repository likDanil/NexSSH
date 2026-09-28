<script lang="ts">
  import { sessions, type Tab } from '../state/sessions.svelte';
  import type { Prompt, PromptReply } from '../types';
  import Icon from './Icon.svelte';

  interface Props {
    tab: Tab;
    entry: { id: number; prompt: Prompt };
  }
  let { tab, entry }: Props = $props();

  const p = $derived(entry.prompt);

  let secret = $state('');
  let remember = $state(false);
  let answers = $state<string[]>([]);
  let form = $state<HTMLFormElement>();

  // Fresh fields for every new prompt, and focus the first input.
  $effect.pre(() => {
    void entry.id;
    secret = '';
    remember = false;
    answers = entry.prompt.kind === 'keyboardInteractive' ? entry.prompt.prompts.map(() => '') : [];
  });

  $effect(() => {
    void entry.id;
    requestAnimationFrame(() => {
      const target = form?.querySelector<HTMLElement>('input, [data-default]');
      target?.focus();
    });
  });

  function reply(r: PromptReply) {
    sessions.answer(tab, entry.id, r);
    secret = '';
    answers = [];
    requestAnimationFrame(() => sessions.focusActive());
  }

  function cancel() {
    reply({ kind: 'cancel' });
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    switch (p.kind) {
      case 'hostKey':
        if (p.check.status === 'changed') cancel();
        else reply({ kind: 'hostKey', accept: true, remember: true });
        break;
      case 'password':
      case 'passphrase':
        if (secret) reply({ kind: 'secret', value: secret, remember });
        break;
      case 'keyboardInteractive':
        reply({ kind: 'answers', values: [...answers] });
        break;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      cancel();
    }
  }

  function target(host: string, port: number) {
    return port === 22 ? host : `${host}:${port}`;
  }

  const KEY_TYPES: Record<string, string> = {
    'ssh-ed25519': 'ED25519',
    'ssh-rsa': 'RSA',
    'rsa-sha2-256': 'RSA',
    'rsa-sha2-512': 'RSA',
    'ecdsa-sha2-nistp256': 'ECDSA P-256',
    'ecdsa-sha2-nistp384': 'ECDSA P-384',
    'ecdsa-sha2-nistp521': 'ECDSA P-521',
  };

  function keyType(t: string) {
    return KEY_TYPES[t] ?? t.replace(/^ssh-/, '').toUpperCase();
  }
</script>

<div class="scrim">
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <form class="card" class:warn={p.kind === 'hostKey' && p.check.status === 'changed'} bind:this={form} onsubmit={submit} {onkeydown}>
    {#if p.kind === 'hostKey'}
      {#if p.check.status === 'changed'}
        <div class="head danger"><Icon name="alert" size={18} /><h3>Host key has changed</h3></div>
        <p>
          The key presented by <b>{target(p.host, p.port)}</b> differs from the one recorded in
          <code>{p.check.file}</code> (line {p.check.line}). Someone could be intercepting the connection, or the
          server was reinstalled.
        </p>
        <div class="fp"><span>Recorded</span><code>{p.check.knownFingerprint}</code></div>
        <div class="fp"><span>Presented · {keyType(p.keyType)}</span><code>{p.fingerprint}</code></div>
        <div class="buttons">
          <button type="button" class="btn danger" onclick={() => reply({ kind: 'hostKey', accept: true, remember: true })}>
            Accept new key
          </button>
          <button type="submit" class="btn primary" data-default>Cancel</button>
        </div>
      {:else}
        <div class="head"><Icon name="shield" size={18} /><h3>New host</h3></div>
        <p>
          First connection to <b>{target(p.host, p.port)}</b>. Check that the key fingerprint matches the server
          before trusting it.
        </p>
        <div class="fp"><span>{keyType(p.keyType)} key</span><code>{p.fingerprint}</code></div>
        <div class="buttons">
          <button type="button" class="btn ghost" onclick={cancel}>Cancel</button>
          <button type="button" class="btn" onclick={() => reply({ kind: 'hostKey', accept: true, remember: false })}>
            Connect once
          </button>
          <button type="submit" class="btn primary" data-default>Trust and connect</button>
        </div>
      {/if}
    {:else if p.kind === 'password' || p.kind === 'passphrase'}
      <div class="head">
        <Icon name={p.kind === 'password' ? 'lock' : 'key'} size={18} />
        <h3>{p.kind === 'password' ? 'Password' : 'Key passphrase'}</h3>
      </div>
      <p class="sub">{p.kind === 'password' ? `${p.user}@${p.host}` : p.keyPath}</p>
      <input
        class="input"
        class:invalid={!!p.error}
        type="password"
        bind:value={secret}
        placeholder={p.kind === 'password' ? 'Password' : 'Passphrase'}
        autocomplete="off"
        spellcheck="false"
      />
      {#if p.error}<p class="error">{p.error}</p>{/if}
      {#if p.canRemember}
        <label class="check remember">
          <input type="checkbox" bind:checked={remember} />
          Save in the system keychain
        </label>
      {/if}
      <div class="buttons">
        <button type="button" class="btn ghost" onclick={cancel}>Cancel</button>
        <button type="submit" class="btn primary" disabled={!secret}>
          {p.kind === 'password' ? 'Connect' : 'Unlock'}
        </button>
      </div>
    {:else if p.kind === 'keyboardInteractive'}
      <div class="head"><Icon name="shield" size={18} /><h3>{p.name || 'Verification'}</h3></div>
      <p class="sub">{p.host}</p>
      {#if p.instructions}<p class="pre">{p.instructions}</p>{/if}
      {#each p.prompts as q, i (i)}
        <label class="field">
          <span>{q.text.trim()}</span>
          <input
            class="input"
            type={q.echo ? 'text' : 'password'}
            bind:value={answers[i]}
            autocomplete={q.echo ? 'off' : 'one-time-code'}
            spellcheck="false"
          />
        </label>
      {/each}
      <div class="buttons">
        <button type="button" class="btn ghost" onclick={cancel}>Cancel</button>
        <button type="submit" class="btn primary">Continue</button>
      </div>
    {/if}
  </form>
</div>

<style>
  .scrim {
    position: absolute;
    inset: 0;
    z-index: 5;
    display: grid;
    place-items: center;
    padding: 20px;
    background: color-mix(in srgb, var(--surface) 72%, transparent);
    backdrop-filter: blur(1.5px);
    animation: fade 0.12s var(--ease);
  }
  .card {
    width: 420px;
    max-width: 100%;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 18px 18px 16px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    animation: rise 0.16s var(--ease);
  }
  .card.warn {
    border-color: color-mix(in srgb, var(--red) 45%, var(--border));
  }
  .head {
    display: flex;
    align-items: center;
    gap: 9px;
    color: var(--text-2);
  }
  .head.danger {
    color: var(--red);
  }
  h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }
  .danger h3 {
    color: var(--red);
  }
  p {
    margin: 0;
    line-height: 1.5;
    color: var(--text-2);
  }
  p b {
    color: var(--text);
    font-weight: 600;
  }
  .sub {
    margin-top: -4px;
    font-family: var(--font-mono);
    font-size: 12px;
    word-break: break-all;
  }
  .pre {
    white-space: pre-wrap;
  }
  code {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .fp {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 9px 11px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-soft);
    background: var(--surface);
  }
  .fp span {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    color: var(--text-2);
    text-transform: uppercase;
  }
  .fp code {
    word-break: break-all;
    color: var(--text);
    -webkit-user-select: text;
    user-select: text;
  }
  .error {
    margin-top: -4px;
    color: var(--red);
    font-size: 12px;
  }
  .remember {
    color: var(--text-2);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
